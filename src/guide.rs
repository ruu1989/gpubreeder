//! Turns a plan into timed instructions: what to plant, when, and what to do with the result.

use crate::genes::*;
use crate::plan::{Plan, order_matters};
use crate::rules::verify;
use crate::search::Recipe;
use crate::timing::*;
use anyhow::{Result, bail};
use serde::Serialize;
use std::collections::BTreeMap;

/// Time allowed to place each neighbour.
const PLANT_SECONDS: f64 = 15.0;
/// Extra room on both sides of the planting window.
const WINDOW_MARGIN: f64 = 30.0;
/// Planter positions, with the centre in the middle. Sides are closest, so they fill first.
pub const NEIGHBOUR_SLOTS: [u8; 8] = [1, 3, 5, 7, 0, 2, 6, 8];
pub const CENTRE_SLOT: u8 = 4;
/// Clones fill a box along each row, top to bottom, then move on to the next box.
pub const BOX_COLUMNS: u32 = 6;
pub const BOX_ROWS: u32 = 8;

/// Where a clone sits in storage. Every field counts from 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Place {
    pub number: u32,
    #[serde(rename = "box")]
    pub storage_box: u32,
    pub row: u32,
    pub column: u32,
}

impl Place {
    pub fn of(number: u32) -> Self {
        let index = number - 1;
        let per_box = BOX_COLUMNS * BOX_ROWS;
        let within = index % per_box;
        Place { number, storage_box: index / per_box + 1, row: within / BOX_COLUMNS + 1, column: within % BOX_COLUMNS + 1 }
    }
}

/// Every place each plant was scanned at.
pub type Places = BTreeMap<Genotype, Vec<Place>>;

