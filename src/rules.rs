//! The game's crossbreeding rules. `docs/RULES.md` explains them in prose.
//!
//! `game_cross` is the reference: it follows the game step by step, including 32-bit float
//! weights and strict comparisons. Search code uses faster integer logic, and every plan is
//! checked against `game_cross` before it is reported.

use crate::genes::*;

/// Crossbreeding weight of each gene, as stored in the game's data.
pub const GAME_WEIGHT: [f32; GENE_KINDS] = [0.6, 0.6, 0.6, 1.0, 1.0];
/// The same weights scaled by 5 so sums are exact integers.
pub const INT_WEIGHT: [u8; GENE_KINDS] = [3, 3, 3, 5, 5];
/// The most plants that can surround a centre in a planter.
pub const MAX_NEIGHBOURS: usize = 8;
/// A red gene's scaled weight: the most a centre gene can weigh.
pub const CENTRE_LIMIT: u8 = 5;

/// Reference simulation: `order` is the neighbours in planting order.
pub fn game_cross(order: &[Genotype], centre: Genotype) -> Genotype {
    let centre_genes = centre.genes();
    let neighbours: Vec<[u8; SLOTS]> = order.iter().map(|g| g.genes()).collect();
    let mut result = centre_genes;
    for slot in 0..SLOTS {
        let mut totals = [0f32; GENE_KINDS];
        let (mut lead_weight, mut lead_gene) = (0f32, 0u8);
        for genes in &neighbours {
            let gene = genes[slot];
            totals[gene as usize] += GAME_WEIGHT[gene as usize];
            if totals[gene as usize] > lead_weight {
                lead_weight = totals[gene as usize];
                lead_gene = gene;
            }
        }
        if lead_weight > GAME_WEIGHT[centre_genes[slot] as usize] {
            result[slot] = lead_gene;
        }
    }
    Genotype::from_genes(&result)
}

/// The winning total and tied winners for one slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotResult {
    /// Highest scaled total, 0 if there are no neighbours.
    pub top: u8,
    /// Bit `g` is set when gene `g` reached `top`.
    pub winners: u8,
}

/// Gene counts for one slot, four bits per gene.
pub fn slot_result(packed_counts: u32) -> SlotResult {
    let mut top = 0u8;
    let mut winners = 0u8;
    for gene in 0..GENE_KINDS {
        let count = ((packed_counts >> (4 * gene)) & 15) as u8;
        let total = count * INT_WEIGHT[gene];
        if total > top {
            top = total;
            winners = 1 << gene;
        } else if total == top && total > 0 {
            winners |= 1 << gene;
        }
    }
    SlotResult { top, winners }
}

/// One slot's packed count contribution per plant.
pub fn packed(g: Genotype) -> [u32; SLOTS] {
    let genes = g.genes();
    std::array::from_fn(|slot| 1u32 << (4 * genes[slot]))
}

/// Tally for an unordered group of neighbours.
pub fn tally(plants: &[Genotype]) -> [SlotResult; SLOTS] {
    let mut counts = [0u32; SLOTS];
    for &plant in plants {
        let p = packed(plant);
        for slot in 0..SLOTS {
            counts[slot] += p[slot];
        }
    }
    counts.map(slot_result)
}

/// True when the centre plant's genes can affect this group.
pub fn centre_matters(slots: &[SlotResult; SLOTS]) -> bool {
    slots.iter().any(|s| s.top <= CENTRE_LIMIT)
}

