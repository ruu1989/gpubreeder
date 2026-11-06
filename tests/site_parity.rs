//! Reproduces rustbreeder.com's published behaviour (v3.0.4, default options) using this
//! crate's tally. The hashes below were read from the live site's results on 2026-11-06.
//! The site treats ties as chance, so this deliberately models the site, not the game: it
//! guards the shared weight and centre logic.

use gpubreeder::genes::*;
use gpubreeder::rules::*;
use std::collections::HashSet;

const SET1: &str = "WGXYYW WGGXYW WGYXGW WYYXGW XYYXGW XYGXYW YWGYGX WGGWYX XGYWGG XGYWYX WYYWGW XYYGWX YXGWGX WYGXYW WGWGYW WYWXGG XYGWGW WGYWGW XYYWYW XYGWYX XYGXYX WGGXYX XYGWYW";
const SET2: &str = "GGGYYH YGGYYH YHGYHH HGGGYH HHGGYH WGHYYY WGGYYY HWYGGH GHYXGH YGYXYH GYYXHH GHGWYH HGGGYW HHGHYW WGGGYY XHHGGH";

#[derive(Clone)]
struct Found {
    generation: usize,
    chance: f64,
    parent_generations: usize,
    plants: Vec<Genotype>,
    centre: Option<Genotype>,
}

impl Found {
    fn rank(&self) -> (usize, f64, usize, usize) {
        (self.generation, -self.chance, self.parent_generations, self.plants.len())
    }
}

struct Site {
    best: Vec<Option<Found>>,
    made_in: Vec<usize>,
}

fn half_score(g: usize) -> i32 {
    Genotype(g as u16).score(&Scores::default()) / 50
}

fn evaluate(site: &mut Site, plants: &[Genotype], centre: Option<Genotype>, slots: &[SlotResult; SLOTS], sources: &HashSet<Genotype>, generation: usize) {
    let centre_genes = centre.map(Genotype::genes);
    let mut results = vec![[0u8; SLOTS]];
    let mut ties = 0;
    for slot in 0..SLOTS {
        if let Some(cg) = centre_genes {
            if INT_WEIGHT[cg[slot] as usize] >= slots[slot].top {
                results.iter_mut().for_each(|r| r[slot] = cg[slot]);
                continue;
            }
        }
        let winners: Vec<u8> = (0..5).filter(|g| slots[slot].winners & (1 << g) != 0).collect();
        if winners.len() > 1 {
            ties += 1;
            if ties > 1 {
                return;
            }
        }
        results = results
            .iter()
            .flat_map(|r| {
                winners.iter().map(move |&w| {
                    let mut next = *r;
                    next[slot] = w;
                    next
                })
            })
            .collect();
    }
    let chance = 1.0 / results.len() as f64;
    for r in &results {
        let g = Genotype::from_genes(r);
        if sources.contains(&g) || half_score(g.0 as usize) < 8 {
            continue;
        }
        let parent_generations = plants.iter().chain(centre.iter()).map(|p| site.made_in[p.0 as usize]).sum();
        let found = Found { generation, chance, parent_generations, plants: plants.to_vec(), centre };
        let slot = &mut site.best[g.0 as usize];
        if slot.as_ref().is_none_or(|b| found.rank().partial_cmp(&b.rank()) == Some(std::cmp::Ordering::Less)) {
            *slot = Some(found);
        }
    }
}

fn cross(site: &mut Site, plants: &[Genotype], pool: &[Genotype], sources: &HashSet<Genotype>, generation: usize) {
    let slots = tally(plants);
    let early_ties = slots.iter().filter(|s| s.winners.count_ones() > 1 && s.top > 5).count();
    if early_ties > 1 {
        return;
    }
    // The site drops groups with a plant that contributes to no winning gene.
    let useless = plants.iter().any(|p| {
        let genes = p.genes();
        !(0..SLOTS).any(|s| slots[s].winners & (1 << genes[s]) != 0)
    });
    if useless {
        return;
    }
    if plants.len() <= 5 && centre_matters(&slots) {
        for &centre in pool {
            if !plants.contains(&centre) {
                evaluate(site, plants, Some(centre), &slots, sources, generation);
            }
        }
    } else {
        evaluate(site, plants, None, &slots, sources, generation);
    }
}

