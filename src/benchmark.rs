//! Measures how many neighbour groups the GPU checks per second, on the same work everywhere.
//!
//! The work is one crossing round over 100 clones, the most a player realistically holds, drawn
//! from a fixed seed. Speed depends on the work as well as the card, so only results from this
//! workload compare across machines. A game running on the same card lowers the result.

use crate::genes::*;
use crate::gpu::GpuBackend;
use crate::search::*;
use anyhow::Result;
use std::time::{Duration, Instant};

pub const CLONES: usize = 100;
pub const NEIGHBOURS: usize = 5;
/// Longest single run, so a slow card still finishes several.
const RUN_LIMIT: Duration = Duration::from_secs(4);

pub struct Report {
    /// Groups per second of each run, in the order they ran.
    pub runs: Vec<f64>,
    pub seconds: f64,
}

impl Report {
    pub fn best(&self) -> f64 {
        self.runs.iter().copied().fold(0.0, f64::max)
    }

    pub fn median(&self) -> f64 {
        let mut sorted = self.runs.clone();
        sorted.sort_by(f64::total_cmp);
        sorted.get(sorted.len() / 2).copied().unwrap_or(0.0)
    }
}

/// The fixed clone list every benchmark searches.
pub fn workload() -> Vec<Genotype> {
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut out = Vec::with_capacity(CLONES);
    while out.len() < CLONES {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let g = Genotype((seed % GENOTYPES as u64) as u16);
        if !out.contains(&g) {
            out.push(g);
        }
    }
    out
}

/// Repeats the workload for about `duration` and reports each run's speed. `progress` gets each
/// run's groups per second as it finishes. The first run includes warming up, so the best run is
/// the card's real top speed.
pub fn run(backend: &mut GpuBackend, duration: Duration, mut progress: impl FnMut(f64)) -> Result<Report> {
    let pool = workload();
    let costs = vec![0u32; pool.len()];
    let mut wanted = vec![true; GENOTYPES];
    pool.iter().for_each(|g| wanted[g.0 as usize] = false);
    let started = Instant::now();
    let mut runs = Vec::new();
    while runs.is_empty() || started.elapsed() < duration {
        let left = duration.saturating_sub(started.elapsed()).max(Duration::from_millis(500));
        let deadline = Some(Instant::now() + left.min(RUN_LIMIT));
        let request = LayerRequest { pool: &pool, costs: &costs, wanted: &wanted, max_plants: NEIGHBOURS, budget: 0, deadline };
        let result = backend.layer(&request)?;
        if backend.last_seconds > 0.0 {
            let speed = backend.last_groups as f64 / backend.last_seconds;
            runs.push(speed);
            progress(speed);
        }
        if backend.control().cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        // A run that finished its round says nothing more by repeating forever on a fast card.
        if result.complete && runs.len() >= 20 {
            break;
        }
    }
    Ok(Report { runs, seconds: started.elapsed().as_secs_f64() })
}