pub fn places_of(scanned: &[Option<Genotype>]) -> Places {
    let mut places = Places::new();
    for (index, plant) in scanned.iter().enumerate() {
        if let Some(plant) = plant {
            places.entry(*plant).or_default().push(Place::of(index as u32 + 1));
        }
    }
    places
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CueKind {
    Prepare,
    PlantCentre,
    Wait,
    PlantNeighbour,
    Check,
    Clone,
    Multiply,
    Finish,
}

/// One instruction. Cues run in order within a crossing.
#[derive(Clone, Debug, Serialize)]
pub struct Cue {
    pub kind: CueKind,
    /// Crossing number, 0 before the first.
    pub step: usize,
    pub headline: String,
    pub detail: String,
    /// What to look for in game before moving on. Times only estimate when this happens.
    pub condition: Option<String>,
    pub plants: Vec<String>,
    /// Where to find this plant in storage. Empty for plants the guide has you make.
    pub places: Vec<Place>,
    /// Planter position for this plant, 0 to 8.
    pub slot: Option<u8>,
    /// The neighbours to plant, in planting order, for a neighbour cue.
    pub neighbours: Vec<Neighbour>,
    /// Seconds after the centre was planted that this cue waits for.
    pub wait_until: Option<u32>,
    /// Latest safe finish, in seconds after the centre was planted.
    pub deadline: Option<u32>,
    /// A timer that starts when this cue is confirmed, for waits with no centre.
    pub timer_seconds: Option<u32>,
}

impl Cue {
    fn new(kind: CueKind, step: usize, headline: String, detail: String) -> Self {
        Cue {
            kind,
            step,
            headline,
            detail,
            condition: None,
            plants: Vec::new(),
            places: Vec::new(),
            slot: None,
            neighbours: Vec::new(),
            wait_until: None,
            deadline: None,
            timer_seconds: None,
        }
    }
}

/// One neighbour within a neighbour cue.
#[derive(Clone, Debug, Serialize)]
pub struct Neighbour {
    /// Planting order, from 1.
    pub number: u32,
    pub genes: String,
    /// Where to find it in storage. Empty for plants the guide has you make.
    pub places: Vec<Place>,
    /// Planter position, 0 to 8.
    pub slot: u8,
}

#[derive(Clone, Debug, Serialize)]
pub struct Need {
    pub genes: String,
    pub count: u32,
    pub starting_plant: bool,
    pub places: Vec<Place>,
}

/// When one crossing's neighbours go in. The ticks are what the guide goes by, because the centre's
/// stage shows them in game. The seconds only estimate them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Window {
    /// Start planting once the centre has grown this many ticks...
    pub first_tick: u32,
    /// ...and no later than this many.
    pub last_tick: u32,
    /// Estimates in seconds after the centre was planted.
    pub earliest: u32,
    pub target: u32,
    pub latest: u32,
    pub crossbreed: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Guide {
    pub crop: &'static str,
    pub quality: f32,
    pub cues: Vec<Cue>,
    pub needs: Vec<Need>,
    /// Working one crossing at a time.
    pub total_seconds: u32,
    /// With a free planter for every crossing that can run at once.
    pub parallel_seconds: u32,
    pub windows: Vec<Window>,
    pub order_steps: usize,
    pub notes: Vec<String>,
}

fn clock(seconds: f64) -> String {
    let total = seconds.round() as u32;
    format!("{}m {:02}s", total / 60, total % 60)
}

fn seconds(ticks: u32) -> f64 {
    (ticks * TICK_SECONDS) as f64
}

/// Neighbours must go in before the centre crossbreeds, but late enough that none of them
/// crossbreeds first, because a neighbour that has crossbred counts with its changed genes.
///
/// The window is counted in the centre's growth ticks, because the player sees those as its stage
/// and progress. Seeing a tick says nothing about how long ago it landed, so with `left` ticks to
/// go the centre crossbreeds somewhere between `left - 1` fast ticks and `left` slow ticks away.
/// Both limits assume every tick lands at the unluckiest end of its jitter.
pub fn planting_window(centre_ticks: u32, neighbour_ticks: &[u32]) -> Result<Window> {
    let fast = 1.0 - TICK_JITTER;
    let slow = 1.0 + TICK_JITTER;
    let quickest = neighbour_ticks.iter().copied().min().unwrap_or(centre_ticks);
    let planting = PLANT_SECONDS * neighbour_ticks.len() as f64;
    // The first neighbour must not reach Crossbreed before the centre does.
    let late_enough = |left: u32| slow * seconds(left) + WINDOW_MARGIN < fast * seconds(quickest);
    // Every neighbour must be in before the centre does.
    let early_enough = |left: u32| left >= 1 && fast * seconds(left - 1) >= planting + WINDOW_MARGIN;
    let safe: Vec<u32> =
        (0..centre_ticks).filter(|&grown| late_enough(centre_ticks - grown) && early_enough(centre_ticks - grown)).collect();
    let (Some(&first_tick), Some(&last_tick)) = (safe.first(), safe.last()) else {
        bail!("no safe planting window: the neighbours grow too fast for this centre");
    };
    // The estimate keeps to times that are safe by the clock alone, when there are any, and to
    // times when the centre is usually inside the tick window.
    let clock_earliest = slow * seconds(centre_ticks) - fast * seconds(quickest) + WINDOW_MARGIN;
    let clock_latest = fast * seconds(centre_ticks) - WINDOW_MARGIN - planting;
    let (mut earliest, mut latest) = (seconds(first_tick), seconds(last_tick + 1) - 1.0);
    if clock_earliest.max(earliest) <= clock_latest.min(latest) {
        (earliest, latest) = (clock_earliest.max(earliest), clock_latest.min(latest));
    }
    Ok(Window {
        first_tick,
        last_tick,
        earliest: earliest as u32,
        target: (((earliest + latest) / 2.0 / 5.0).round() * 5.0) as u32,
        latest: latest as u32,
        crossbreed: (slow * seconds(centre_ticks)).ceil() as u32,
    })
}

/// A stage and progress as the game shows it, such as "Sapling 40%". `up` rounds the percentage
/// up instead of down, so a limit holds whichever way the game rounds.
fn stage_text(crop: &Crop, g_genes: u32, conditions: Conditions, ticks: u32, up: bool) -> String {
    let (stage, done) = stage_at(crop, g_genes, conditions, ticks);
    let percent = done * 100.0;
    // The small nudge keeps float error such as 20.000002 from rounding to the wrong whole number.
    let percent = if up { (percent - 1e-3).ceil() } else { (percent + 1e-3).floor() };
    format!("{} {percent:.0}%", stage.name())
}

fn count(g: Genotype, gene: u8) -> u32 {
    g.genes().iter().filter(|&&slot| slot == gene).count() as u32
}

/// Clones one plant gives. Taking them kills the plant.
pub fn clones_from(crop: &Crop, g: Genotype) -> u32 {
    crop.base_clones + count(g, Y) / 2
}

/// "Any plant" centres still need a real plant, and the fastest grower saves the most time.
fn resolve_centre(step: &Recipe, inputs: &[Genotype]) -> Result<Genotype> {
    if let Some(centre) = step.centre {
        return Ok(centre);
    }
    match inputs.iter().copied().max_by_key(|&g| count(g, G)) {
        Some(plant) if verify(&step.order, Some(plant), step.result) => Ok(plant),
        _ => bail!("no starting plant can stand in as the centre for {}", step.result),
    }
}

/// How many clones of each genotype the crossings use up.
fn demand(steps: &[(Recipe, Genotype)]) -> BTreeMap<Genotype, u32> {
    let mut wanted = BTreeMap::new();
    for (step, centre) in steps {
        for plant in step.order.iter().chain(std::iter::once(centre)) {
            *wanted.entry(*plant).or_insert(0) += 1;
        }
    }
    wanted
}

/// Rounds of replanting needed to turn `have` clones into `need`, each clone giving `per`.
fn multiply_rounds(have: u32, need: u32, per: u32) -> Option<u32> {
    if have >= need {
        return Some(0);
    }
    if per < 2 || have == 0 {
        return None;
    }
    let mut held = have;
    let mut rounds = 0;
    while held < need {
        held *= per;
        rounds += 1;
    }
    Some(rounds)
}

pub fn build(plan: &Plan, inputs: &[Genotype], places: &Places, crop: &Crop, conditions: Conditions) -> Result<Guide> {
    let steps: Vec<(Recipe, Genotype)> = plan.steps.iter().map(|s| Ok((s.clone(), resolve_centre(s, inputs)?))).collect::<Result<_>>()?;
    let wanted = demand(&steps);
    let timeline = |g: Genotype| Timeline::new(crop, count(g, G), conditions);
    let quality = conditions.quality(crop);
    let mut cues = Vec::new();
    let mut windows = Vec::new();
    let mut total = 0.0;
    let mut finished: BTreeMap<Genotype, f64> = BTreeMap::new();

    let made_here = |g: &Genotype| plan.steps.iter().any(|s| s.result == *g);
    let stored = |g: &Genotype| if made_here(g) { Vec::new() } else { places.get(g).cloned().unwrap_or_default() };
    let needs: Vec<Need> =
        wanted.iter().map(|(g, n)| Need { genes: g.to_string(), count: *n, starting_plant: !made_here(g), places: stored(g) }).collect();
    let gather: Vec<String> = needs.iter().filter(|n| n.starting_plant).map(|n| format!("{} ×{}", n.genes, n.count)).collect();
    let mut prepare = Cue::new(
        CueKind::Prepare,
        0,
        "Gather your clones".into(),
        format!(
            "You need {}. Bring {}, and use an empty planter. Each step says what to look for on the plant before you go on. \
             Its percentages and the timer assume {} with {:.0}% health, so check the planter shows that. The timer is only an estimate.",
            gather.join(", "),
            if conditions.fertilized { "water and fertiliser" } else { "water" },
            if conditions.fertilized { "a fertilised planter" } else { "a planter without fertiliser" },
            quality * 100.0
        ),
    );
    prepare.plants = needs.iter().filter(|n| n.starting_plant).map(|n| n.genes.clone()).collect();
    cues.push(prepare);

    for (index, (step, centre)) in steps.iter().enumerate() {
        let number = index + 1;
        let centre_time = timeline(*centre)?;
        let neighbour_ticks: Vec<u32> = step.order.iter().map(|&g| Ok(timeline(g)?.to_crossbreed())).collect::<Result<_>>()?;
        let window = planting_window(centre_time.to_crossbreed(), &neighbour_ticks)?;
        windows.push(window);
        let order_note = if order_matters(step) { "Planting order matters here." } else { "Any order works here." };

        let detail = "Remove every plant of this crop from the planter, and water it to about 60%, not full. Then plant this in the middle. The estimate starts when you confirm.";
        let mut plant = Cue::new(CueKind::PlantCentre, number, "Plant the centre".into(), detail.into());
        plant.plants = vec![centre.to_string()];
        plant.places = stored(centre);
        plant.slot = Some(CENTRE_SLOT);
        cues.push(plant);

        let centre_g = count(*centre, G);
        let from = stage_text(crop, centre_g, conditions, window.first_tick, false);
        let to = stage_text(crop, centre_g, conditions, window.last_tick, true);
        let mut wait = Cue::new(
            CueKind::Wait,
            number,
            "Wait to plant the neighbours".into(),
            format!(
                "Look at the centre plant. Too early and a neighbour crossbreeds first, too late and they miss it. \
                 That's roughly {} to {} after the centre went in. {order_note}",
                clock(window.earliest as f64),
                clock(window.latest as f64)
            ),
        );
        wait.condition = Some(if window.first_tick == 0 {
            format!("Plant them straight away, before the centre passes {to}")
        } else {
            format!("Plant them once the centre shows {from}, and no later than {to}")
        });
        wait.wait_until = Some(window.target);
        cues.push(wait);

        // All of a crossing's neighbours share one cue, since they go in together with the same
        // deadline. The numbers give the planting order and match the planter picture.
        let count = step.order.len();
        let (headline, how) = match count {
            1 => ("Plant the neighbour".to_string(), "Put it in planter spot 1.".to_string()),
            n if order_matters(step) => (
                format!("Plant the {n} neighbours"),
                format!("Plant them in this order, 1 first, each in its numbered planter spot. {order_note}"),
            ),
            n => (format!("Plant the {n} neighbours"), format!("Put each in its numbered planter spot. {order_note}")),
        };
        let mut cue = Cue::new(CueKind::PlantNeighbour, number, headline, how);
        cue.condition = Some("Every neighbour must be in before the centre reaches Crossbreed".into());
        cue.plants = step.order.iter().map(Genotype::to_string).collect();
        cue.neighbours = step
            .order
            .iter()
            .enumerate()
            .map(|(i, g)| Neighbour { number: i as u32 + 1, genes: g.to_string(), places: stored(g), slot: NEIGHBOUR_SLOTS[i] })
            .collect();
        cue.deadline = Some(window.latest + PLANT_SECONDS as u32 * count as u32);
        cues.push(cue);

        let ready = (seconds(centre_time.to_mature()) * (1.0 + TICK_JITTER)).ceil();
        let mut wait = Cue::new(
            CueKind::Wait,
            number,
            "Wait for the crossbreed".into(),
            "Its genes change as it enters Crossbreed, and clones can be taken again once it's Mature. Keep the planter watered, lit and warm. Avoid server restarts, and leave the plants alone.".into(),
        );
        wait.condition = Some("The centre shows Mature".into());
        wait.wait_until = Some(ready as u32);
        cues.push(wait);

        let mut check = Cue::new(
            CueKind::Check,
            number,
            "Check the centre plant".into(),
            "It must show exactly these genes. Don't go on if it doesn't.".into(),
        );
        check.plants = vec![step.result.to_string()];
        check.slot = Some(CENTRE_SLOT);
        cues.push(check);

        let yielded = clones_from(crop, step.result);
        let later = wanted.get(&step.result).copied().unwrap_or(0);
        let mut clone =
            Cue::new(CueKind::Clone, number, "Take clones from the centre".into(), format!("This gives {yielded} and removes the plant."));
        clone.plants = vec![step.result.to_string()];
        cues.push(clone);
        total += ready;

        let parents = step.order.iter().chain(std::iter::once(centre));
        let start = parents.filter_map(|p| finished.get(p)).copied().fold(0.0, f64::max);
        let mut done = start + ready;

        if step.result != plan.goal && later > yielded {
            let Some(rounds) = multiply_rounds(yielded, later, yielded) else {
                bail!("{} gives only {yielded} clone per plant, so it can't reach the {later} this plan needs", step.result);
            };
            let round_seconds = (seconds(timeline(step.result)?.to_sapling()) * (1.0 + TICK_JITTER)).ceil() as u32;
            let mut held = yielded;
            for _ in 0..rounds {
                let gain = held * yielded;
                let detail = format!(
                    "You need {later} in total. Plant all {held}, confirm, and take clones from each once they reach Sapling, before they crossbreed."
                );
                let mut cue = Cue::new(CueKind::Multiply, number, format!("Multiply to {gain} clones"), detail);
                cue.condition = Some("Every plant shows Sapling. Take the clones before any reaches Crossbreed".into());
                cue.plants = vec![step.result.to_string()];
                cue.timer_seconds = Some(round_seconds);
                cues.push(cue);
                total += round_seconds as f64;
                done += round_seconds as f64;
                held = gain;
            }
        }
        finished.insert(step.result, done);
    }

    let mut finish = Cue::new(
        CueKind::Finish,
        plan.steps.len(),
        format!("{} is ready", plan.goal),
        "Keep a clone safe before you do anything else with it.".into(),
    );
    finish.plants = vec![plan.goal.to_string()];
    cues.push(finish);

    let mut notes = Vec::new();
    if !conditions.fertilized {
        notes.push(format!("Without fertiliser, {} grows at {:.0}% speed in a planter.", crop.name, quality * 100.0));
    }
    let wet_low = 0.6 * crop.optimal_water * 100.0;
    let wet_high = (1.6 - crop.optimal_water) * 100.0;
    notes.push(format!(
        "Water is at full quality between {wet_low:.0}% and {wet_high:.0}% planter saturation. A full planter grows slower."
    ));
    let order_steps = plan.steps.iter().filter(|s| order_matters(s)).count();
    let parallel = finished.values().copied().fold(0.0, f64::max);
    Ok(Guide {
        crop: crop.name,
        quality,
        cues,
        needs,
        total_seconds: total as u32,
        parallel_seconds: parallel as u32,
        windows,
        order_steps,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{PlanConfig, run};
    use crate::search::CpuBackend;

    fn genotype(text: &str) -> Genotype {
        Genotype::parse(text).unwrap()
    }

    #[test]
    fn places_fill_each_box_row_by_row() {
        let place = |n| {
            let p = Place::of(n);
            (p.storage_box, p.row, p.column)
        };
        assert_eq!(place(1), (1, 1, 1));
        assert_eq!(place(3), (1, 1, 3));
        assert_eq!(place(6), (1, 1, 6));
        assert_eq!(place(7), (1, 2, 1));
        assert_eq!(place(48), (1, 8, 6));
        assert_eq!(place(49), (2, 1, 1));
        assert_eq!(place(50), (2, 1, 2));
        assert_eq!(place(97), (3, 1, 1));
    }

    #[test]
    fn repeats_and_gaps_keep_real_positions() {
        let (a, b) = (genotype("GGGHHH"), genotype("GGGYHH"));
        let places = places_of(&[Some(a), None, Some(b), Some(a)]);
        let numbers = |g: Genotype| places[&g].iter().map(|p| p.number).collect::<Vec<_>>();
        assert_eq!(numbers(a), vec![1, 4]);
        assert_eq!(numbers(b), vec![3]);
    }

    #[test]
    fn only_stored_plants_get_a_place() {
        let (a, b, made, goal) = (genotype("GGGHHH"), genotype("GGGYHH"), genotype("GGHHHH"), genotype("GGGGHH"));
        let steps =
            vec![Recipe { result: made, order: vec![a], centre: Some(b) }, Recipe { result: goal, order: vec![made], centre: Some(b) }];
        let plan = Plan { goal, generation: 2, steps, inputs_used: vec![a, b] };
        let places = places_of(&[Some(a), None, Some(b)]);
        let guide = build(&plan, &[a, b], &places, crop_by_key("hemp").unwrap(), Conditions::PERFECT).unwrap();
        // Every planted plant with where the guide says to find it, centres and neighbours alike.
        let mut planted: Vec<(String, Vec<Place>)> = Vec::new();
        for cue in &guide.cues {
            match cue.kind {
                CueKind::PlantCentre => planted.push((cue.plants[0].clone(), cue.places.clone())),
                CueKind::PlantNeighbour => planted.extend(cue.neighbours.iter().map(|n| (n.genes.clone(), n.places.clone()))),
                _ => {}
            }
        }
        assert_eq!(planted.len(), 4);
        for (genes, places) in planted {
            let want: Vec<u32> = match genes.as_str() {
                p if p == a.to_string() => vec![1],
                p if p == b.to_string() => vec![3],
                _ => vec![],
            };
            assert_eq!(places.iter().map(|p| p.number).collect::<Vec<_>>(), want);
        }
        let need = |g: Genotype| guide.needs.iter().find(|n| n.genes == g.to_string()).unwrap();
        assert_eq!(need(b).places[0].number, 3);
        assert!(need(made).places.is_empty());
    }

    #[test]
    fn a_window_keeps_slow_centres_ahead_of_fast_neighbours() {
        // A 0G centre takes 24 ticks, a 6G neighbour 11.
        // With 8 ticks to go the centre needs at most 528 s, plus 30 s of margin, against the
        // neighbour's quickest 594 s. With 9 to go it could need 594 s, which is too close.
        let w = planting_window(24, &[11]).unwrap();
        assert_eq!(w.first_tick, 16);
        // The single neighbour needs 15 s and 30 s of margin, so 2 ticks must still be to come.
        assert_eq!(w.last_tick, 22);
        assert!(w.earliest < w.latest);
        assert!(w.target >= w.earliest && w.target <= w.latest);
        assert!(((w.latest as f64) + 15.0) < 0.9 * 24.0 * 60.0);
    }

    #[test]
    fn every_gene_pairing_has_a_window() {
        let ticks = [24, 19, 15, 13, 13, 11, 11];
        for &centre in &ticks {
            for &neighbour in &ticks {
                planting_window(centre, &[neighbour; 8]).unwrap();
            }
        }
    }

    /// Checks the window's own promise for every pairing: planting at any tick inside it, at the
    /// unluckiest jitter, gets every neighbour in before the centre crossbreeds and none crossbreeds first.
    #[test]
    fn every_tick_in_a_window_is_safe() {
        let ticks = [24, 19, 15, 13, 13, 11, 11, 36, 28];
        for &centre in &ticks {
            for &neighbour in &ticks {
                for count in 1..=8 {
                    let w = planting_window(centre, &vec![neighbour; count]).unwrap();
                    assert!(w.first_tick <= w.last_tick && w.last_tick < centre);
                    for grown in w.first_tick..=w.last_tick {
                        let left = (centre - grown) as f64;
                        let centre_soonest = 0.9 * 60.0 * (left - 1.0);
                        let centre_latest = 1.1 * 60.0 * left;
                        assert!(centre_soonest >= 15.0 * count as f64, "{centre} {neighbour} {count} {grown}");
                        assert!(0.9 * 60.0 * neighbour as f64 > centre_latest, "{centre} {neighbour} {count} {grown}");
                    }
                }
            }
        }
    }

    #[test]
    fn waits_name_what_to_look_for() {
        let (a, b, made, goal) = (genotype("GGGHHH"), genotype("GGGYHH"), genotype("GGHHHH"), genotype("GGGGHH"));
        let steps =
            vec![Recipe { result: made, order: vec![a], centre: Some(b) }, Recipe { result: goal, order: vec![made], centre: Some(b) }];
        let plan = Plan { goal, generation: 2, steps, inputs_used: vec![a, b] };
        let plain = Conditions { fertilized: false, other_quality: 1.0 };
        let guide = build(&plan, &[a, b], &Places::new(), crop_by_key("hemp").unwrap(), plain).unwrap();
        for cue in guide.cues.iter().filter(|c| matches!(c.kind, CueKind::Wait | CueKind::Multiply | CueKind::PlantNeighbour)) {
            assert!(cue.condition.is_some(), "{} has no condition", cue.headline);
            println!("{}: {}", cue.headline, cue.condition.as_ref().unwrap());
        }
        let first = guide.cues.iter().find(|c| c.kind == CueKind::Wait).unwrap();
        let text = first.condition.as_ref().unwrap();
        assert!(text.contains('%') && (text.contains("Seedling") || text.contains("Sapling")), "{text}");
    }

    #[test]
    fn clones_follow_the_y_rule() {
        let hemp = crop_by_key("hemp").unwrap();
        let orchid = crop_by_key("orchid").unwrap();
        assert_eq!(clones_from(hemp, genotype("GGGGGG")), 2);
        assert_eq!(clones_from(hemp, genotype("GGGYYY")), 3);
        assert_eq!(clones_from(hemp, genotype("YYYYYY")), 5);
        assert_eq!(clones_from(orchid, genotype("GGGGGY")), 1);
    }

    #[test]
    fn multiplying_counts_rounds() {
        assert_eq!(multiply_rounds(3, 3, 3), Some(0));
        assert_eq!(multiply_rounds(2, 3, 2), Some(1));
        assert_eq!(multiply_rounds(2, 9, 2), Some(3));
        assert_eq!(multiply_rounds(1, 2, 1), None);
    }

    /// Recipes here are made up, because only the clone arithmetic is under test.
    #[test]
    fn short_supply_adds_multiply_rounds() {
        let (a, b, made, goal) = (genotype("GGGHHH"), genotype("GGGYHH"), genotype("GGHHHH"), genotype("GGGGHH"));
        let steps =
            vec![Recipe { result: made, order: vec![a], centre: Some(b) }, Recipe { result: goal, order: vec![made; 5], centre: Some(b) }];
        let plan = Plan { goal, generation: 2, steps, inputs_used: vec![a, b] };
        let guide = build(&plan, &[a, b], &Places::new(), crop_by_key("hemp").unwrap(), Conditions::PERFECT).unwrap();
        let rounds: Vec<&Cue> = guide.cues.iter().filter(|c| c.kind == CueKind::Multiply).collect();
        assert_eq!(rounds.len(), 2, "2 clones become 4, then 8, to cover the 5 needed");
        assert!(rounds.iter().all(|c| c.step == 1 && c.timer_seconds.is_some()));
        assert!(rounds.iter().all(|c| c.plants == vec![made.to_string()]));
        let need = |g: Genotype| guide.needs.iter().find(|n| n.genes == g.to_string()).unwrap().count;
        assert_eq!(need(made), 5);
        assert_eq!(need(b), 2);
    }

    #[test]
    fn a_single_clone_crop_refuses_to_multiply() {
        let (a, b, made, goal) = (genotype("GGGHHH"), genotype("GGGYHH"), genotype("GGHHHH"), genotype("GGGGHH"));
        let steps =
            vec![Recipe { result: made, order: vec![a], centre: Some(b) }, Recipe { result: goal, order: vec![made; 2], centre: Some(b) }];
        let plan = Plan { goal, generation: 2, steps, inputs_used: vec![a, b] };
        assert!(build(&plan, &[a, b], &Places::new(), crop_by_key("orchid").unwrap(), Conditions::PERFECT).is_err());
    }

    #[test]
    fn a_guide_covers_every_step_in_order() {
        let inputs = parse_list("GGGYYH XGHGWY HGGGYH YGHWGX WGGYYY").unwrap();
        let cfg = PlanConfig { max_crossings: 2, max_plants: 3, goal: None, time_limit: None };
        let outcome = run(&inputs, &cfg, &mut CpuBackend, |_| {}).unwrap();
        let goal = outcome.reached.keys().copied().max_by_key(|&g| outcome.plan(g).unwrap().steps.len()).unwrap();
        let plan = outcome.plan(goal).unwrap();
        assert!(plan.steps.len() >= 2);
        plan.check(&outcome.inputs).unwrap();
        let guide = build(&plan, &outcome.inputs, &Places::new(), crop_by_key("hemp").unwrap(), Conditions::PERFECT).unwrap();

        assert_eq!(guide.cues.first().unwrap().kind, CueKind::Prepare);
        assert_eq!(guide.cues.last().unwrap().kind, CueKind::Finish);
        assert_eq!(guide.windows.len(), plan.steps.len());
        for (index, step) in plan.steps.iter().enumerate() {
            let cues: Vec<&Cue> = guide.cues.iter().filter(|c| c.step == index + 1 && c.kind == CueKind::PlantNeighbour).collect();
            assert_eq!(cues.len(), 1, "a crossing's neighbours share one cue");
            let neighbours = &cues[0].neighbours;
            let planted: Vec<String> = neighbours.iter().map(|n| n.genes.clone()).collect();
            let wanted: Vec<String> = step.order.iter().map(Genotype::to_string).collect();
            assert_eq!(planted, wanted, "neighbours must keep the verified order");
            assert_eq!(neighbours.iter().map(|n| n.number).collect::<Vec<_>>(), (1..=neighbours.len() as u32).collect::<Vec<_>>());
            let mut slots: Vec<u8> = neighbours.iter().map(|n| n.slot).chain([CENTRE_SLOT]).collect();
            slots.sort();
            slots.dedup();
            assert_eq!(slots.len(), neighbours.len() + 1, "every plant needs its own planter spot");
        }
        assert!(guide.parallel_seconds <= guide.total_seconds);
    }
}