fn each_group(pool: &[Genotype], size: usize, first_limit: usize, visit: &mut dyn FnMut(&[Genotype])) {
    fn go(pool: &[Genotype], size: usize, from: usize, limit: usize, group: &mut Vec<Genotype>, visit: &mut dyn FnMut(&[Genotype])) {
        if group.len() == size {
            visit(group);
            return;
        }
        let end = if group.is_empty() { limit } else { pool.len() };
        for i in from..end {
            group.push(pool[i]);
            go(pool, size, i, limit, group, visit);
            group.pop();
        }
    }
    go(pool, size, 0, first_limit, &mut Vec::new(), visit);
}

fn chance_product(site: &Site, g: usize) -> f64 {
    match &site.best[g] {
        None => 1.0,
        Some(f) => {
            let mut p = f.chance;
            for parent in f.plants.iter().chain(f.centre.iter()) {
                if site.made_in[parent.0 as usize] > 0 {
                    p *= chance_product(site, parent.0 as usize);
                }
            }
            p
        }
    }
}

fn carried(site: &Site, pool: &[Genotype], generation: usize) -> Vec<Genotype> {
    let found: Vec<usize> = (0..GENOTYPES).filter(|&i| site.best[i].is_some()).collect();
    let mut ranked = found.clone();
    ranked.sort_by(|&a, &b| {
        let key = |i: usize| {
            let f = site.best[i].as_ref().unwrap();
            (-(half_score(i) as f64), -chance_product(site, i), f.generation as f64, f.parent_generations as f64)
        };
        key(a).partial_cmp(&key(b)).unwrap().then(Genotype(a as u16).to_string().cmp(&Genotype(b as u16).to_string()))
    });
    let mut maps: Vec<usize> = ranked.into_iter().filter(|&i| site.best[i].as_ref().unwrap().generation == generation).collect();
    let mut columns = [0i32; SLOTS];
    let value = |g: usize, slot: usize| Scores::default().0[Genotype(g as u16).genes()[slot] as usize] / 50;
    for p in pool {
        (0..SLOTS).for_each(|s| columns[s] += value(p.0 as usize, s));
    }
    let mut out = Vec::new();
    for _ in 0..maps.len().min(20) {
        let mut order: Vec<usize> = (0..SLOTS).collect();
        order.sort_by_key(|&s| columns[s]);
        let mut subset = maps.clone();
        for &slot in &order {
            let best = subset.iter().map(|&m| value(m, slot)).max().unwrap();
            subset.retain(|&m| value(m, slot) == best);
        }
        subset.sort_by(|&a, &b| site.best[a].as_ref().unwrap().rank().partial_cmp(&site.best[b].as_ref().unwrap().rank()).unwrap());
        let pick = subset[0];
        (0..SLOTS).for_each(|s| columns[s] += value(pick, s));
        maps.retain(|&m| m != pick);
        out.push(Genotype(pick as u16));
    }
    out
}

fn site_rows(text: &str) -> String {
    let mut site = Site { best: vec![None; GENOTYPES], made_in: vec![0; GENOTYPES] };
    let mut pool = parse_list(text).unwrap();
    let mut added = pool.len();
    for generation in 1..=2 {
        let sources: HashSet<Genotype> = pool.iter().copied().collect();
        let snapshot = pool.clone();
        for size in 2..=5 {
            each_group(&snapshot, size, added, &mut |group| cross(&mut site, group, &snapshot, &sources, generation));
        }
        if generation == 1 {
            let extra = carried(&site, &pool, 1);
            extra.iter().for_each(|g| site.made_in[g.0 as usize] = 1);
            added = extra.len();
            let mut next = extra;
            next.extend(pool);
            pool = next;
        }
    }
    let mut rows: Vec<String> = (0..GENOTYPES)
        .filter_map(|i| site.best[i].as_ref().map(|f| format!("{},{},{},{}", Genotype(i as u16), half_score(i) as f32 / 2.0, f.chance, f.generation)))
        .collect();
    rows.sort();
    rows.join(";")
}

fn fnv(text: &str) -> u32 {
    text.bytes().fold(2_166_136_261u32, |h, b| (h ^ b as u32).wrapping_mul(16_777_619))
}

#[test]
fn matches_the_live_site_on_sample_set_1() {
    let rows = site_rows(SET1);
    assert_eq!(rows.split(';').count(), 249);
    assert_eq!(fnv(&rows), 4_163_999_673);
}

#[test]
fn matches_the_live_site_on_sample_set_2() {
    let rows = site_rows(SET2);
    assert_eq!(rows.split(';').count(), 465);
    assert_eq!(fnv(&rows), 1_124_048_318);
}
