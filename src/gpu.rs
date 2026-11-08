//! GPU backend. Enumeration happens in `kernel.wgsl`; this side splits the work into batches
//! short enough not to trip the Windows GPU watchdog, then reads the claims back.

use crate::genes::*;
use crate::rules::*;
use crate::search::SearchControl;
use crate::search::{Backend, Candidate, CpuBackend, LayerRequest, LayerResult};
use anyhow::{Result, anyhow};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WANT_FREE: u32 = 0;
const UNWANTED: u32 = 1;
const CLAIMED: u32 = 2;
const PREFIX_STRIDE: usize = 14;
/// Last plants one entry may try, for groups too small to share a prefix across a workgroup.
/// Shared entries take one plant per thread instead.
const TAIL_CHUNK: usize = 16;
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
const MAX_POOL: usize = GENOTYPES;
/// Windows resets the driver when one job holds the GPU for about 2 seconds, and a game sharing the
/// card can stretch a job several times over. Keeping batches this short leaves room for that.
const TARGET_BATCH_SECONDS: f64 = 0.05;
const MAX_SIZE_TRYING_CENTRES: usize = 3;
/// Batch sizes count `Job::group_work` units, not groups.
const INITIAL_BATCH_GROUPS: f64 = 2e5;
const MIN_BATCH_GROUPS: f64 = 1e4;
/// Speed can fall 30 times over between batches, faster than measuring can react, so fast stretches
/// can't be allowed to grow a batch past what the slowest stretches finish safely.
const MAX_BATCH_GROUPS: f64 = 3e8;
/// Batches grow slowly, because speed can fall sharply from one part of the search to the next.
const BATCH_GROWTH: f64 = 1.5;
/// Far beyond any healthy batch, and long enough to outlast a driver reset.
const GPU_WAIT_LIMIT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_micros(200);

/// The GPU stopped working mid-search, usually because the driver reset. This device is finished,
/// so whoever holds it must start a new one.
#[derive(Debug)]
pub struct GpuFailure {
    /// What went wrong, for bug reports. A driver reset usually shows as a lost device.
    pub cause: String,
}

impl GpuFailure {
    pub fn new(cause: impl Into<String>) -> Self {
        GpuFailure { cause: cause.into() }
    }
}

impl std::fmt::Display for GpuFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The GPU stopped responding during the search, usually because the graphics driver reset. Press Find plans to try again. (Details: {})",
            self.cause
        )
    }
}

impl std::error::Error for GpuFailure {}

/// Picks the next batch size from how the last one went. A slow batch cuts straight to the
/// measured speed, and only full batches can grow it.
fn next_batch_budget(budget: f64, cost: f64, seconds: f64) -> f64 {
    let next = if seconds > TARGET_BATCH_SECONDS {
        budget.min(cost / seconds * TARGET_BATCH_SECONDS)
    } else if cost >= budget * 0.5 {
        budget * (TARGET_BATCH_SECONDS / seconds).min(BATCH_GROWTH)
    } else {
        budget
    };
    next.clamp(MIN_BATCH_GROUPS, MAX_BATCH_GROUPS)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    pool_len: u32,
    group_size: u32,
    shared: u32,
    prefix_count: u32,
    budget: u32,
    padding: [u32; 3],
}

const BACKENDS: wgpu::Backends = wgpu::Backends::VULKAN.union(wgpu::Backends::DX12);
/// Picks a card by part of its name, for machines with more than one, or to test on a software
/// renderer such as WARP ("Basic Render") or lavapipe ("llvmpipe").
pub const ADAPTER_VARIABLE: &str = "GPUBREEDER_ADAPTER";

/// Every card wgpu can use here, as "name (backend, type)".
pub fn list_adapters() -> Vec<String> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor { backends: BACKENDS, ..wgpu::InstanceDescriptor::new_without_display_handle() });
    pollster::block_on(instance.enumerate_adapters(BACKENDS))
        .iter()
        .map(|a| {
            let i = a.get_info();
            format!("{} ({:?}, {:?})", i.name, i.backend, i.device_type)
        })
        .collect()
}

