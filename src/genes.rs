//! Genes and six-slot genotypes.

use anyhow::{bail, Result};
use std::fmt;

pub const SLOTS: usize = 6;
pub const GENE_KINDS: usize = 5;
/// 5^6: every genotype that can exist.
pub const GENOTYPES: usize = 15_625;

/// Gene ids, used everywhere as array indexes.
pub const G: u8 = 0;
pub const H: u8 = 1;
pub const Y: u8 = 2;
pub const W: u8 = 3;
pub const X: u8 = 4;

const LETTERS: [u8; GENE_KINDS] = *b"GHYWX";

/// A genotype stored as a base-5 number, slot 0 being the most significant digit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Genotype(pub u16);

impl Genotype {
    pub fn from_genes(genes: &[u8; SLOTS]) -> Self {
        Genotype(genes.iter().fold(0u16, |acc, &g| acc * 5 + g as u16))
    }

    pub fn genes(self) -> [u8; SLOTS] {
        let mut v = self.0;
        let mut out = [0u8; SLOTS];
        for slot in (0..SLOTS).rev() {
            out[slot] = (v % 5) as u8;
            v /= 5;
        }
        out
    }

    pub fn parse(text: &str) -> Result<Self> {
        let cleaned: Vec<char> = text.chars().filter(|c| !matches!(c, '-' | ' ')).collect();
        if cleaned.len() != SLOTS {
            bail!("genotype '{text}' must have exactly {SLOTS} genes");
        }
        let mut genes = [0u8; SLOTS];
        for (slot, c) in cleaned.iter().enumerate() {
            let upper = c.to_ascii_uppercase() as u8;
            match LETTERS.iter().position(|&l| l == upper) {
                Some(gene) => genes[slot] = gene as u8,
                None => bail!("'{c}' in '{text}' is not one of G, H, Y, W, X"),
            }
        }
        Ok(Self::from_genes(&genes))
    }

    pub fn score(self, scores: &Scores) -> i32 {
        self.genes().iter().map(|&g| scores.0[g as usize]).sum()
    }
}

impl fmt::Display for Genotype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for g in self.genes() {
            write!(f, "{}", LETTERS[g as usize] as char)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Genotype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Per-gene value in hundredths, kept integral so comparisons are exact.
#[derive(Clone, Copy, Debug)]
pub struct Scores(pub [i32; GENE_KINDS]);

impl Default for Scores {
    /// The website's defaults: G 1, H 0.5, Y 1, W 0, X 0.
    fn default() -> Self {
        Scores([100, 50, 100, 0, 0])
    }
}

/// Parses one whitespace or comma separated list of genotypes.
pub fn parse_list(text: &str) -> Result<Vec<Genotype>> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .map(Genotype::parse)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_genotype_round_trips() {
        for i in 0..GENOTYPES as u16 {
            let g = Genotype(i);
            assert_eq!(Genotype::from_genes(&g.genes()), g);
            assert_eq!(Genotype::parse(&g.to_string()).unwrap(), g);
        }
    }

    #[test]
    fn parsing_accepts_site_formatting_and_rejects_junk() {
        assert_eq!(Genotype::parse("w-g-x-y-y-w").unwrap(), Genotype::parse("WGXYYW").unwrap());
        assert!(Genotype::parse("WGXYY").is_err());
        assert!(Genotype::parse("WGXYYQ").is_err());
    }
}
