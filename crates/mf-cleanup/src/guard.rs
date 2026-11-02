//! Faithfulness guard: cleanup may delete and reformat, but must not invent.
//!
//! Both texts are normalised the same way as benchmarks score them (numbers
//! spelled out, fillers and punctuation dropped), then compared as bags of
//! content words. Function words ("the", "and", "is") are free to come and go.

use std::collections::HashSet;

/// Words the LLM may add or drop without changing meaning.
const FUNCTION_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "so", "to", "of", "in", "on", "at", "for", "with", "by",
    "from", "is", "are", "was", "were", "be", "it", "its", "this", "that", "we", "you", "i", "me",
    "my", "our", "your", "can", "could", "would", "will", "do", "does", "just", "also", "then",
    "there", "here", "as", "like", "okay", "yeah", "well", "please", "no", "not", "wait",
    "actually", "sorry", "mean", "rather", "instead", "make", "let", "lets", "s", "im", "dollars",
    "percent",
];

/// Extra words that may freely vanish: greetings and fillers ASR misheard as words.
const DISPOSABLE: &[&str] = &[
    "hey", "hi", "oh", "ill", "all", "um", "uh", "had", "have", "has", "get", "got",
];

/// Phrases that mark a self-correction. Content words in the few words before one may be
/// dropped (they're what the speaker took back).
const CORRECTION_CUES: &[&[&str]] = &[
    &["no", "wait"],
    &["no", "actually"],
    &["actually"],
    &["scratch", "that"],
    &["make", "that"],
    &["i", "mean"],
    &["sorry"],
    &["no"],
];
/// How far back before a cue the retracted words may reach.
const RETRACT_WINDOW: usize = 8;
/// Content words that may vanish outside a correction (ASR junk the LLM rightly removes).
const MAX_UNEXPLAINED_DROPS: usize = 1;

/// At most this share of the output's content words may be absent from the input.
const MAX_ADDED_SHARE: f32 = 0.10;
/// The output must keep at least this share of the input's content words
/// (self-corrections legitimately drop some).
const MIN_KEPT_SHARE: f32 = 0.60;

#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// Content words in the output that the speaker never said.
    pub added: Vec<String>,
    /// Content words the speaker said that vanished, outside any self-correction.
    pub dropped: Vec<String>,
    /// Share of the input's content words kept in the output.
    pub kept: f32,
    pub reason: Option<&'static str>,
}

impl Verdict {
    pub fn ok(&self) -> bool {
        self.reason.is_none()
    }
}

fn content(text: &str) -> Vec<String> {
    mf_eval::normalize(text)
        .into_iter()
        .filter(|w| !FUNCTION_WORDS.contains(&w.as_str()) && !DISPOSABLE.contains(&w.as_str()))
        .collect()
}

/// Content words the speaker may legitimately have taken back: those in the
/// [`RETRACT_WINDOW`] words before each correction cue.
fn retractable(words: &[String]) -> HashSet<String> {
    let mut out = HashSet::new();
    for i in 0..words.len() {
        let is_cue = CORRECTION_CUES.iter().any(|cue| {
            i + cue.len() <= words.len() && cue.iter().zip(&words[i..]).all(|(c, w)| c == w)
        });
        if is_cue {
            for w in &words[i.saturating_sub(RETRACT_WINDOW)..i] {
                out.insert(w.clone());
            }
        }
    }
    out
}

pub fn check(raw: &str, cleaned: &str) -> Verdict {
    let all_input = mf_eval::normalize(raw);
    let input = content(raw);
    let output = content(cleaned);
    let input_set: HashSet<&str> = input.iter().map(String::as_str).collect();
    let output_set: HashSet<&str> = output.iter().map(String::as_str).collect();
    let may_retract = retractable(&all_input);

    let added: Vec<String> = output
        .iter()
        .filter(|w| !input_set.contains(w.as_str()))
        .cloned()
        .collect();
    let mut dropped: Vec<String> = input
        .iter()
        .filter(|w| !output_set.contains(w.as_str()) && !may_retract.contains(*w))
        .cloned()
        .collect();
    dropped.dedup();
    // Share of what the speaker stood by (not retracted) that survived.
    let stood_by: Vec<&str> = input_set
        .iter()
        .copied()
        .filter(|w| !may_retract.contains(*w))
        .collect();
    let kept = if stood_by.is_empty() {
        1.0
    } else {
        stood_by.iter().filter(|w| output_set.contains(*w)).count() as f32 / stood_by.len() as f32
    };

    let reason = if cleaned.trim().is_empty() && !input.is_empty() {
        Some("empty output")
    } else if added.len() > 1 && added.len() as f32 > MAX_ADDED_SHARE * output.len().max(1) as f32 {
        Some("added words the speaker didn't say")
    } else if dropped.len() > MAX_UNEXPLAINED_DROPS {
        Some("dropped words the speaker didn't take back")
    } else if kept < MIN_KEPT_SHARE {
        Some("dropped too much of what was said")
    } else {
        None
    };
    Verdict {
        added,
        dropped,
        kept,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_cleanup_and_reformatting() {
        let v = check(
            "um so can we uh move the the call to tuesday no wait wednesday at like three pm",
            "So can we move the call to Wednesday at 3 pm?",
        );
        assert!(v.ok(), "{v:?}");
        assert!(check("it's twelve thousand four hundred dollars", "It's $12,400.").ok());
    }

    #[test]
    fn rejects_answers_and_inventions() {
        let v = check(
            "what time is the standup tomorrow",
            "The standup is at 10 am tomorrow in the main room.",
        );
        assert!(!v.ok());
        let v = check(
            "send the invoice to sarah",
            "Here is a professional email: Dear Sarah, please find attached the invoice for your review.",
        );
        assert!(!v.ok());
    }

    #[test]
    fn rejects_dropping_most_of_the_message() {
        assert!(
            !check(
                "tell gaurav the kubernetes migration is done and staging is tested",
                "Done."
            )
            .ok()
        );
    }

    #[test]
    fn rejects_a_dropped_clause_but_allows_retractions() {
        let raw = "three things billing apps going back and forth with the contractor and interacting with the foreman";
        let v = check(
            raw,
            "Three things: billing apps and interacting with the foreman.",
        );
        assert!(!v.ok(), "{v:?}");
        let v = check(
            "could we move the review to wednesday at three pm no actually make that thursday morning",
            "Could we move the review to Thursday morning?",
        );
        assert!(v.ok(), "{v:?}");
    }
}
