//! GPU backend. Enumeration happens in `kernel.wgsl`; this side splits the work into batches
//! short enough not to trip the Windows GPU watchdog, then reads the claims back.

use crate::genes::*;
use crate::rules::*;
use crate::search::{Backend, Candidate, CpuBackend};
use anyhow::{anyhow, Result};
use std::time::Instant;

const WANT_FREE: u32 = 0;
const UNWANTED: u32 = 1;
const CLAIMED: u32 = 2;
const PREFIX_STRIDE: usize = 11;
const RECIPE_STRIDE: usize = 6;
const WORKGROUP: u32 = 64;
/// The want buffer starts with one allowed-gene mask per slot, then one bit per genotype.
const WANT_HEADER: usize = SLOTS;
/// The centre table starts with 65 offsets, one range per set of slots.
const CENTRE_HEADER: usize = 65;
const MAX_PREFIXES: usize = 1 << 22;
/// Largest dispatch along one axis.
const DISPATCH_ROW: u32 = 65_535;
/// From this group size up, a whole workgroup shares one prefix and adds two plants.
const SHARED_FROM_SIZE: usize = 4;
const MAX_POOL: usize = 4096;
/// Long enough to keep the GPU busy, short enough to stay well clear of the watchdog.
const TARGET_BATCH_SECONDS: f64 = 0.25;
const INITIAL_BATCH_GROUPS: f64 = 2e5;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    pool_len: u32,
    group_size: u32,
    shared: u32,
    prefix_count: u32,
}

pub struct GpuBackend {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    adapter_name: String,
    params: wgpu::Buffer,
    pool: wgpu::Buffer,
    prefixes: wgpu::Buffer,
    claims: wgpu::Buffer,
    recipes: wgpu::Buffer,
    want: wgpu::Buffer,
    centres: wgpu::Buffer,
    readback: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// Genes per slot that appear in a wanted genotype, a snapshot of the last refresh.
    allowed: [u32; SLOTS],
    nothing_wanted: bool,
    /// Groups-per-batch estimate, tuned from measured speed.
    batch_budget: f64,
    /// Group evaluations in the last `layer` call and the GPU time they took.
    pub last_groups: u64,
    pub last_seconds: f64,
}

fn storage(device: &wgpu::Device, label: &str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
}

