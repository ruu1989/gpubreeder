//! Genes and six-slot genotypes.

use anyhow::{Result, bail};
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

/// How many of each gene a plant has, in any order. Slot position has no effect in the game.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GeneMix(pub [u8; GENE_KINDS]);

impl GeneMix {
    /// Reads forms like `3G3Y` or `2G2Y2H`.
    pub fn parse(text: &str) -> Result<Self> {
        let mut counts = [0u8; GENE_KINDS];
        let mut chars = text.chars().filter(|c| !matches!(c, ' ' | '-' | '+'));
        while let Some(digit) = chars.next() {
            let Some(count) = digit.to_digit(10) else {
                bail!("expected a number at '{digit}' in '{text}', as in 3G3Y");
            };
            let Some(letter) = chars.next() else {
                bail!("'{text}' ends with a number, as in 3G3Y a gene follows each number");
            };
            let upper = letter.to_ascii_uppercase() as u8;
            let Some(gene) = LETTERS.iter().position(|&l| l == upper) else {
                bail!("'{letter}' in '{text}' is not one of G, H, Y, W, X");
            };
            if counts[gene] != 0 {
                bail!("'{text}' names {} twice", letter.to_ascii_uppercase());
            }
            counts[gene] = count as u8;
        }
        let total: u8 = counts.iter().sum();
        if total as usize != SLOTS {
            bail!("'{text}' adds up to {total} genes, a plant has {SLOTS}");
        }
        Ok(GeneMix(counts))
    }

    pub fn matches(self, g: Genotype) -> bool {
        let mut counts = [0u8; GENE_KINDS];
        for gene in g.genes() {
            counts[gene as usize] += 1;
        }
        counts == self.0
    }
}

impl fmt::Display for GeneMix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (gene, &count) in self.0.iter().enumerate() {
            if count > 0 {
                write!(f, "{count}{}", LETTERS[gene] as char)?;
            }
        }
        Ok(())
    }
}

/// Plants found in pasted text, plus whatever could not be read as a plant.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pasted {
    pub plants: Vec<Genotype>,
    pub ignored: Vec<String>,
    /// Every position in scan order, repeats included. `None` is a place with no readable plant.
    pub scanned: Vec<Option<Genotype>>,
}

/// Reads plants out of anything a scanner or a person might paste. Letters may be separated by
/// spaces or dashes, and several plants may run together, so only whole sixes count.
/// A line holding only a dash keeps a place for a clone that was never scanned.
pub fn parse_pasted(text: &str) -> Pasted {
    let upper = text.to_ascii_uppercase();
    let breaks = |c: char| !matches!(c, 'G' | 'H' | 'Y' | 'W' | 'X' | ' ' | '-');
    let mut pasted = Pasted::default();
    for run in upper.split(breaks) {
        if run.trim() == "-" {
            pasted.scanned.push(None);
            continue;
        }
        let letters: String = run.chars().filter(char::is_ascii_alphabetic).collect();
        if letters.is_empty() {
            continue;
        }
        if letters.len() % SLOTS != 0 {
            pasted.ignored.push(run.trim().to_string());
            // A misread clone still sits in a slot, so the ones after it keep their numbers.
            pasted.scanned.resize(pasted.scanned.len() + letters.len().div_ceil(SLOTS), None);
            continue;
        }
        for chunk in letters.as_bytes().chunks(SLOTS) {
            let genes = chunk.iter().map(|&b| LETTERS.iter().position(|&l| l == b).unwrap_or(0) as u8);
            let mut slots = [0u8; SLOTS];
            slots.iter_mut().zip(genes).for_each(|(slot, gene)| *slot = gene);
            let plant = Genotype::from_genes(&slots);
            pasted.scanned.push(Some(plant));
            if !pasted.plants.contains(&plant) {
                pasted.plants.push(plant);
            }
        }
    }
    pasted
}

/// Parses one whitespace or comma separated list of genotypes.
pub fn parse_list(text: &str) -> Result<Vec<Genotype>> {
    text.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).map(Genotype::parse).collect()
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
    fn a_mix_ignores_slot_order() {
        let mix = GeneMix::parse("3g3y").unwrap();
        assert!(mix.matches(Genotype::parse("GGGYYY").unwrap()));
        assert!(mix.matches(Genotype::parse("GYGYGY").unwrap()));
        assert!(mix.matches(Genotype::parse("YYGGYG").unwrap()));
        assert!(!mix.matches(Genotype::parse("GGGGYY").unwrap()));
        assert!(!mix.matches(Genotype::parse("GGGYYH").unwrap()));
        assert_eq!((0..GENOTYPES as u16).filter(|&i| mix.matches(Genotype(i))).count(), 20);
        assert_eq!(mix.to_string(), "3G3Y");
    }

    #[test]
    fn a_mix_must_describe_six_genes() {
        for bad in ["3G2Y", "4G3Y", "3G3Q", "GGGYYY", "3G3", "3G3G", ""] {
            assert!(GeneMix::parse(bad).is_err(), "{bad}");
        }
        assert!(GeneMix::parse("2G2Y2H").is_ok());
        assert!(GeneMix::parse("6G").is_ok());
    }

    #[test]
    fn pasted_text_is_read_generously() {
        let want = parse_list("GGGYYY WGXYYW").unwrap();
        for text in [
            "GGGYYY\nWGXYYW",
            "gggyyy, wgxyyw",
            "G-G-G-Y-Y-Y W-G-X-Y-Y-W",
            "G G G Y Y Y\tW G X Y Y W",
            "GGGYYYWGXYYW",
            "1. GGGYYY\r\n2. WGXYYW\r\n",
        ] {
            let pasted = parse_pasted(text);
            assert_eq!(pasted.plants, want, "{text}");
            assert!(pasted.ignored.is_empty(), "{text}");
        }
    }

    #[test]
    fn pasted_duplicates_and_junk_are_handled() {
        let pasted = parse_pasted("GGGYYY GGGYYY\nGGGYY\n??");
        assert_eq!(pasted.plants, parse_list("GGGYYY").unwrap());
        assert_eq!(pasted.ignored, vec!["GGGYY"]);
    }

    #[test]
    fn every_scanned_position_is_kept_in_order() {
        let (a, b) = (Genotype::parse("GGGYYY").unwrap(), Genotype::parse("WGXYYW").unwrap());
        let pasted = parse_pasted("GGGYYY\n-\nGGGYY\nWGXYYW\nGGGYYY\n\n");
        assert_eq!(pasted.scanned, vec![Some(a), None, None, Some(b), Some(a)]);
        assert_eq!(pasted.plants, vec![a, b]);
    }

    #[test]
    fn parsing_accepts_site_formatting_and_rejects_junk() {
        assert_eq!(Genotype::parse("w-g-x-y-y-w").unwrap(), Genotype::parse("WGXYYW").unwrap());
        assert!(Genotype::parse("WGXYY").is_err());
        assert!(Genotype::parse("WGXYYQ").is_err());
    }
}