pub struct GpuBackend {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    adapter_name: String,
    /// The card and driver in use, as wgpu reports them.
    pub info: wgpu::AdapterInfo,
    params: wgpu::Buffer,
    pool: wgpu::Buffer,
    prefixes: wgpu::Buffer,
    claims: wgpu::Buffer,
    recipes: wgpu::Buffer,
    want: wgpu::Buffer,
    centres: wgpu::Buffer,
    costs: wgpu::Buffer,
    readback: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// Genes per slot that appear in a wanted genotype, a snapshot of the last refresh.
    allowed: [u32; SLOTS],
    nothing_wanted: bool,
    deadline: Option<Instant>,
    timed_out: bool,
    /// Groups-per-batch estimate, tuned from measured speed.
    batch_budget: f64,
    /// Group evaluations in the last `layer` call and the GPU time they took.
    pub last_groups: u64,
    control: Arc<SearchControl>,
    pub last_seconds: f64,
    /// Set by wgpu when the device dies or reports an error.
    /// Set by wgpu's callbacks when the device is lost or reports an error.
    failed: Arc<Mutex<Option<String>>>,
}

fn storage(device: &wgpu::Device, label: &str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
}

impl GpuBackend {
    pub fn new() -> Result<Self> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor { backends: BACKENDS, ..wgpu::InstanceDescriptor::new_without_display_handle() });
        let adapter = match std::env::var(ADAPTER_VARIABLE) {
            Ok(wanted) if !wanted.is_empty() => {
                let all = pollster::block_on(instance.enumerate_adapters(BACKENDS));
                let names: Vec<String> = all.iter().map(|a| a.get_info().name).collect();
                all.into_iter()
                    .find(|a| a.get_info().name.to_lowercase().contains(&wanted.to_lowercase()))
                    .ok_or_else(|| anyhow!("{ADAPTER_VARIABLE}={wanted} matches none of: {}", names.join(", ")))?
            }
            _ => pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            }))
            .map_err(|e| anyhow!("no usable GPU: {e}"))?,
        };
        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?})", info.name, info.backend);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("gpubreeder"),
            required_limits: adapter.limits(),
            ..Default::default()
        }))?;
        let failed: Arc<Mutex<Option<String>>> = Arc::default();
        let lost = failed.clone();
        device.set_device_lost_callback(move |reason, message| {
            lost.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(format!("device lost ({reason:?}): {message}"));
        });
        // Without this handler wgpu panics on the search thread.
        let errored = failed.clone();
        device.on_uncaptured_error(Arc::new(move |error| {
            errored.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(format!("wgpu error: {error}"));
        }));

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
        let params = storage(&device, "params", std::mem::size_of::<Params>() as u64, U::UNIFORM | U::COPY_DST);
        let pool = storage(&device, "pool", (MAX_POOL * 24) as u64, U::STORAGE | U::COPY_DST);
        let prefixes = storage(&device, "prefixes", (MAX_PREFIXES * PREFIX_STRIDE * 4) as u64, U::STORAGE | U::COPY_DST);
        let claims = storage(&device, "claims", (GENOTYPES * 4) as u64, U::STORAGE | U::COPY_DST | U::COPY_SRC);
        let recipes = storage(&device, "recipes", (GENOTYPES * RECIPE_STRIDE * 4) as u64, U::STORAGE | U::COPY_SRC);
        let want = storage(&device, "want", ((WANT_HEADER + GENOTYPES / 32 + 1) * 4) as u64, U::STORAGE | U::COPY_DST);
        let centres = storage(&device, "centres", ((CENTRE_HEADER + 64 * MAX_POOL) * 4) as u64, U::STORAGE | U::COPY_DST);
        let costs = storage(&device, "costs", (MAX_POOL * 4) as u64, U::STORAGE | U::COPY_DST);
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
                wgpu::BindGroupEntry { binding: 7, resource: costs.as_entire_binding() },
            ],
        });

        Ok(GpuBackend {
            device,
            queue,
            pipeline,
            adapter_name,
            info,
            params,
            pool,
            prefixes,
            claims,
            recipes,
            want,
            centres,
            costs,
            deadline: None,
            timed_out: false,
            allowed: [0; SLOTS],
            nothing_wanted: false,
            readback,
            bind_group,
            batch_budget: INITIAL_BATCH_GROUPS,
            last_groups: 0,
            control: Arc::default(),
            last_seconds: 0.0,
            failed,
        })
    }

    pub fn control(&self) -> Arc<SearchControl> {
        self.control.clone()
    }

    /// Polls until `done`, giving up if the GPU fails or goes quiet. Blocking waits can't be used
    /// here, because after a driver reset they may never return.
    fn settle(&self, mut done: impl FnMut(wgpu::PollStatus) -> bool) -> Result<()> {
        let started = Instant::now();
        loop {
            let status = self.device.poll(wgpu::PollType::Poll).map_err(|e| GpuFailure::new(format!("poll failed: {e}")))?;
            if let Some(cause) = self.failed.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                return Err(GpuFailure::new(cause).into());
            }
            if done(status) {
                return Ok(());
            }
            if started.elapsed() > GPU_WAIT_LIMIT {
                return Err(GpuFailure::new(format!("no answer within {} s", GPU_WAIT_LIMIT.as_secs())).into());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn wait(&self) -> Result<()> {
        self.settle(|status| status.is_queue_empty())
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
        let mut mapped = None;
        self.settle(|_| {
            mapped = receiver.try_recv().ok();
            mapped.is_some()
        })?;
        mapped.ok_or_else(|| GpuFailure::new("readback never mapped"))?.map_err(|e| GpuFailure::new(format!("readback failed: {e}")))?;
        let data = slice.get_mapped_range().map_err(|e| GpuFailure::new(format!("readback unreadable: {e}")))?;
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
        let params = Params {
            pool_len: job.pool_len as u32,
            group_size: job.size as u32,
            shared: u32::from(job.shared()),
            prefix_count: entries as u32,
            budget: job.budget,
            padding: [0; 3],
        };
        self.dispatch(batch, params)?;
        let seconds = started.elapsed().as_secs_f64().max(1e-4);
        if trace {
            eprintln!(
                "size {} n {}: {entries} prefixes, cost {cost:.3e}, budget {:.3e}, {seconds:.3}s",
                job.size, job.pool_len, self.batch_budget
            );
        }
        self.last_groups += *cost as u64;
        self.control.groups.fetch_add(*cost as u64, Ordering::Relaxed);
        self.last_seconds += seconds;
        self.batch_budget = next_batch_budget(self.batch_budget, *cost * job.group_work(), seconds);
        batch.clear();
        *cost = 0.0;
        self.timed_out |= self.deadline.is_some_and(|d| Instant::now() >= d) || self.control.cancel.load(Ordering::Relaxed);
        self.refresh_wanted()
    }

    /// Lists every useful group prefix, depth first, dispatching as batches fill.
    #[allow(clippy::too_many_arguments)]
    fn walk(
        &mut self,
        job: &Job,
        depth: usize,
        from: usize,
        spent: u32,
        counts: [u32; SLOTS],
        ids: &mut [usize; MAX_NEIGHBOURS],
        batch: &mut Vec<u32>,
        cost: &mut f64,
    ) -> Result<()> {
        if self.nothing_wanted || self.timed_out {
            return Ok(());
        }
        if depth == job.prefix_len() {
            let first = ids[depth - 1];
            let mut words = [0u32; 4];
            for (i, &id) in ids[..depth].iter().enumerate() {
                words[i / 2] |= (id as u32) << (16 * (i % 2));
            }
            let reach = job.costs.partition_point(|&c| c <= job.budget - spent).max(first + 1);
            // One entry runs on one GPU thread, and with a whole pool of centres to try it can
            // run past the watchdog alone. Splitting it also lets the GPU work on it in parallel.
            let chunk = if job.shared() { WORKGROUP as usize } else { TAIL_CHUNK };
            let mut start = first;
            while start < reach && !self.nothing_wanted && !self.timed_out {
                let stop = (start + chunk).min(reach);
                batch.extend_from_slice(&counts);
                batch.push(first as u32);
                batch.extend_from_slice(&words);
                batch.push(spent);
                batch.extend_from_slice(&[start as u32, stop as u32]);
                let tail = (stop - start) as f64;
                // A shared entry pairs each plant in its range with every plant after it.
                *cost += if job.shared() { tail * (reach - start + reach - stop + 1) as f64 / 2.0 } else { tail };
                if *cost * job.group_work() >= self.batch_budget || batch.len() / PREFIX_STRIDE >= MAX_PREFIXES {
                    self.flush(job, batch, cost)?;
                }
                start = stop;
            }
            return Ok(());
        }
        let extra = (job.size - depth - 1) as u32;
        for next in from..job.pool_len {
            let price = if depth > 0 && next == ids[depth - 1] { 0 } else { job.costs[next] };
            if spent + price > job.budget {
                break;
            }
            let mut sum = counts;
            for slot in 0..SLOTS {
                sum[slot] += job.packs[next][slot];
            }
            if self.can_help(&sum, extra) {
                ids[depth] = next;
                self.walk(job, depth + 1, next, spent + price, sum, ids, batch, cost)?;
            }
        }
        Ok(())
    }

    /// Runs all groups of one size.
    fn run_size(&mut self, packs: &[[u32; SLOTS]], costs: &[u32], budget: u32, size: usize) -> Result<()> {
        let job = Job { packs, costs, budget, pool_len: packs.len(), size };
        // Speed differs a lot between sizes because small groups try every centre. Never start a
        // size above what the GPU just managed, since it may be busy with something else.
        self.batch_budget = self.batch_budget.min(INITIAL_BATCH_GROUPS);
        let mut batch: Vec<u32> = Vec::with_capacity(MAX_PREFIXES * PREFIX_STRIDE);
        let mut cost = 0f64;
        let mut ids = [0usize; MAX_NEIGHBOURS];
        self.walk(&job, 0, 0, 0, [0; SLOTS], &mut ids, &mut batch, &mut cost)?;
        self.flush(&job, &mut batch, &mut cost)
    }
}

/// One group size's work description.
struct Job<'a> {
    packs: &'a [[u32; SLOTS]],
    costs: &'a [u32],
    budget: u32,
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

    /// Rough GPU work per group. Up to three plants rarely beat a centre's weight in every slot,
    /// so each group tries most of the pool as its centre. Larger groups almost never do.
    fn group_work(&self) -> f64 {
        if self.size <= MAX_SIZE_TRYING_CENTRES { self.pool_len as f64 } else { 1.0 }
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

    fn layer(&mut self, request: &LayerRequest) -> Result<LayerResult> {
        let (pool, wanted, max_plants) = (request.pool, request.wanted, request.max_plants);
        if pool.len() > MAX_POOL {
            return Err(anyhow!("pool of {} exceeds the {MAX_POOL} plant limit", pool.len()));
        }
        self.last_groups = 0;
        self.last_seconds = 0.0;
        self.deadline = request.deadline;
        self.timed_out = false;
        self.batch_budget = INITIAL_BATCH_GROUPS;

        // Single neighbours are trivial, and leave only sizes that fit the prefix scheme.
        let mut found = CpuBackend.layer(&LayerRequest { max_plants: 1, deadline: None, ..*request })?.found;
        let mut still_wanted = wanted.to_vec();
        for (g, _) in &found {
            still_wanted[g.0 as usize] = false;
        }

        let packs: Vec<[u32; SLOTS]> = pool.iter().map(|&g| packed(g)).collect();
        let flat: Vec<u32> = packs.iter().flatten().copied().collect();
        self.queue.write_buffer(&self.pool, 0, bytemuck::cast_slice(&flat));
        self.queue.write_buffer(&self.centres, 0, bytemuck::cast_slice(&centre_table(pool)));
        self.queue.write_buffer(&self.costs, 0, bytemuck::cast_slice(request.costs));
        let initial: Vec<u32> = still_wanted.iter().map(|&w| if w { WANT_FREE } else { UNWANTED }).collect();
        self.queue.write_buffer(&self.claims, 0, bytemuck::cast_slice(&initial));
        self.refresh_wanted()?;

        // Ascending size means each genotype is claimed by its smallest group first.
        for size in 2..=max_plants.min(MAX_NEIGHBOURS) {
            if self.nothing_wanted || self.timed_out {
                break;
            }
            self.run_size(&packs, request.costs, request.budget, size)?;
        }

        let (claims, recipes) = self.read_results()?;
        // A reset can wipe GPU memory without an error, and an empty result would read as proof
        // that nothing was reachable. Plants nobody asked for must still be marked as such.
        let odd = (0..GENOTYPES).filter(|&i| claims[i] > CLAIMED || (!still_wanted[i] && claims[i] != UNWANTED)).collect::<Vec<_>>();
        if let Some(&first) = odd.first() {
            return Err(GpuFailure::new(format!(
                "results look wiped: {} genotypes have impossible claims, the first is {} with {:#x}",
                odd.len(),
                Genotype(first as u16),
                claims[first]
            ))
            .into());
        }
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
        Ok(LayerResult { found, complete: !self.timed_out })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selftest;

    #[test]
    fn a_slow_batch_shrinks_at_once_and_growth_stays_gentle() {
        let budget = 1e8;
        // A game takes the card, and the same work runs 20 times slower.
        assert_eq!(next_batch_budget(budget, budget, 1.0), budget * TARGET_BATCH_SECONDS);
        // A fast batch can't grow the next one enough to reach the watchdog.
        assert_eq!(next_batch_budget(budget, budget, 0.001), budget * BATCH_GROWTH);
        // A short end-of-size batch says nothing about speed, unless it was slow.
        assert_eq!(next_batch_budget(budget, 1e3, 0.001), budget);
        assert!(next_batch_budget(budget, 1e3, 0.5) < budget);
        // A stalled batch can't push the size to nothing.
        assert_eq!(next_batch_budget(budget, 1.0, 2.0), MIN_BATCH_GROUPS);
        // However fast it has been going, a batch stays under the ceiling.
        assert_eq!(next_batch_budget(MAX_BATCH_GROUPS, MAX_BATCH_GROUPS, 0.001), MAX_BATCH_GROUPS);
    }

    /// Runs no compute, so it's safe beside a game.
    #[test]
    fn a_lost_gpu_is_reported_and_never_hangs() {
        let mut backend = GpuBackend::new().expect("GPU required for this test");
        backend.device.destroy();
        let inputs = parse_list("GGGYYH XGHGWY HGGGYH").unwrap();
        let costs = vec![0; inputs.len()];
        let wanted = vec![true; GENOTYPES];
        let started = Instant::now();
        let result =
            backend.layer(&LayerRequest { pool: &inputs, costs: &costs, wanted: &wanted, max_plants: 5, budget: 0, deadline: None });
        assert!(result.err().is_some_and(|e| e.is::<GpuFailure>()));
        assert!(started.elapsed() < GPU_WAIT_LIMIT * 2);
        std::mem::forget(backend);
    }

    /// The same cases `gpubreeder --selftest` runs.
    #[test]
    fn gpu_matches_cpu() {
        let mut gpu = GpuBackend::new().expect("GPU required for this test");
        for case in selftest::cases() {
            assert_eq!(selftest::compare(&mut gpu, &case).unwrap(), None, "{}", case.name);
        }
    }
}
