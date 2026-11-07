//! Real grow times, from each crop's `plantproperty` asset and the server's growth tick.
//!
//! Plants only advance on a tick of `server.planttick` seconds, 60 by default, and leftover
//! progress is thrown away when a stage ends. Times are therefore whole ticks, not a smooth
//! curve: 3G and 4G plants reach Crossbreed together, as do 5G and 6G.

use anyhow::{Result, bail};
use serde::Serialize;

pub const TICK_SECONDS: u32 = 60;
/// Each tick lands up to this fraction early or late.
pub const TICK_JITTER: f64 = 0.1;

/// Seed, Seedling and Sapling are the same length for every crop.
const SEED: f32 = 1.0;
const SEEDLING: f32 = 10.0;
const SAPLING: f32 = 10.0;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Crop {
    pub key: &'static str,
    pub name: &'static str,
    /// Stage lengths in minutes. Crossbreed ignores conditions and G genes.
    pub crossbreed: f32,
    pub mature: f32,
    pub fruiting: f32,
    /// Planters reach this ground quality only with fertiliser.
    pub optimal_ground: f32,
    /// Sets the range of planter saturation that counts as fully watered.
    pub optimal_water: f32,
    /// Clones from one plant before Y genes are added.
    pub base_clones: u32,
}

const fn crop(
    key: &'static str,
    name: &'static str,
    crossbreed: f32,
    mature: f32,
    optimal_ground: f32,
    optimal_water: f32,
    base_clones: u32,
) -> Crop {
    Crop { key, name, crossbreed, mature, fruiting: 30.0, optimal_ground, optimal_water, base_clones }
}

/// Read from build 26431843 on 2026-11-06.
pub const CROPS: [Crop; 14] = [
    crop("hemp", "Hemp", 1.0, 60.0, 0.9, 0.85, 2),
    crop("corn", "Corn", 1.0, 30.0, 0.9, 0.85, 2),
    crop("pumpkin", "Pumpkin", 1.0, 30.0, 0.9, 0.85, 2),
    crop("potato", "Potato", 1.0, 45.0, 0.7, 0.7, 2),
    crop("wheat", "Wheat", 1.0, 30.0, 0.9, 0.85, 2),
    crop("rose", "Rose", 1.0, 60.0, 0.9, 0.85, 2),
    crop("sunflower", "Sunflower", 1.0, 60.0, 0.9, 0.85, 2),
    crop("orchid", "Orchid", 1.0, 60.0, 0.9, 0.85, 1),
    crop("blue_berry", "Blue berry", 5.0, 45.0, 0.8, 0.8, 2),
    crop("red_berry", "Red berry", 5.0, 45.0, 0.8, 0.8, 2),
    crop("green_berry", "Green berry", 5.0, 45.0, 0.8, 0.8, 2),
    crop("yellow_berry", "Yellow berry", 5.0, 45.0, 0.8, 0.8, 2),
    crop("white_berry", "White berry", 5.0, 45.0, 0.8, 0.8, 2),
    crop("black_berry", "Black berry", 5.0, 45.0, 0.8, 0.8, 2),
];

pub fn crop_by_key(key: &str) -> Result<&'static Crop> {
    match CROPS.iter().find(|c| c.key == key) {
        Some(crop) => Ok(crop),
        None => bail!("unknown crop '{key}'"),
    }
}

/// What the plant is growing in.
#[derive(Clone, Copy, Debug)]
pub struct Conditions {
    /// A planter gives ground quality 0.6, or 1.0 with fertiliser.
    pub fertilized: bool,
    /// The lowest of the light, water and temperature qualities, 0 to 1.
    pub other_quality: f32,
}

impl Conditions {
    pub const PERFECT: Conditions = Conditions { fertilized: true, other_quality: 1.0 };

    /// The game takes the worst of the four qualities.
    pub fn quality(self, crop: &Crop) -> f32 {
        let ground: f32 = if self.fertilized { 1.0 } else { 0.6 };
        (ground / crop.optimal_ground).min(1.0).min(self.other_quality)
    }
}

