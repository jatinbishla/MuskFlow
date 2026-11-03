//! Turns hotword phrases into the token sequences sherpa-onnx boosts.
//!
//! For NeMo transducers sherpa-onnx looks each hotword up in `tokens.txt` as-is,
//! so a plain word like "foreman" (three SentencePiece pieces) is rejected and,
//! with it, the whole list. We do the SentencePiece BPE encoding ourselves:
//! `tokens.txt` is ordered by merge rank, so merging the adjacent pair whose
//! result has the lowest id, until no pair merges, reproduces the model's own
//! segmentation.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};

/// SentencePiece word-boundary marker.
const SPACE: char = '▁';

pub struct Vocab {
    ids: HashMap<String, usize>,
}

impl Vocab {
    pub fn load(tokens: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(tokens)
            .with_context(|| format!("could not read {}", tokens.display()))?;
        Ok(Self::parse(&text))
    }

    /// `tokens.txt` lines are `<piece> <id>`.
    fn parse(text: &str) -> Self {
        let ids = text
            .lines()
            .filter_map(|l| {
                let (piece, id) = l.rsplit_once(' ')?;
                Some((piece.to_string(), id.trim().parse().ok()?))
            })
            .collect();
        Self { ids }
    }

    /// One word → its pieces, or `None` if a character isn't in the vocabulary.
    fn encode_word(&self, word: &str) -> Option<Vec<String>> {
        let mut syms: Vec<String> = std::iter::once(SPACE)
            .chain(word.chars())
            .map(String::from)
            .collect();
        loop {
            let best = syms
                .windows(2)
                .enumerate()
                .filter_map(|(i, p)| Some((self.ids.get(&format!("{}{}", p[0], p[1]))?, i)))
                .min();
            let Some((_, i)) = best else { break };
            let right = syms.remove(i + 1);
            syms[i].push_str(&right);
        }
        syms.iter()
            .all(|s| self.ids.contains_key(s))
            .then_some(syms)
    }

    /// A phrase → space-separated pieces, the hotword line sherpa-onnx expects.
    pub fn encode(&self, phrase: &str) -> Option<String> {
        let mut out = Vec::new();
        for word in phrase.split_whitespace() {
            out.extend(self.encode_word(word)?);
        }
        (!out.is_empty()).then(|| out.join(" "))
    }

    /// Hotword lines for sherpa-onnx. Each phrase is boosted as written and in the
    /// other casings a cased model may emit (sentence start, mid-sentence), since a
    /// boost only applies to the exact piece sequence. Unencodable phrases are skipped.
    pub fn hotword_lines(&self, phrases: &[String]) -> String {
        let mut lines: Vec<String> = Vec::new();
        for phrase in phrases {
            for variant in casings(phrase) {
                match self.encode(&variant) {
                    Some(line) if !lines.contains(&line) => lines.push(line),
                    Some(_) => {}
                    None => log::warn!("hotword {variant:?} has characters the model can't spell"),
                }
            }
        }
        lines.join("\n")
    }
}

fn casings(phrase: &str) -> Vec<String> {
    let phrase = phrase.trim();
    let mut first = phrase.chars();
    let flip = |f: char, rest: &str| -> String {
        if f.is_uppercase() {
            f.to_lowercase().chain(rest.chars()).collect()
        } else {
            f.to_uppercase().chain(rest.chars()).collect()
        }
    };
    match first.next() {
        Some(f) => vec![phrase.to_string(), flip(f, first.as_str())],
        None => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vocab {
        // Merge order: lower id merges first.
        Vocab::parse(
            "<unk> 0\n▁f 1\nor 2\n▁for 3\nem 4\nan 5\n▁ 6\nf 7\no 8\nr 9\ne 10\nm 11\na 12\nn 13\nF 14\n▁F 15",
        )
    }

    #[test]
    fn encodes_by_merge_rank() {
        assert_eq!(vocab().encode("foreman").as_deref(), Some("▁for em an"));
        assert_eq!(vocab().encode("Forman").as_deref(), Some("▁F or m an"));
    }

    #[test]
    fn skips_unspellable_phrases() {
        let v = vocab();
        assert_eq!(v.encode("fox"), None);
        assert_eq!(
            v.hotword_lines(&["fox".into(), "foreman".into()]),
            "▁for em an\n▁F or em an"
        );
    }
}