impl GpuBackend {
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::DX12,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| anyhow!("no usable GPU: {e}"))?;
        let adapter_name = format!("{} ({:?})", adapter.get_info().name, adapter.get_info().backend);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("gpubreeder"),
            required_limits: adapter.limits(),
            ..Default::default()
        }))?;

        let shader = unsafe {
            // Indexes are all derived from buffer sizes set here, and bounds checks cost real
            // speed in the innermost loop. The result-equality tests cover this path.
            device.create_shader_module_trusted(
                wgpu::ShaderModuleDescriptor {
                    label: Some("kernel"),
                    source: wgpu::ShaderSource::Wgsl(include_str!("kernel.wgsl").into()),
                },
                wgpu::ShaderRuntimeChecks::unchecked(),
            )
        };
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("kernel"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        use wgpu::BufferUsages as U;
        let params = storage(&device, "params", 16, U::UNIFORM | U::COPY_DST);
        let pool = storage(&device, "pool", (MAX_POOL * 24) as u64, U::STORAGE | U::COPY_DST);
        let prefixes = storage(&device, "prefixes", (MAX_PREFIXES * PREFIX_STRIDE * 4) as u64, U::STORAGE | U::COPY_DST);
        let claims = storage(&device, "claims", (GENOTYPES * 4) as u64, U::STORAGE | U::COPY_DST | U::COPY_SRC);
        let recipes = storage(&device, "recipes", (GENOTYPES * RECIPE_STRIDE * 4) as u64, U::STORAGE | U::COPY_SRC);
        let want = storage(&device, "want", ((WANT_HEADER + GENOTYPES / 32 + 1) * 4) as u64, U::STORAGE | U::COPY_DST);
        let centres = storage(&device, "centres", ((CENTRE_HEADER + 64 * MAX_POOL) * 4) as u64, U::STORAGE | U::COPY_DST);
        let readback = storage(&device, "readback", (GENOTYPES * (RECIPE_STRIDE + 1) * 4) as u64, U::MAP_READ | U::COPY_DST);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: pool.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: prefixes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: claims.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: recipes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: want.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: centres.as_entire_binding() },
            ],
        });

        Ok(GpuBackend {
            device,
            queue,
            pipeline,
            adapter_name,
            params,
            pool,
            prefixes,
            claims,
            recipes,
            want,
            centres,
            allowed: [0; SLOTS],
            nothing_wanted: false,
            readback,
            bind_group,
            batch_budget: INITIAL_BATCH_GROUPS,
            last_groups: 0,
            last_seconds: 0.0,
        })
    }

    fn wait(&self) -> Result<()> {
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("GPU wait failed: {e:?}"))?;
        Ok(())
    }

    fn dispatch(&self, prefix_data: &[u32], params: Params) -> Result<()> {
        self.queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        self.queue.write_buffer(&self.prefixes, 0, bytemuck::cast_slice(prefix_data));
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            let groups = if params.shared != 0 { params.prefix_count } else { params.prefix_count.div_ceil(WORKGROUP) };
            pass.dispatch_workgroups(groups.min(DISPATCH_ROW), groups.div_ceil(DISPATCH_ROW).max(1), 1);
        }
        self.queue.submit([encoder.finish()]);
        self.wait()
    }

    /// Reads (claims, recipes) back from the GPU.
    fn read_results(&self) -> Result<(Vec<u32>, Vec<u32>)> {
        let claim_bytes = (GENOTYPES * 4) as u64;
        let recipe_bytes = (GENOTYPES * RECIPE_STRIDE * 4) as u64;
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.claims, 0, &self.readback, 0, claim_bytes);
        encoder.copy_buffer_to_buffer(&self.recipes, 0, &self.readback, claim_bytes, recipe_bytes);
        self.queue.submit([encoder.finish()]);

        let slice = self.readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.wait()?;
        receiver.recv()?.map_err(|e| anyhow!("GPU readback failed: {e:?}"))?;
        let data = slice.get_mapped_range().map_err(|e| anyhow!("GPU readback failed: {e:?}"))?;
        let words: &[u32] = bytemuck::cast_slice(&data);
        let claims = words[..GENOTYPES].to_vec();
        let recipes = words[GENOTYPES..].to_vec();
        drop(data);
        self.readback.unmap();
        Ok((claims, recipes))
    }

    /// Refreshes what is still wanted from the GPU's claims and passes it back to the kernel.
    fn refresh_wanted(&mut self) -> Result<()> {
        let (claims, _) = self.read_results()?;
        let mut words = vec![0u32; WANT_HEADER + GENOTYPES / 32 + 1];
        let mut allowed = [0u32; SLOTS];
        let mut any = false;
        for (index, &claim) in claims.iter().enumerate() {
            if claim == WANT_FREE {
                any = true;
                words[WANT_HEADER + index / 32] |= 1 << (index % 32);
                for (slot, gene) in Genotype(index as u16).genes().into_iter().enumerate() {
                    allowed[slot] |= 1 << gene;
                }
            }
        }
        words[..SLOTS].copy_from_slice(&allowed);
        self.queue.write_buffer(&self.want, 0, bytemuck::cast_slice(&words));
        self.allowed = allowed;
        self.nothing_wanted = !any;
        Ok(())
    }

    /// False when no completion of these counts, with `extra` more plants, can give a wanted
    /// gene in some slot. Only slots already past any centre's weight can rule a group out,
    /// because a centre can still hold its gene in the others.
    fn can_help(&self, counts: &[u32; SLOTS], extra: u32) -> bool {
        (0..SLOTS).all(|slot| {
            let mut top = 0u32;
            let mut best = 0u32;
            for gene in 0..GENE_KINDS {
                let count = (counts[slot] >> (4 * gene)) & 15;
                let weight = INT_WEIGHT[gene] as u32;
                top = top.max(count * weight);
                if self.allowed[slot] & (1 << gene) != 0 {
                    best = best.max((count + extra) * weight);
                }
            }
            top <= CENTRE_LIMIT as u32 || best >= top
        })
    }

    fn flush(&mut self, job: &Job, batch: &mut Vec<u32>, cost: &mut f64) -> Result<()> {
        let entries = batch.len() / PREFIX_STRIDE;
        if entries == 0 {
            return Ok(());
        }
        let trace = std::env::var_os("GPUBREEDER_TRACE").is_some();
        let started = Instant::now();
        let params = Params { pool_len: job.pool_len as u32, group_size: job.size as u32, shared: u32::from(job.shared()), prefix_count: entries as u32 };
        self.dispatch(batch, params)?;
        let seconds = started.elapsed().as_secs_f64().max(1e-4);
        if trace {
            eprintln!("size {} n {}: {entries} prefixes, cost {cost:.3e}, budget {:.3e}, {seconds:.3}s", job.size, job.pool_len, self.batch_budget);
        }
        self.last_groups += *cost as u64;
        self.last_seconds += seconds;
        // Only full batches say anything about speed.
        if *cost >= self.batch_budget * 0.5 {
            let scale = (TARGET_BATCH_SECONDS / seconds).clamp(0.25, 4.0);
            self.batch_budget = (self.batch_budget * scale).clamp(1e6, 2e12);
        }
        batch.clear();
        *cost = 0.0;
        self.refresh_wanted()
    }

    /// Lists every useful group prefix, depth first, dispatching as batches fill.
    fn walk(&mut self, job: &Job, depth: usize, from: usize, counts: [u32; SLOTS], ids: &mut [usize; MAX_NEIGHBOURS], batch: &mut Vec<u32>, cost: &mut f64) -> Result<()> {
        if self.nothing_wanted {
            return Ok(());
        }
        if depth == job.prefix_len() {
            batch.extend_from_slice(&counts);
            let first = ids[depth - 1];
            batch.push(first as u32);
            let mut words = [0u32; 4];
            for (i, &id) in ids[..depth].iter().enumerate() {
                words[i / 2] |= (id as u32) << (16 * (i % 2));
            }
            batch.extend_from_slice(&words);
            let tail = (job.pool_len - first) as f64;
            *cost += if job.shared() { tail * (tail + 1.0) / 2.0 } else { tail };
            if *cost >= self.batch_budget || batch.len() / PREFIX_STRIDE >= MAX_PREFIXES {
                self.flush(job, batch, cost)?;
            }
            return Ok(());
        }
        let extra = (job.size - depth - 1) as u32;
        for next in from..job.pool_len {
            let mut sum = counts;
            for slot in 0..SLOTS {
                sum[slot] += job.packs[next][slot];
            }
            if self.can_help(&sum, extra) {
                ids[depth] = next;
                self.walk(job, depth + 1, next, sum, ids, batch, cost)?;
            }
        }
        Ok(())
    }

    /// Runs all groups of one size.
    fn run_size(&mut self, packs: &[[u32; SLOTS]], size: usize) -> Result<()> {
        let job = Job { packs, pool_len: packs.len(), size };
        // Speed differs a lot between sizes because small groups try every centre.
        self.batch_budget = INITIAL_BATCH_GROUPS;
        let mut batch: Vec<u32> = Vec::with_capacity(MAX_PREFIXES * PREFIX_STRIDE);
        let mut cost = 0f64;
        let mut ids = [0usize; MAX_NEIGHBOURS];
        self.walk(&job, 0, 0, [0; SLOTS], &mut ids, &mut batch, &mut cost)?;
        self.flush(&job, &mut batch, &mut cost)
    }
}

