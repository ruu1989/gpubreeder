//! One generation of search: every group of neighbours, every centre, every tie outcome.
//!
//! A `Backend` reports, per genotype, one group of plants that can produce it. Whichever
//! backend ran, `confirm` turns that into a verified planting order.

use crate::genes::*;
use crate::rules::*;
use anyhow::{bail, Result};
use rayon::prelude::*;

/// How to run one layer of search.
#[derive(Clone, Copy, Debug)]
pub struct LayerConfig {
    pub max_plants: usize,
    pub min_score: i32,
    pub scores: Scores,
}

/// A group of plants found by a backend, not yet given an order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub plants: Vec<Genotype>,
    /// `None` means any plant of the same crop works as the centre.
    pub centre: Option<Genotype>,
}

/// A verified way to produce a genotype.
#[derive(Clone, Debug)]
pub struct Recipe {
    pub result: Genotype,
    /// Neighbours in the order they must be planted.
    pub order: Vec<Genotype>,
    pub centre: Option<Genotype>,
}

pub trait Backend {
    fn name(&self) -> String;
    /// `wanted[g]` says the caller needs genotype `g`. Returns a candidate for every wanted
    /// genotype that can be produced, preferring fewer neighbours.
    fn layer(&mut self, pool: &[Genotype], wanted: &[bool], max_plants: usize) -> Result<Vec<(Genotype, Candidate)>>;
}

/// Genotypes worth searching for: new, and at or above the score floor.
pub fn wanted_mask(known: &[bool], cfg: &LayerConfig) -> Vec<bool> {
    (0..GENOTYPES).map(|i| !known[i] && Genotype(i as u16).score(&cfg.scores) >= cfg.min_score).collect()
}

/// Turns a backend's candidate into a recipe, refusing anything the reference rejects.
pub fn confirm(result: Genotype, candidate: &Candidate) -> Result<Recipe> {
    let Some(order) = planting_order(&candidate.plants, candidate.centre, result) else {
        bail!("backend claimed {result} from {candidate:?}, but no planting order produces it");
    };
    if !verify(&order, candidate.centre, result) {
        bail!("backend claimed {result} from {candidate:?}, but the game rules disagree");
    }
    Ok(Recipe { result, order, centre: candidate.centre })
}

/// Fast per-slot lookup indexed by four-bit gene counts.
struct SlotTable(Vec<SlotResult>);

impl SlotTable {
    fn new() -> Self {
        SlotTable((0..1u32 << 20).map(slot_result).collect())
    }
}

/// Every outcome of a group for one optional centre, calling `visit` on new wanted genotypes.
fn outcomes(slots: &[SlotResult; SLOTS], centre: Option<[u8; SLOTS]>, wanted: &[bool], mut visit: impl FnMut(usize)) {
    let mut choices = [0u8; SLOTS];
    let mut tied = false;
    for slot in 0..SLOTS {
        choices[slot] = match centre {
            Some(cg) if INT_WEIGHT[cg[slot] as usize] >= slots[slot].top => 1 << cg[slot],
            _ => slots[slot].winners,
        };
        tied |= choices[slot].count_ones() > 1;
    }
    if !tied {
        let index = choices.iter().fold(0usize, |acc, &m| acc * 5 + m.trailing_zeros() as usize);
        if wanted[index] {
            visit(index);
        }
        return;
    }
    fn walk(choices: &[u8; SLOTS], slot: usize, index: usize, wanted: &[bool], visit: &mut impl FnMut(usize)) {
        if slot == SLOTS {
            if wanted[index] {
                visit(index);
            }
            return;
        }
        let mut mask = choices[slot];
        while mask != 0 {
            let gene = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            walk(choices, slot + 1, index * 5 + gene, wanted, visit);
        }
    }
    walk(&choices, 0, 0, wanted, &mut visit);
}

/// Multi-threaded reference search. Deterministic: results don't depend on thread timing.
pub struct CpuBackend;

impl Backend for CpuBackend {
    fn name(&self) -> String {
        format!("CPU ({} threads)", rayon::current_num_threads())
    }

