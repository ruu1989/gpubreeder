//! Chains search layers into generations and reconstructs step-by-step plans.

use crate::genes::*;
use crate::rules::*;
use crate::search::*;
use anyhow::{bail, Result};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct PlanConfig {
    pub generations: usize,
    /// Most plants carried into the next generation.
    pub carry: usize,
    pub layer: LayerConfig,
    pub target: Option<Genotype>,
}

#[derive(Clone, Debug)]
pub struct GenerationStats {
    pub generation: usize,
    pub pool: usize,
    pub new_genotypes: usize,
    pub groups: u128,
    pub seconds: f64,
}

/// Every genotype reached, with how to make it.
pub struct Outcome {
    pub inputs: Vec<Genotype>,
    pub recipes: BTreeMap<Genotype, (usize, Recipe)>,
    pub stats: Vec<GenerationStats>,
}

fn multisets(n: u128, k: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    (1..=k).fold(1u128, |acc, i| acc * (n + i - 1) / i)
}

/// Picks plants that fill the pool's weakest slots, judged by `value`.
fn choose_carry(pool: &[Genotype], bench: &[Genotype], carry: usize, value: impl Fn(u8, usize) -> i32) -> Vec<Genotype> {
    let mut columns = [0i64; SLOTS];
    for plant in pool {
        for (slot, gene) in plant.genes().into_iter().enumerate() {
            columns[slot] += value(gene, slot) as i64;
        }
    }
    let mut left: Vec<(Genotype, [i32; SLOTS])> = bench
        .iter()
        .map(|&g| {
            let genes = g.genes();
            (g, std::array::from_fn(|slot| value(genes[slot], slot)))
        })
        .collect();
    let mut chosen = Vec::new();
    for _ in 0..carry.min(left.len()) {
        let mut order: [usize; SLOTS] = std::array::from_fn(|i| i);
        order.sort_by_key(|&slot| columns[slot]);
        let key = |values: &[i32; SLOTS]| order.map(|slot| values[slot]);
        let pick = left.par_iter().enumerate().max_by_key(|(_, (g, values))| (key(values), g.0)).map(|(i, _)| i).unwrap();
        let (plant, values) = left.swap_remove(pick);
        for slot in 0..SLOTS {
            columns[slot] += values[slot] as i64;
        }
        chosen.push(plant);
    }
    chosen
}

pub fn run(inputs: &[Genotype], cfg: &PlanConfig, backend: &mut dyn Backend, mut progress: impl FnMut(&GenerationStats)) -> Result<Outcome> {
    let mut seen = HashSet::new();
    let inputs: Vec<Genotype> = inputs.iter().copied().filter(|g| seen.insert(*g)).collect();
    let mut known = vec![false; GENOTYPES];
    for g in &inputs {
        known[g.0 as usize] = true;
    }
    let mut pool = inputs.clone();
    let mut outcome = Outcome { inputs, recipes: BTreeMap::new(), stats: Vec::new() };
    let target = cfg.target.map(Genotype::genes);
    let scores = cfg.layer.scores;
    let value = |gene: u8, slot: usize| match target {
        Some(t) => i32::from(t[slot] == gene),
        None => scores.0[gene as usize],
    };

    for generation in 1..=cfg.generations {
        let started = Instant::now();
        let wanted = match cfg.target {
            Some(t) if generation == cfg.generations => (0..GENOTYPES).map(|i| i == t.0 as usize && !known[i]).collect(),
            _ => wanted_mask(&known, &cfg.layer),
        };
        let found = backend.layer(&pool, &wanted, cfg.layer.max_plants)?;
        let recipes: Vec<Recipe> = found.par_iter().map(|(g, c)| confirm(*g, c)).collect::<Result<_>>()?;
        let new_genotypes = recipes.len();
        for recipe in recipes {
            known[recipe.result.0 as usize] = true;
            outcome.recipes.insert(recipe.result, (generation, recipe));
        }
        let stats = GenerationStats {
            generation,
            pool: pool.len(),
            new_genotypes,
            groups: (1..=cfg.layer.max_plants.min(MAX_NEIGHBOURS) as u128).map(|k| multisets(pool.len() as u128, k)).sum(),
            seconds: started.elapsed().as_secs_f64(),
        };
        progress(&stats);
        outcome.stats.push(stats);

        if cfg.target.is_some_and(|t| outcome.recipes.contains_key(&t)) || generation == cfg.generations {
            break;
        }
        let in_pool: HashSet<Genotype> = pool.iter().copied().collect();
        let bench: Vec<Genotype> = outcome.recipes.keys().copied().filter(|g| !in_pool.contains(g)).collect();
        if bench.is_empty() {
            break;
        }
        let chosen = choose_carry(&pool, &bench, cfg.carry, value);
        pool.extend(chosen);
    }
    Ok(outcome)
}

/// A complete plan, parents first.
pub struct Plan {
    pub goal: Genotype,
    pub generation: usize,
    pub steps: Vec<Recipe>,
    /// Input genotypes the plan uses.
    pub inputs_used: Vec<Genotype>,
}

