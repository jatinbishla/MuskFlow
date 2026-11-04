//! Learning from corrections: when the user fixes a word MuskFlow typed ("Pria" →
//! "Priya"), the fix goes into their dictionary, so the next dictation gets it right.
//!
//! After typing, the field is read again a few times over the next minute and a half
//! and lined up against what was typed. A word swapped for a near-spelling of itself
//! is a correction; anything else (rewording, typing more) is left alone.

use std::path::Path;
use std::time::{Duration, Instant};

use mf_cleanup::Dictionary;
use mf_cleanup::guard::is_everyday_word;

/// When to look at the field again, after typing into it.
pub const CHECKS: [Duration; 3] = [
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(90),
];

/// More edits than this share of the typed words means it isn't the same text any more.
const MAX_CHANGED_SHARE: f32 = 0.4;

/// Text MuskFlow typed and is watching for corrections.
pub struct Pending {
    pub pid: i32,
    pub typed: String,
    pub at: Instant,
    /// Checks done so far.
    pub checks: usize,
}

impl Pending {
    /// When the next check is due, or `None` when done watching.
    pub fn next_check(&self) -> Option<Instant> {
        CHECKS.get(self.checks).map(|d| self.at + *d)
    }
}

/// Words the user corrected: `typed` lined up inside `field` (the field's whole text now),
/// keeping 1:1 swaps where the new spelling is one the dictionary would match the old one to.
pub fn corrections(typed: &str, field: &str, dictionary: &Dictionary) -> Vec<String> {
    let a = words(typed);
    let b = words(field);
    // A huge document makes the alignment slow; corrections there aren't worth it.
    if a.is_empty() || b.is_empty() || a.len() * b.len() > 2_000_000 {
        return Vec::new();
    }
    let swaps = match align(&a, &b) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let mut learned = Vec::new();
    for (old, new) in swaps {
        let lower = new.to_lowercase();
        let looks_like_a_term = new.chars().filter(|c| c.is_alphabetic()).count() >= 3
            && !is_everyday_word(&lower)
            && !dictionary.terms().any(|t| t.eq_ignore_ascii_case(new));
        // The fix must be a spelling the dictionary will match the misheard word to.
        if looks_like_a_term
            && !old.eq_ignore_ascii_case(new)
            && Dictionary::new([new]).apply(old) == new
            && !learned.contains(&new.to_string())
        {
            learned.push(new.to_string());
        }
    }
    learned
}

fn words(text: &str) -> Vec<&str> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Lines `a` up against the best-matching stretch of `b` (edit distance over words, free
/// to start and end anywhere in `b`). Returns the 1:1 substitutions, or `None` if the
/// stretch differs too much to be the same text.
fn align<'a, 'b>(a: &[&'a str], b: &[&'b str]) -> Option<Vec<(&'a str, &'b str)>> {
    let (n, m) = (a.len(), b.len());
    let same = |i: usize, j: usize| a[i].eq_ignore_ascii_case(b[j]);
    // d[i][j]: cost of aligning a[..i] to a stretch of b ending at j.
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for i in 1..=n {
        for j in 1..=m {
            let sub = d[i - 1][j - 1] + usize::from(!same(i - 1, j - 1));
            d[i][j] = sub.min(d[i - 1][j] + 1).min(d[i][j - 1] + 1);
        }
    }
    let (end, cost) = (0..=m).map(|j| (j, d[n][j])).min_by_key(|&(_, c)| c)?;
    if cost as f32 > n as f32 * MAX_CHANGED_SHARE {
        return None;
    }
    let mut swaps = Vec::new();
    let (mut i, mut j) = (n, end);
    while i > 0 {
        if j > 0 && d[i][j] == d[i - 1][j - 1] + usize::from(!same(i - 1, j - 1)) {
            if !same(i - 1, j - 1) {
                swaps.push((a[i - 1], b[j - 1]));
            }
            i -= 1;
            j -= 1;
        } else if d[i][j] == d[i - 1][j] + 1 {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    swaps.reverse();
    Some(swaps)
}

/// Appends learned words to the dictionary file, under a heading the user can recognise.
pub fn save(path: &Path, words: &[String]) -> std::io::Result<()> {
    use std::io::Write;
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    const HEADING: &str = "# Learned from your corrections";
    if !existing.contains(HEADING) {
        let gap = if existing.is_empty() || existing.ends_with("\n\n") {
            ""
        } else if existing.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        writeln!(f, "{gap}{HEADING}")?;
    } else if !existing.ends_with('\n') {
        writeln!(f)?;
    }
    for w in words {
        writeln!(f, "{w}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_name_is_learned() {
        let typed = "Hey Pria, can we move the call to Friday?";
        let field = "Hi team,\nHey Priya, can we move the call to Friday? \nThanks";
        assert_eq!(
            corrections(typed, field, &Dictionary::default()),
            vec!["Priya"]
        );
    }

    #[test]
    fn rewording_is_not_a_correction() {
        let typed = "Can we move the call to Friday?";
        // A different word, not a respelling.
        let field = "Can we move the meeting to Friday?";
        assert!(corrections(typed, field, &Dictionary::default()).is_empty());
        // Everyday words aren't dictionary material.
        assert!(corrections("send it their", "send it there", &Dictionary::default()).is_empty());
        // A different text altogether.
        assert!(
            corrections(
                "Can we move the call to Friday?",
                "Totally unrelated draft about lunch plans",
                &Dictionary::default()
            )
            .is_empty()
        );
    }

    #[test]
    fn known_terms_are_not_learned_twice() {
        let d = Dictionary::new(["SubFlo"]);
        assert!(corrections("the Subflow deck", "the SubFlo deck", &d).is_empty());
    }

    #[test]
    fn save_adds_a_heading_once() {
        let dir = std::env::temp_dir().join(format!("mf-learn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dictionary.txt");
        std::fs::write(&path, "# my terms\nSubFlo\n").unwrap();
        save(&path, &["Priya".into()]).unwrap();
        save(&path, &["Gaurav".into()]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "# my terms\nSubFlo\n\n# Learned from your corrections\nPriya\nGaurav\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