    fn layer(&mut self, pool: &[Genotype], wanted: &[bool], max_plants: usize) -> Result<Vec<(Genotype, Candidate)>> {
        let table = SlotTable::new();
        let packs: Vec<[u32; SLOTS]> = pool.iter().map(|&g| packed(g)).collect();
        let centres: Vec<(Genotype, [u8; SLOTS])> = pool.iter().map(|&g| (g, g.genes())).collect();
        let mut remaining = wanted.to_vec();
        let mut found = Vec::new();

        for size in 1..=max_plants.min(MAX_NEIGHBOURS) {
            let shards: Vec<Vec<Option<Candidate>>> = (0..pool.len())
                .into_par_iter()
                .map(|first| {
                    let mut best: Vec<Option<Candidate>> = vec![None; GENOTYPES];
                    let mut ids = [0usize; MAX_NEIGHBOURS];
                    ids[0] = first;
                    let walk = Walk { pool, packs: &packs, centres: &centres, table: &table, wanted: &remaining, size };
                    walk.descend(1, first, packs[first], &mut ids, &mut best);
                    best
                })
                .collect();
            let mut merged: Vec<Option<Candidate>> = vec![None; GENOTYPES];
            for shard in shards {
                for (slot, candidate) in merged.iter_mut().zip(shard) {
                    if slot.is_none() {
                        *slot = candidate;
                    }
                }
            }
            for (index, candidate) in merged.into_iter().enumerate() {
                if let Some(candidate) = candidate {
                    remaining[index] = false;
                    found.push((Genotype(index as u16), candidate));
                }
            }
        }
        Ok(found)
    }
}

struct Walk<'a> {
    pool: &'a [Genotype],
    packs: &'a [[u32; SLOTS]],
    centres: &'a [(Genotype, [u8; SLOTS])],
    table: &'a SlotTable,
    wanted: &'a [bool],
    size: usize,
}

impl Walk<'_> {
    fn descend(&self, depth: usize, from: usize, counts: [u32; SLOTS], ids: &mut [usize; MAX_NEIGHBOURS], best: &mut [Option<Candidate>]) {
        if depth == self.size {
            self.leaf(&ids[..depth], counts, best);
            return;
        }
        for next in from..self.pool.len() {
            ids[depth] = next;
            let mut sum = counts;
            for slot in 0..SLOTS {
                sum[slot] += self.packs[next][slot];
            }
            self.descend(depth + 1, next, sum, ids, best);
        }
    }

    fn leaf(&self, ids: &[usize], counts: [u32; SLOTS], best: &mut [Option<Candidate>]) {
        let slots: [SlotResult; SLOTS] = std::array::from_fn(|slot| self.table.0[counts[slot] as usize]);
        let plants = |ids: &[usize]| ids.iter().map(|&i| self.pool[i]).collect::<Vec<_>>();
        if centre_matters(&slots) {
            for &(centre, genes) in self.centres {
                outcomes(&slots, Some(genes), self.wanted, |index| {
                    if best[index].is_none() {
                        let group = plants(ids);
                        if planting_order(&group, Some(centre), Genotype(index as u16)).is_some() {
                            best[index] = Some(Candidate { plants: group, centre: Some(centre) });
                        }
                    }
                });
            }
        } else {
            outcomes(&slots, None, self.wanted, |index| {
                if best[index].is_none() {
                    let group = plants(ids);
                    if planting_order(&group, None, Genotype(index as u16)).is_some() {
                        best[index] = Some(Candidate { plants: group, centre: None });
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn pool() -> Vec<Genotype> {
        parse_list("GGGYYH XGHGWY HGGGYH YGHWGX").unwrap()
    }

    /// Brute force over ordered neighbour lists with the reference simulation only.
    fn reference_results(pool: &[Genotype], max_plants: usize) -> HashSet<Genotype> {
        fn go(pool: &[Genotype], order: &mut Vec<Genotype>, left: usize, out: &mut HashSet<Genotype>) {
            if !order.is_empty() {
                for &c in pool {
                    out.insert(game_cross(order, c));
                }
            }
            if left == 0 {
                return;
            }
            for &p in pool {
                order.push(p);
                go(pool, order, left - 1, out);
                order.pop();
            }
        }
        let mut out = HashSet::new();
        go(pool, &mut Vec::new(), max_plants, &mut out);
        out
    }

    #[test]
    fn cpu_search_finds_exactly_what_brute_force_finds() {
        let pool = pool();
        let everything = vec![true; GENOTYPES];
        let found = CpuBackend.layer(&pool, &everything, 5).unwrap();
        let ours: HashSet<Genotype> = found.iter().map(|(g, _)| *g).collect();
        assert_eq!(ours, reference_results(&pool, 5));
        for (g, candidate) in &found {
            confirm(*g, candidate).unwrap();
        }
    }

    #[test]
    fn confirm_rejects_a_false_claim() {
        let claim = Candidate { plants: parse_list("GGGGGG GGGGGG").unwrap(), centre: None };
        assert!(confirm(Genotype::parse("YYYYYY").unwrap(), &claim).is_err());
    }
}
