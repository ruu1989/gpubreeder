//! Checks a backend against the CPU on fixed plant lists, so anyone can test their own card.
//!
//! The GPU tests and `gpubreeder --selftest` both run these cases. A bad driver should only cost
//! missed routes, because every claim is re-checked on the CPU, but a missed route could make a
//! plan look like the shortest when it isn't. This is how that would show up.

use crate::genes::*;
use crate::search::*;
use anyhow::Result;
use std::collections::BTreeMap;

/// One comparison: settle `rounds` rounds from `inputs` with at most `max_plants` neighbours.
pub struct Case {
    pub name: String,
    pub inputs: Vec<Genotype>,
    pub max_plants: usize,
    pub rounds: u32,
}

/// The example lists from rustbreeder.com, under its MIT licence. See NOTICE.
const SAMPLE_SETS: [(&str, &str); 2] = [
    (
        "Rust Breeder sample 1",
        "WGXYYW WGGXYW WGYXGW WYYXGW XYYXGW XYGXYW YWGYGX WGGWYX XGYWGG XGYWYX WYYWGW XYYGWX YXGWGX WYGXYW WGWGYW WYWXGG XYGWGW WGYWGW XYYWYW XYGWYX XYGXYX WGGXYX XYGWYW",
    ),
    (
        "Rust Breeder sample 2",
        "GGGYYH YGGYYH YHGYHH HGGGYH HHGGYH WGHYYY WGGYYY HWYGGH GHYXGH YGYXYH GYYXHH GHGWYH HGGGYW HHGHYW WGGGYY XHHGGH",
    ),
];

/// Two real-sized lists at both extremes of neighbour count, then 30 small random pools.
pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for (name, text) in SAMPLE_SETS {
        let inputs = parse_list(text).expect("sample sets parse");
        out.push(Case { name: format!("{name}, 8 neighbours, 1 round"), inputs: inputs.clone(), max_plants: 8, rounds: 1 });
        out.push(Case { name: format!("{name}, 4 neighbours, 3 rounds"), inputs, max_plants: 4, rounds: 3 });
    }
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for trial in 0..30 {
        let size = 2 + trial % 7;
        let inputs: Vec<Genotype> = (0..size).map(|_| Genotype((next() % GENOTYPES as u64) as u16)).collect();
        let max_plants = 1 + trial % 8;
        let rounds = 1 + (trial % 3) as u32;
        out.push(Case {
            name: format!("Random pool {}, {size} plants, {max_plants} neighbours, {rounds} rounds", trial + 1),
            inputs,
            max_plants,
            rounds,
        });
    }
    out
}

/// Settles genotypes round by round, as the planner does. Returns each one's price and group size.
/// Every candidate must pass the reference rules, or this returns an error.
pub fn settle(backend: &mut dyn Backend, inputs: &[Genotype], max_plants: usize, rounds: u32) -> Result<BTreeMap<Genotype, (u32, usize)>> {
    let mut known = vec![false; GENOTYPES];
    inputs.iter().for_each(|g| known[g.0 as usize] = true);
    let mut pool = inputs.to_vec();
    let mut costs = vec![0u32; pool.len()];
    let mut out = BTreeMap::new();
    for budget in 0..rounds {
        let wanted: Vec<bool> = known.iter().map(|k| !k).collect();
        let result = backend.layer(&LayerRequest { pool: &pool, costs: &costs, wanted: &wanted, max_plants, budget, deadline: None })?;
        anyhow::ensure!(result.complete, "round {} stopped early", budget + 1);
        for (g, candidate) in result.found {
            confirm(g, &candidate)?;
            known[g.0 as usize] = true;
            out.insert(g, (budget + 1, candidate.plants.len()));
            pool.push(g);
            costs.push(budget + 1);
        }
    }
    Ok(out)
}

/// Runs one case on `backend` and on the CPU. Returns a description of the first difference in
/// which plants were found, or at what price, or `None` when they agree.
pub fn compare(backend: &mut dyn Backend, case: &Case) -> Result<Option<String>> {
    let theirs = settle(backend, &case.inputs, case.max_plants, case.rounds)?;
    let cpu = settle(&mut CpuBackend, &case.inputs, case.max_plants, case.rounds)?;
    for (g, (price, _)) in &cpu {
        match theirs.get(g) {
            None => return Ok(Some(format!("missed {g}, which the CPU makes in {price} crossing(s)"))),
            Some((p, _)) if p != price => return Ok(Some(format!("priced {g} at {p} crossing(s), the CPU at {price}"))),
            _ => {}
        }
    }
    if let Some((g, _)) = theirs.iter().find(|(g, _)| !cpu.contains_key(g)) {
        return Ok(Some(format!("made {g}, which the CPU did not find in these rounds")));
    }
    Ok(None)
}