/// Finds a planting order producing `want`, or `None` if no order can.
///
/// Among tied genes, the one whose final copy is planted first wins. So each tied slot demands
/// that the wanted gene's last copy comes before the last copy of every rival. Building from
/// the back, any plant not needed before a rival can safely go last, which makes this greedy
/// choice complete.
pub fn planting_order(plants: &[Genotype], centre: Option<Genotype>, want: Genotype) -> Option<Vec<Genotype>> {
    let slots = tally(plants);
    let want_genes = want.genes();
    let centre_genes = centre.map(Genotype::genes);
    let genes: Vec<[u8; SLOTS]> = plants.iter().map(|p| p.genes()).collect();

    let mut rules: Vec<(usize, u8, u8)> = Vec::new();
    for slot in 0..SLOTS {
        if let Some(cg) = centre_genes {
            if INT_WEIGHT[cg[slot] as usize] >= slots[slot].top {
                if cg[slot] != want_genes[slot] {
                    return None;
                }
                continue;
            }
        }
        let winners = slots[slot].winners;
        if winners & (1 << want_genes[slot]) == 0 {
            return None;
        }
        for rival in 0..GENE_KINDS as u8 {
            if rival != want_genes[slot] && winners & (1 << rival) != 0 {
                rules.push((slot, want_genes[slot], rival));
            }
        }
    }

    let mut left: Vec<usize> = (0..plants.len()).collect();
    let mut reversed = Vec::with_capacity(plants.len());
    while !left.is_empty() {
        let pick = left.iter().position(|&i| !rules.iter().any(|&(s, must_precede, _)| genes[i][s] == must_precede))?;
        let plant = left.remove(pick);
        rules.retain(|&(s, _, rival)| genes[plant][s] != rival);
        reversed.push(plants[plant]);
    }
    reversed.reverse();
    Some(reversed)
}