/// One group size's work description.
struct Job<'a> {
    packs: &'a [[u32; SLOTS]],
    pool_len: usize,
    size: usize,
}

impl Job<'_> {
    fn shared(&self) -> bool {
        self.size >= SHARED_FROM_SIZE
    }

    fn prefix_len(&self) -> usize {
        self.size - if self.shared() { 2 } else { 1 }
    }
}

/// For each set of slots, the pool indexes of centres with distinct genes in those slots.
fn centre_table(pool: &[Genotype]) -> Vec<u32> {
    let genes: Vec<[u8; SLOTS]> = pool.iter().map(|g| g.genes()).collect();
    let mut offsets = vec![0u32; CENTRE_HEADER];
    let mut list = Vec::new();
    for held in 0..64usize {
        offsets[held] = list.len() as u32;
        let mut seen = std::collections::HashSet::new();
        for (index, plant) in genes.iter().enumerate() {
            let key: [u8; SLOTS] = std::array::from_fn(|slot| if held & (1 << slot) != 0 { plant[slot] } else { 0 });
            if seen.insert(key) {
                list.push(index as u32);
            }
        }
    }
    offsets[64] = list.len() as u32;
    offsets.extend(list);
    offsets
}

impl Backend for GpuBackend {
    fn name(&self) -> String {
        self.adapter_name.clone()
    }

