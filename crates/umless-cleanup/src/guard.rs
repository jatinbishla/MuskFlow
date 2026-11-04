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
/// Rewriting styles rephrase more, so they need to keep less word-for-word.
const MIN_KEPT_SHARE_REWRITE: f32 = 0.45;
/// Words a rewrite may turn into list structure.
const LIST_WORDS: &[&str] = &[
    "first",
    "second",
    "third",
    "fourth",
    "fifth",
    "firstly",
    "secondly",
    "thirdly",
    "lastly",
    "finally",
    "next",
    "things",
    "following",
];
/// A rewrite may reword, but most of its content words should still be the speaker's.
const MAX_ADDED_SHARE_REWRITE: f32 = 0.40;

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
    umless_eval::normalize(text)
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
    let all_input = umless_eval::normalize(raw);
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

/// Rewriting styles (formal, AI prompt) may rephrase, so word-for-word overlap is
/// too strict. What they must never do is invent facts or answer instead of rewrite:
/// no names or numbers the speaker didn't say, and most of what they said survives.
/// How freely a rewrite may reword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rewrite {
    /// Formal: rephrased sentences, but mostly the speaker's words.
    Polish,
    /// AI prompt: a restatement in new words is the point, so word overlap means little.
    /// What must hold: no invented names or numbers, no dropped names, no answering,
    /// and no longer than what was said.
    Restate,
}

