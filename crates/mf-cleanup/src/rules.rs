//! Deterministic cleanup: always safe, never changes meaning. Used for short
//! utterances and as the fallback when the LLM's output fails the guard.

const FILLERS: &[&str] = &[
    "um", "uh", "uhm", "umm", "uhh", "ah", "er", "erm", "hmm", "mm",
];

pub fn is_filler(word: &str) -> bool {
    FILLERS.contains(&word)
}

/// Removes fillers and stammered repeats, tidies spacing, capitalises the start.
pub fn clean(raw: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for tok in raw.split_whitespace() {
        let core = tok
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        if is_filler(&core) {
            // "Friday, uh, and" → keep the comma the filler was carrying on the previous word.
            continue;
        }
        if let Some(prev) = out.last() {
            let prev_core = prev
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase();
            let prev_ends_clause = prev.ends_with(['.', ',', '?', '!', ';', ':']);
            if !core.is_empty() && core == prev_core && !prev_ends_clause {
                out.pop(); // "the the review" → keep the second copy (it carries any punctuation)
            }
        }
        out.push(tok.to_string());
    }
    let mut s = out.join(" ");
    // Fillers can leave ", ," or a leading comma behind.
    while s.contains(", ,") {
        s = s.replace(", ,", ",");
    }
    let s = s.trim_start_matches([',', ' ']).trim().to_string();
    capitalize_first(&s)
}

fn capitalize_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_fillers_and_stammers() {
        assert_eq!(
            clean("um, so hey Priya, can we uh push the the review"),
            "So hey Priya, can we push the review"
        );
        assert_eq!(clean("Uh."), "");
    }

    #[test]
    fn keeps_deliberate_repeats_across_punctuation() {
        assert_eq!(clean("Bye, bye now"), "Bye, bye now");
    }
}
