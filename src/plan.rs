//! Cheapest-first search over crossings, and reconstruction of step-by-step plans.

use crate::genes::*;
use crate::rules::*;
use crate::search::*;
use anyhow::{bail, Result};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct PlanConfig {
    /// Stop after this many crossings per plant.
    pub max_crossings: u32,
    pub max_plants: usize,
    pub target: Option<Genotype>,
    pub time_limit: Option<Duration>,
}

#[derive(Clone, Debug)]
pub struct RoundStats {
    /// Result plants of this round cost this many crossings.
    pub crossings: u32,
    pub pool: usize,
    pub new_genotypes: usize,
    pub complete: bool,
    pub seconds: f64,
}

/// A reached genotype and how it was made.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Crossings including every ancestor, counted separately per branch.
    pub cost: u32,
    /// Longest chain of crossings before this plant exists.
    pub generation: u32,
    pub recipe: Recipe,
}

pub struct Outcome {
    pub inputs: Vec<Genotype>,
    pub reached: BTreeMap<Genotype, Entry>,
    pub stats: Vec<RoundStats>,
}

impl Outcome {
    /// Costs up to this are proven cheapest: every round up to it ran to completion.
    pub fn proven_up_to(&self) -> u32 {
        self.stats.iter().take_while(|s| s.complete).map(|s| s.crossings).max().unwrap_or(0)
    }
}

pub fn run(inputs: &[Genotype], cfg: &PlanConfig, backend: &mut dyn Backend, mut progress: impl FnMut(&RoundStats)) -> Result<Outcome> {
    let mut seen = HashSet::new();
    let inputs: Vec<Genotype> = inputs.iter().copied().filter(|g| seen.insert(*g)).collect();
    let mut known = vec![false; GENOTYPES];
    let mut generation_of = vec![0u32; GENOTYPES];
    for g in &inputs {
        known[g.0 as usize] = true;
    }
    let mut pool = inputs.clone();
    let mut costs = vec![0u32; pool.len()];
    let mut outcome = Outcome { inputs, reached: BTreeMap::new(), stats: Vec::new() };
    let deadline = cfg.time_limit.map(|limit| Instant::now() + limit);

    for budget in 0..cfg.max_crossings {
        let started = Instant::now();
        let wanted: Vec<bool> = known.iter().map(|k| !k).collect();
        let result = backend.layer(&LayerRequest { pool: &pool, costs: &costs, wanted: &wanted, max_plants: cfg.max_plants, budget, deadline })?;
        let recipes: Vec<Recipe> = result.found.par_iter().map(|(g, c)| confirm(*g, c)).collect::<Result<_>>()?;
        let new_genotypes = recipes.len();
        for recipe in recipes {
            let parents = recipe.order.iter().chain(recipe.centre.iter());
            let generation = 1 + parents.map(|p| generation_of[p.0 as usize]).max().unwrap_or(0);
            let result = recipe.result;
            known[result.0 as usize] = true;
            generation_of[result.0 as usize] = generation;
            pool.push(result);
            costs.push(budget + 1);
            outcome.reached.insert(result, Entry { cost: budget + 1, generation, recipe });
        }
        let stats = RoundStats { crossings: budget + 1, pool: pool.len(), new_genotypes, complete: result.complete, seconds: started.elapsed().as_secs_f64() };
        progress(&stats);
        outcome.stats.push(stats);

        let target_done = cfg.target.is_some_and(|t| outcome.reached.contains_key(&t));
        if target_done || !result.complete || known.iter().all(|&k| k) {
            break;
        }
    }
    Ok(outcome)
}

/// A complete plan, parents first.
pub struct Plan {
    pub goal: Genotype,
    pub generation: u32,
    pub steps: Vec<Recipe>,
    /// Input genotypes the plan uses.
    pub inputs_used: Vec<Genotype>,
}

impl Outcome {
    pub fn plan(&self, goal: Genotype) -> Option<Plan> {
        let generation = self.reached.get(&goal)?.generation;
        let mut steps = Vec::new();
        let mut done = HashSet::new();
        let mut used = HashSet::new();
        fn visit(outcome: &Outcome, g: Genotype, steps: &mut Vec<Recipe>, done: &mut HashSet<Genotype>, used: &mut HashSet<Genotype>) {
            let Some(entry) = outcome.reached.get(&g) else {
                used.insert(g);
                return;
            };
            if !done.insert(g) {
                return;
            }
            for &parent in entry.recipe.order.iter().chain(entry.recipe.centre.iter()) {
                visit(outcome, parent, steps, done, used);
            }
            steps.push(entry.recipe.clone());
        }
        visit(self, goal, &mut steps, &mut done, &mut used);
        let mut inputs_used: Vec<Genotype> = used.into_iter().collect();
        inputs_used.sort();
        Some(Plan { goal, generation, steps, inputs_used })
    }