/// Confirms a plan with the reference simulation. With no named centre the result must hold
/// whatever the centre is, so it tries centres that are all one gene, covering every slot value.
pub fn verify(order: &[Genotype], centre: Option<Genotype>, want: Genotype) -> bool {
    match centre {
        Some(c) => game_cross(order, c) == want,
        None => (0..GENE_KINDS as u8).all(|g| game_cross(order, Genotype::from_genes(&[g; SLOTS])) == want),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn g(text: &str) -> Genotype {
        Genotype::parse(text).unwrap()
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    fn all_orders(items: &mut Vec<Genotype>, from: usize, visit: &mut dyn FnMut(&[Genotype])) {
        if from == items.len() {
            visit(items);
            return;
        }
        for i in from..items.len() {
            items.swap(from, i);
            all_orders(items, from + 1, visit);
            items.swap(from, i);
        }
    }

    #[test]
    fn five_greens_tie_three_reds_at_float_precision() {
        let mut sum = 0f32;
        for _ in 0..5 {
            sum += 0.6;
        }
        assert_eq!(sum, 3.0);
        let mut other = 0f32;
        for _ in 0..3 {
            other += 1.0;
        }
        assert_eq!(sum, other);
    }

    #[test]
    fn first_gene_to_reach_the_top_total_wins_a_tie() {
        let (a, b) = (g("GGGGGG"), g("YYYYYY"));
        assert_eq!(game_cross(&[a, b], g("WWWWWW")), g("WWWWWW"), "0.6 cannot beat a red centre");
        assert_eq!(game_cross(&[a, b], g("XXXXXX")), g("XXXXXX"));
        assert_eq!(game_cross(&[a, a, b, b], g("WWWWWW")), a);
        assert_eq!(game_cross(&[b, b, a, a], g("WWWWWW")), b);
        // G reaches 1.2 at the third plant, Y only at the fourth.
        assert_eq!(game_cross(&[a, b, a, b], g("WWWWWW")), a);
        assert_eq!(game_cross(&[b, a, a, b], g("WWWWWW")), a);
    }

    #[test]
    fn equal_weight_keeps_the_centre_gene() {
        assert_eq!(game_cross(&[g("GGGGGG")], g("YYYYYY")), g("YYYYYY"));
        assert_eq!(game_cross(&[g("WWWWWW")], g("XXXXXX")), g("XXXXXX"));
        assert_eq!(game_cross(&[g("WWWWWW")], g("GGGGGG")), g("WWWWWW"));
    }

    #[test]
    fn the_centre_is_irrelevant_once_every_slot_beats_it() {
        let plants = [g("GGGGGG"), g("GGGGGG"), g("YYYYYY")];
        assert!(!centre_matters(&tally(&plants)));
        assert!(verify(&plants, None, g("GGGGGG")));
    }

    #[test]
    fn a_tie_order_that_conflicts_is_rejected() {
        // Wanting G in slot 0 forces the G copies first; wanting Y in slot 1 forces the reverse.
        let a = g("GYWWWW");
        let b = g("YGWWWW");
        let plants = [a, a, b, b];
        assert!(planting_order(&plants, None, g("GYWWWW")).is_some());
        assert!(planting_order(&plants, None, g("GGWWWW")).is_none());
    }

    /// The central claim: for random neighbours, the outcomes reachable across every planting
    /// order are exactly those `planting_order` says are reachable.
    #[test]
    fn predictions_match_the_reference_across_every_order() {
        let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
        let mut with_ties = 0;
        for trial in 0..12_000u64 {
            let count = 1 + (trial % 7) as usize;
            let narrow: [u64; SLOTS] = std::array::from_fn(|_| rng.next());
            let mut plants: Vec<Genotype> = (0..count)
                .map(|_| {
                    let mut genes = Genotype((rng.next() % GENOTYPES as u64) as u16).genes();
                    for slot in 0..SLOTS {
                        if narrow[slot] % 3 != 0 {
                            genes[slot] = [genes[slot] % 2, 3 + genes[slot] % 2][(narrow[slot] >> 8) as usize % 2];
                        }
                    }
                    Genotype::from_genes(&genes)
                })
                .collect();
            let centre = Genotype((rng.next() % GENOTYPES as u64) as u16);

            let mut reachable = HashSet::new();
            all_orders(&mut plants, 0, &mut |o| {
                reachable.insert(game_cross(o, centre));
            });

            let slots = tally(&plants);
            let cg = centre.genes();
            let mut candidates = vec![cg];
            for slot in 0..SLOTS {
                if INT_WEIGHT[cg[slot] as usize] >= slots[slot].top {
                    continue;
                }
                let mut next = Vec::new();
                for c in &candidates {
                    for gene in 0..GENE_KINDS as u8 {
                        if slots[slot].winners & (1 << gene) != 0 {
                            let mut v = *c;
                            v[slot] = gene;
                            next.push(v);
                        }
                    }
                }
                candidates = next;
            }

            let mut predicted = HashSet::new();
            for c in candidates {
                let want = Genotype::from_genes(&c);
                if let Some(order) = planting_order(&plants, Some(centre), want) {
                    assert!(verify(&order, Some(centre), want));
                    predicted.insert(want);
                }
            }
            assert_eq!(predicted, reachable, "plants {plants:?} centre {centre}");
            if reachable.len() > 1 {
                with_ties += 1;
            }
        }
        assert!(with_ties > 1000, "test must exercise ties, saw {with_ties}");
    }

    /// Eight plants is the largest case, so check it directly on a smaller sample.
    #[test]
    fn predictions_hold_at_eight_plants() {
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        for _ in 0..40 {
            let base: Vec<Genotype> = (0..4).map(|_| Genotype((rng.next() % GENOTYPES as u64) as u16)).collect();
            let mut plants: Vec<Genotype> = (0..8).map(|i| base[i % 4]).collect();
            let centre = Genotype((rng.next() % GENOTYPES as u64) as u16);
            let mut reachable = HashSet::new();
            all_orders(&mut plants, 0, &mut |o| {
                reachable.insert(game_cross(o, centre));
            });
            for &want in &reachable {
                let order = planting_order(&plants, Some(centre), want).expect("reachable outcome needs an order");
                assert!(verify(&order, Some(centre), want));
            }
        }
    }
}