/// Ticks until a stage ends: progress must strictly exceed the stage length.
fn stage_ticks(minutes: f32, rate: f32) -> u32 {
    let length = minutes * 60.0;
    let step = TICK_SECONDS as f32 * rate;
    let mut age = 0.0f32;
    let mut ticks = 0;
    while age <= length {
        age += step;
        ticks += 1;
    }
    ticks
}

/// Ticks spent in each stage by a plant with a given number of G genes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Timeline {
    pub seed: u32,
    pub seedling: u32,
    pub sapling: u32,
    pub crossbreed: u32,
    pub mature: u32,
    pub fruiting: u32,
}

impl Timeline {
    pub fn new(crop: &Crop, g_genes: u32, conditions: Conditions) -> Result<Self> {
        let quality = conditions.quality(crop);
        if quality < 0.05 {
            bail!("quality {quality} is too low for this plant to grow");
        }
        let rate = quality * (1.0 + g_genes as f32 * 0.25);
        Ok(Timeline {
            seed: stage_ticks(SEED, rate),
            seedling: stage_ticks(SEEDLING, rate),
            sapling: stage_ticks(SAPLING, rate),
            crossbreed: stage_ticks(crop.crossbreed, 1.0),
            mature: stage_ticks(crop.mature, rate),
            fruiting: stage_ticks(crop.fruiting, rate),
        })
    }

    /// Ticks from planting until clones can first be taken.
    pub fn to_sapling(&self) -> u32 {
        self.seed + self.seedling
    }

    /// Ticks from planting until the plant crossbreeds.
    pub fn to_crossbreed(&self) -> u32 {
        self.to_sapling() + self.sapling
    }

    /// Ticks from planting until a crossbred plant can be cloned.
    pub fn to_mature(&self) -> u32 {
        self.to_crossbreed() + self.crossbreed
    }

    pub fn to_ripe(&self) -> u32 {
        self.to_mature() + self.mature + self.fruiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hemp() -> &'static Crop {
        crop_by_key("hemp").unwrap()
    }

    /// Worked by hand from the tick rule. Stages end when age is strictly above their length.
    #[test]
    fn crossbreed_time_follows_whole_ticks() {
        let expected = [24, 19, 15, 13, 13, 11, 11];
        for (g, minutes) in expected.iter().enumerate() {
            assert_eq!(Timeline::new(hemp(), g as u32, Conditions::PERFECT).unwrap().to_crossbreed(), *minutes, "{g} G");
        }
    }

    #[test]
    fn exact_multiples_need_one_more_tick() {
        assert_eq!(stage_ticks(1.0, 1.0), 2);
        assert_eq!(stage_ticks(10.0, 2.0), 6);
        assert_eq!(stage_ticks(10.0, 2.5), 5);
        assert_eq!(stage_ticks(10.0, 2.25), 5);
    }

    #[test]
    fn crossbreed_stage_ignores_genes_and_conditions() {
        let poor = Conditions { fertilized: false, other_quality: 0.5 };
        assert_eq!(Timeline::new(hemp(), 6, Conditions::PERFECT).unwrap().crossbreed, 2);
        assert_eq!(Timeline::new(hemp(), 0, poor).unwrap().crossbreed, 2);
        assert_eq!(Timeline::new(crop_by_key("blue_berry").unwrap(), 0, poor).unwrap().crossbreed, 6);
    }

    #[test]
    fn fertiliser_matters_differently_per_crop() {
        let plain = Conditions { fertilized: false, other_quality: 1.0 };
        assert!((plain.quality(hemp()) - 2.0 / 3.0).abs() < 1e-5);
        assert!((plain.quality(crop_by_key("blue_berry").unwrap()) - 0.75).abs() < 1e-5);
        assert!((plain.quality(crop_by_key("potato").unwrap()) - 6.0 / 7.0).abs() < 1e-5);
        assert_eq!(Conditions::PERFECT.quality(hemp()), 1.0);
    }

    #[test]
    fn the_worst_quality_sets_the_pace() {
        let dim = Conditions { fertilized: true, other_quality: 0.5 };
        assert_eq!(dim.quality(hemp()), 0.5);
        assert!(Timeline::new(hemp(), 0, dim).unwrap().to_crossbreed() > 24);
        assert!(Timeline::new(hemp(), 0, Conditions { fertilized: true, other_quality: 0.0 }).is_err());
    }
}