impl Outcome {
    pub fn plan(&self, goal: Genotype) -> Option<Plan> {
        let (generation, _) = self.recipes.get(&goal)?;
        let mut steps = Vec::new();
        let mut done = HashSet::new();
        let mut used = HashSet::new();
        fn visit(outcome: &Outcome, g: Genotype, steps: &mut Vec<Recipe>, done: &mut HashSet<Genotype>, used: &mut HashSet<Genotype>) {
            let Some((_, recipe)) = outcome.recipes.get(&g) else {
                used.insert(g);
                return;
            };
            if !done.insert(g) {
                return;
            }
            for &parent in recipe.order.iter().chain(recipe.centre.iter()) {
                visit(outcome, parent, steps, done, used);
            }
            steps.push(recipe.clone());
        }
        visit(self, goal, &mut steps, &mut done, &mut used);
        let mut inputs_used: Vec<Genotype> = used.into_iter().collect();
        inputs_used.sort();
        Some(Plan { goal, generation: *generation, steps, inputs_used })
    }

    /// Best results: highest score, then earliest generation, then fewest crossings.
    pub fn best(&self, scores: &Scores, count: usize) -> Vec<Plan> {
        let top = self.recipes.keys().map(|g| g.score(scores)).max().unwrap_or(0);
        let mut goals: Vec<Genotype> = self.recipes.keys().copied().filter(|g| g.score(scores) == top).collect();
        goals.sort();
        let mut plans: Vec<Plan> = goals.into_iter().filter_map(|g| self.plan(g)).collect();
        plans.sort_by_key(|p| (p.generation, p.steps.len(), p.goal));
        plans.truncate(count);
        plans
    }
}

impl Plan {
    /// Replays the whole plan with the reference rules, using only plants that exist by then.
    pub fn check(&self, inputs: &[Genotype]) -> Result<()> {
        let mut have: HashSet<Genotype> = inputs.iter().copied().collect();
        for step in &self.steps {
            for plant in step.order.iter().chain(step.centre.iter()) {
                if !have.contains(plant) {
                    bail!("plan for {} uses {plant} before it exists", self.goal);
                }
            }
            if !verify(&step.order, step.centre, step.result) {
                bail!("plan for {} fails at {}", self.goal, step.result);
            }
            have.insert(step.result);
        }
        if !have.contains(&self.goal) {
            bail!("plan for {} never produces it", self.goal);
        }
        Ok(())
    }
}

/// True when planting order changes this crossing's result.
pub fn order_matters(recipe: &Recipe) -> bool {
    let slots = tally(&recipe.order);
    (0..SLOTS).any(|slot| {
        let held = recipe.centre.is_some_and(|c| INT_WEIGHT[c.genes()[slot] as usize] >= slots[slot].top);
        !held && slots[slot].winners.count_ones() > 1
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(generations: usize, target: Option<Genotype>) -> PlanConfig {
        PlanConfig {
            generations,
            carry: 40,
            layer: LayerConfig { max_plants: 5, min_score: 300, scores: Scores::default() },
            target,
        }
    }

    const SET1: &str = "WGXYYW WGGXYW WGYXGW WYYXGW XYYXGW XYGXYW YWGYGX WGGWYX XGYWGG XGYWYX WYYWGW XYYGWX YXGWGX WYGXYW WGWGYW WYWXGG XYGWGW WGYWGW XYYWYW XYGWYX XYGXYX WGGXYX XYGWYW";

    #[test]
    fn every_plan_replays_with_real_rules() {
        let inputs = parse_list(SET1).unwrap();
        let outcome = run(&inputs, &config(2, None), &mut CpuBackend, |_| {}).unwrap();
        assert!(outcome.recipes.len() > 200);
        for &goal in outcome.recipes.keys() {
            outcome.plan(goal).unwrap().check(&outcome.inputs).unwrap();
        }
    }

    #[test]
    fn a_plan_with_a_missing_parent_is_rejected() {
        let inputs = parse_list(SET1).unwrap();
        let outcome = run(&inputs, &config(2, None), &mut CpuBackend, |_| {}).unwrap();
        let plan = outcome.best(&Scores::default(), 1).remove(0);
        let stranger = Genotype::parse("HHHHHH").unwrap();
        assert!(plan.check(&[stranger]).is_err());
    }

    #[test]
    fn a_target_search_stops_once_reached() {
        let inputs = parse_list(SET1).unwrap();
        let target = Genotype::parse("YGGYGG").unwrap();
        let outcome = run(&inputs, &config(3, Some(target)), &mut CpuBackend, |_| {}).unwrap();
        let plan = outcome.plan(target).expect("site reaches this in two generations");
        assert!(plan.generation <= 2);
        plan.check(&outcome.inputs).unwrap();
    }

    #[test]
    fn multiset_count_matches_known_values() {
        assert_eq!(multisets(23, 5), 80_730);
        assert_eq!(multisets(70 - 7, 8), 9_440_350_920);
    }
}