pub fn check_rewrite(raw: &str, rewritten: &str, kind: Rewrite) -> Verdict {
    let all_input = umless_eval::normalize(raw);
    let input: HashSet<String> = content(raw).into_iter().collect();
    // List markers would otherwise count as the words "one", "two", "three".
    let unlisted: String = rewritten
        .split_whitespace()
        .filter(|w| !is_list_marker(w))
        .collect::<Vec<_>>()
        .join(" ");
    let output = content(&unlisted);
    let output_set: HashSet<&str> = output.iter().map(String::as_str).collect();
    let may_retract = retractable(&all_input);
    let raw_lower = raw.to_lowercase();

    // Names and numbers in the output that the speaker never said. List markers ("1.")
    // are formatting, not facts.
    let words: Vec<&str> = rewritten.split_whitespace().collect();
    let added: Vec<String> = words
        .iter()
        .enumerate()
        .filter(|(_, w)| !is_list_marker(w))
        .filter_map(|(i, w)| {
            let core = w.trim_matches(|c: char| !c.is_alphanumeric());
            // After a list marker or sentence end, a capital is just a sentence start.
            let starts_sentence = i == 0
                || words[i - 1].ends_with(['.', '?', '!', ':'])
                || is_list_marker(words[i - 1]);
            let is_name = !starts_sentence
                && core.chars().next().is_some_and(char::is_uppercase)
                && core.len() > 1
                && !matches!(core, "I" | "I'm" | "I'll" | "I've" | "I'd");
            let is_number = core.chars().any(|c| c.is_ascii_digit());
            ((is_name || is_number) && !mentions(&raw_lower, &all_input, core))
                .then(|| core.to_string())
        })
        .collect();
    // Ordinals become list numbering in a rewrite ("first … second …" → "1. … 2. …").
    let stood_by: Vec<&String> = input
        .iter()
        .filter(|w| !may_retract.contains(*w) && !LIST_WORDS.contains(&w.as_str()))
        .collect();
    let kept = if stood_by.is_empty() {
        1.0
    } else {
        stood_by
            .iter()
            .filter(|w| output_set.contains(w.as_str()))
            .count() as f32
            / stood_by.len() as f32
    };
    // Names the speaker said must survive a rewrite ("Hey Sarah" must not lose Sarah).
    let rewritten_lower = rewritten.to_lowercase();
    let dropped: Vec<String> = names(raw)
        .into_iter()
        .filter(|n| {
            !may_retract.contains(&n.to_lowercase()) && !rewritten_lower.contains(&n.to_lowercase())
        })
        .collect();
    let new_words = output.iter().filter(|w| !input.contains(*w)).count();
    let reason = if rewritten.trim().is_empty() && !input.is_empty() {
        Some("empty output")
    } else if !added.is_empty() {
        Some("added names or numbers the speaker didn't say")
    } else if !dropped.is_empty() {
        Some("dropped a name the speaker said")
    } else if rewritten.contains("```") || rewritten.contains("**") {
        Some("formatted like an answer, not a rewrite")
    } else if kind == Rewrite::Polish
        && new_words > 2
        && new_words as f32 > MAX_ADDED_SHARE_REWRITE * output.len() as f32
    {
        Some("added too much the speaker didn't say")
    } else if kind == Rewrite::Polish && kept < MIN_KEPT_SHARE_REWRITE {
        Some("dropped too much of what was said")
    } else if kind == Rewrite::Restate && output.len() > input.len() + input.len() / 3 + 4 {
        Some("longer than what was said (answered instead of rewrote?)")
    } else if output.len() > 3 * input.len().max(4) {
        Some("much longer than what was said (answered instead of rewrote?)")
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

/// "1." or "2)" at the start of a list item.
fn is_list_marker(w: &str) -> bool {
    let digits = w.trim_end_matches(['.', ')']);
    w.len() > digits.len()
        && !digits.is_empty()
        && digits.len() <= 2
        && digits.chars().all(|c| c.is_ascii_digit())
}

/// Capitalised words that aren't at a sentence start: the names a speech model heard.
fn names(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    words
        .iter()
        .enumerate()
        .filter(|(i, _)| *i > 0 && !words[i - 1].ends_with(['.', '?', '!']))
        .map(|(_, w)| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| {
            let lower = w.to_lowercase();
            w.len() > 1
                && w.chars().next().is_some_and(char::is_uppercase)
                // Speech models capitalise ordinary words mid-sentence ("task Can you…").
                && !FUNCTION_WORDS.contains(&lower.as_str())
                && !COMMON_WORDS.contains(&lower.as_str())
        })
        .map(String::from)
        .collect()
}

/// Whether a lower-case word is an everyday word, never a name ("the", "please", "okay").
pub fn is_everyday_word(lower: &str) -> bool {
    FUNCTION_WORDS.contains(&lower) || COMMON_WORDS.contains(&lower)
}

/// Everyday words a speech model may capitalise that are never names.
const COMMON_WORDS: &[&str] = &[
    "i",
    "i'm",
    "i'll",
    "i've",
    "i'd",
    "ok",
    "okay",
    "yes",
    "yeah",
    "no",
    "not",
    "now",
    "then",
    "please",
    "thanks",
    "thank",
    "hey",
    "hi",
    "hello",
    "what",
    "how",
    "why",
    "when",
    "where",
    "who",
    "which",
    "should",
    "shall",
    "may",
    "might",
    "must",
    "let's",
    "let",
    "there",
    "here",
    "they",
    "he",
    "she",
    "his",
    "her",
    "their",
    "them",
    "us",
    "if",
    "as",
    "all",
    "some",
    "any",
    "every",
    "one",
    "first",
    "second",
    "third",
    "next",
    "last",
    "maybe",
    "well",
    "actually",
    "basically",
    "right",
    "sure",
    "great",
    "good",
    "also",
    "after",
    "before",
    "because",
];

/// Whether the speaker said `word`, allowing for "$12,400" vs "twelve thousand four hundred".
fn mentions(raw_lower: &str, normalized_raw: &[String], word: &str) -> bool {
    let w = word.to_lowercase();
    if raw_lower.contains(&w) {
        return true;
    }
    let norm = umless_eval::normalize(word);
    !norm.is_empty()
        && normalized_raw
            .windows(norm.len())
            .any(|win| win.iter().zip(&norm).all(|(a, b)| a == b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_may_rephrase_but_not_invent() {
        let raw = "um so can you like help me write a function in python that takes a list of numbers and returns the average but ignore none values";
        let ok = "Write a Python function that takes a list of numbers and returns the average, ignoring None values.";
        assert!(
            check_rewrite(raw, ok, Rewrite::Restate).ok(),
            "{:?}",
            check_rewrite(raw, ok, Rewrite::Restate)
        );
        let invented = "Write a Python function for Stripe that returns the average of 10 numbers.";
        assert!(!check_rewrite(raw, invented, Rewrite::Restate).ok());
        let answered = "def average(nums):\n    vals = [n for n in nums if n is not None]\n    return sum(vals) / len(vals) if vals else 0.0\n\nThis function filters out None values, then divides the sum by the count, returning zero for an empty list so it never divides by zero, which is a common bug.";
        assert!(!check_rewrite(raw, answered, Rewrite::Restate).ok());
        let money = check_rewrite(
            "the invoice is twelve thousand four hundred dollars",
            "The invoice is $12,400.",
            Rewrite::Polish,
        );
        assert!(money.ok(), "{money:?}");
        let list = check_rewrite(
            "do the following first create a app second make it secure and third make it fast",
            "Do the following:\n1. Create an app.\n2. Make it secure.\n3. Make it fast.",
            Rewrite::Restate,
        );
        assert!(list.ok(), "{list:?}");
        let lost_name = check_rewrite(
            "hey um Sarah did you get the invoice I sent on Monday",
            "Hey, did you get the invoice I sent on Monday?",
            Rewrite::Polish,
        );
        assert_eq!(lost_name.reason, Some("dropped a name the speaker said"));
        let capitalised = check_rewrite(
            "now let's move to the next task Can you list what is left But only for the app",
            "List the remaining tasks for the app.",
            Rewrite::Restate,
        );
        assert!(capitalised.ok(), "{capitalised:?}");
        let padded = check_rewrite(
            "Sarah did you get the invoice",
            "Hi Sarah, I hope you're doing well and having a wonderful week. Did you get the invoice?",
            Rewrite::Polish,
        );
        assert!(!padded.ok());
    }

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