    /// Best results: highest score, then fewest crossings, then fewest generations.
    pub fn best(&self, scores: &Scores, count: usize) -> Vec<Plan> {
        let top = self.reached.keys().map(|g| g.score(scores)).max().unwrap_or(0);
        let goals = self.reached.keys().copied().filter(|g| g.score(scores) == top);
        let mut plans: Vec<Plan> = goals.filter_map(|g| self.plan(g)).collect();
        plans.sort_by_key(|p| (p.steps.len(), p.generation, p.goal));
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

    fn config(max_crossings: u32, target: Option<Genotype>) -> PlanConfig {
        PlanConfig { max_crossings, max_plants: 3, target, time_limit: None }
    }

    const SMALL: &str = "GGGYYH XGHGWY HGGGYH YGHWGX WGGYYY";

    #[test]
    fn every_plan_replays_with_real_rules() {
        let inputs = parse_list(SMALL).unwrap();
        let outcome = run(&inputs, &config(2, None), &mut CpuBackend, |_| {}).unwrap();
        assert!(outcome.reached.len() > 50);
        for &goal in outcome.reached.keys() {
            outcome.plan(goal).unwrap().check(&outcome.inputs).unwrap();
        }
    }

    #[test]
    fn a_plan_with_a_missing_parent_is_rejected() {
        let inputs = parse_list(SMALL).unwrap();
        let outcome = run(&inputs, &config(1, None), &mut CpuBackend, |_| {}).unwrap();
        let plan = outcome.best(&Scores::default(), 1).remove(0);
        assert!(plan.check(&[Genotype::parse("HHHHHH").unwrap()]).is_err());
    }

    /// Shortest routes from a tiny pool, computed independently by exhausting every chain.
    #[test]
    fn prices_are_the_true_minimum() {
        let inputs = parse_list("GGGGGY YYYYYW").unwrap();
        let outcome = run(&inputs, &PlanConfig { max_crossings: 3, max_plants: 3, target: None, time_limit: None }, &mut CpuBackend, |_| {}).unwrap();

        // Bellman-Ford over the reference simulation: cost of a crossing is its distinct parents.
        let mut best: BTreeMap<Genotype, u32> = inputs.iter().map(|g| (*g, 0)).collect();
        for _ in 0..4 {
            let plants: Vec<(Genotype, u32)> = best.iter().map(|(g, c)| (*g, *c)).collect();
            let mut updates = Vec::new();
            for &(a, ca) in &plants {
                for &(b, cb) in &plants {
                    for &(c, cc) in &plants {
                        for &(centre, cz) in &plants {
                            let mut seen: BTreeMap<Genotype, u32> = BTreeMap::new();
                            for (g, cost) in [(a, ca), (b, cb), (c, cc), (centre, cz)] {
                                seen.insert(g, cost);
                            }
                            let price = 1 + seen.values().sum::<u32>();
                            if price > 3 {
                                continue;
                            }
                            for group in [vec![a], vec![a, b], vec![a, b, c]] {
                                updates.push((game_cross(&group, centre), price));
                            }
                        }
                    }
                }
            }
            for (g, price) in updates {
                let slot = best.entry(g).or_insert(u32::MAX);
                *slot = (*slot).min(price);
            }
        }
        for (g, price) in best.iter().filter(|(_, p)| **p > 0) {
            let entry = outcome.reached.get(g).unwrap_or_else(|| panic!("{g} at price {price} was missed"));
            assert!(entry.cost <= *price, "{g}: search says {}, reference says {price}", entry.cost);
        }
    }

    #[test]
    fn a_target_search_stops_once_reached() {
        let inputs = parse_list(SMALL).unwrap();
        let outcome = run(&inputs, &config(1, None), &mut CpuBackend, |_| {}).unwrap();
        let target = *outcome.reached.keys().next().unwrap();
        let focused = run(&inputs, &config(5, Some(target)), &mut CpuBackend, |_| {}).unwrap();
        assert_eq!(focused.stats.len(), 1);
        assert!(focused.plan(target).is_some());
    }
}