    fn layer(&mut self, pool: &[Genotype], wanted: &[bool], max_plants: usize) -> Result<Vec<(Genotype, Candidate)>> {
        if pool.len() > MAX_POOL {
            return Err(anyhow!("pool of {} exceeds the {MAX_POOL} plant limit", pool.len()));
        }
        self.last_groups = 0;
        self.last_seconds = 0.0;

        // Single neighbours are trivial, and leave only sizes that fit the prefix scheme.
        let mut found = CpuBackend.layer(pool, wanted, 1)?;
        let mut still_wanted = wanted.to_vec();
        for (g, _) in &found {
            still_wanted[g.0 as usize] = false;
        }

        let packs: Vec<[u32; SLOTS]> = pool.iter().map(|&g| packed(g)).collect();
        let flat: Vec<u32> = packs.iter().flatten().copied().collect();
        self.queue.write_buffer(&self.pool, 0, bytemuck::cast_slice(&flat));
        self.queue.write_buffer(&self.centres, 0, bytemuck::cast_slice(&centre_table(pool)));
        let initial: Vec<u32> = still_wanted.iter().map(|&w| if w { WANT_FREE } else { UNWANTED }).collect();
        self.queue.write_buffer(&self.claims, 0, bytemuck::cast_slice(&initial));
        self.refresh_wanted()?;

        // Ascending size means each genotype is claimed by its smallest group first.
        for size in 2..=max_plants.min(MAX_NEIGHBOURS) {
            if self.nothing_wanted {
                break;
            }
            self.run_size(&packs, size)?;
        }

        let (claims, recipes) = self.read_results()?;
        for index in 0..GENOTYPES {
            if claims[index] != CLAIMED {
                continue;
            }
            let record = &recipes[index * RECIPE_STRIDE..(index + 1) * RECIPE_STRIDE];
            let size = record[5] as usize;
            let plants = (0..size)
                .map(|i| {
                    let id = (record[i / 2] >> (16 * (i % 2))) & 0xFFFF;
                    pool.get(id as usize).copied().ok_or_else(|| anyhow!("GPU returned plant id {id}"))
                })
                .collect::<Result<Vec<_>>>()?;
            let centre = match record[4] {
                0 => None,
                c => Some(*pool.get(c as usize - 1).ok_or_else(|| anyhow!("GPU returned centre id {c}"))?),
            };
            found.push((Genotype(index as u16), Candidate { plants, centre }));
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{confirm, CpuBackend};
    use std::collections::BTreeSet;

    fn compare(pool: &[Genotype], wanted: &[bool], max_plants: usize) {
        let cpu = CpuBackend.layer(pool, wanted, max_plants).unwrap();
        let mut gpu_backend = GpuBackend::new().expect("GPU required for this test");
        let gpu = gpu_backend.layer(pool, wanted, max_plants).unwrap();
        for (g, candidate) in &gpu {
            confirm(*g, candidate).unwrap();
        }
        let cpu_set: BTreeSet<Genotype> = cpu.iter().map(|x| x.0).collect();
        let gpu_set: BTreeSet<Genotype> = gpu.iter().map(|x| x.0).collect();
        assert_eq!(gpu_set, cpu_set);
        // Both pick the smallest group, so the sizes must agree too.
        for (g, c) in &cpu {
            let matching = gpu.iter().find(|x| x.0 == *g).unwrap();
            assert_eq!(matching.1.plants.len(), c.plants.len(), "group size for {g}");
        }
    }

    #[test]
    fn gpu_matches_cpu_on_sample_sets() {
        let everything = vec![true; GENOTYPES];
        for text in [
            "WGXYYW WGGXYW WGYXGW WYYXGW XYYXGW XYGXYW YWGYGX WGGWYX XGYWGG XGYWYX WYYWGW XYYGWX YXGWGX WYGXYW WGWGYW WYWXGG XYGWGW WGYWGW XYYWYW XYGWYX XYGXYX WGGXYX XYGWYW",
            "GGGYYH YGGYYH YHGYHH HGGGYH HHGGYH WGHYYY WGGYYY HWYGGH GHYXGH YGYXYH GYYXHH GHGWYH HGGGYW HHGHYW WGGGYY XHHGGH",
        ] {
            compare(&parse_list(text).unwrap(), &everything, 8);
        }
    }

    #[test]
    fn gpu_matches_cpu_on_random_pools() {
        let everything = vec![true; GENOTYPES];
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for trial in 0..40 {
            let size = 2 + trial % 9;
            let pool: Vec<Genotype> = (0..size).map(|_| Genotype((next() % GENOTYPES as u64) as u16)).collect();
            compare(&pool, &everything, 1 + trial % 8);
        }
    }

    #[test]
    fn gpu_respects_the_wanted_mask() {
        let pool = parse_list("GGGYYH XGHGWY HGGGYH YGHWGX").unwrap();
        let scores = Scores::default();
        let wanted: Vec<bool> = (0..GENOTYPES).map(|i| Genotype(i as u16).score(&scores) >= 400).collect();
        compare(&pool, &wanted, 6);
    }
}
