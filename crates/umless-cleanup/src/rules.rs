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

/// Drops stray fillers ("um", "uh") an LLM left in, keeping line breaks and lists intact.
pub fn strip_fillers(text: &str) -> String {
    text.lines()
        .map(|line| {
            let kept: Vec<&str> = line
                .split(' ')
                .filter(|w| {
                    let core = w
                        .trim_matches(|c: char| !c.is_alphanumeric())
                        .to_lowercase();
                    core.is_empty() || !is_filler(&core)
                })
                .collect();
            let mut out = kept.join(" ");
            // "yeah, um, I'm" leaves "yeah, I'm" once the filler and its comma go.
            while out.contains(", ,") {
                out = out.replace(", ,", ",");
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Phrases that take back what came just before them. Kept to ones that are hard to mean
/// any other way: a bare "no" or "actually" is left to the LLM.
const RETRACT_CUES: &[&[&str]] = &[
    &["no", "wait"],
    &["wait", "no"],
    &["no", "sorry"],
    &["sorry", "no"],
    &["sorry", "i", "mean"],
    &["no", "i", "mean"],
    &["or", "rather"],
    &["scratch", "that"],
    &["no", "actually"],
    &["actually", "no"],
];
/// How far back the replaced phrase may start.
const RETRACT_REACH: usize = 6;

const DETERMINERS: &[&str] = &[
    "the", "my", "a", "an", "your", "our", "his", "her", "their", "this", "that", "these", "those",
];
const PREPOSITIONS: &[&str] = &[
    "to", "at", "on", "in", "by", "for", "from", "with", "before", "after", "until", "into",
];
const WEEKDAYS: &[&str] = &[
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "today",
    "tomorrow",
    "tonight",
];

/// Words that can stand in for each other in a correction: a phrase starting with "the"
/// replaces one starting with "my", "on Friday" replaces "to Thursday".
fn same_slot(a: &str, b: &str) -> bool {
    let number = |w: &str| w.chars().all(|c| c.is_ascii_digit()) || numbers_word(w);
    a == b
        || [DETERMINERS, PREPOSITIONS, WEEKDAYS]
            .iter()
            .any(|set| set.contains(&a) && set.contains(&b))
        || (number(a) && number(b))
}

fn numbers_word(w: &str) -> bool {
    matches!(
        w,
        "one"
            | "two"
            | "three"
            | "four"
            | "five"
            | "six"
            | "seven"
            | "eight"
            | "nine"
            | "ten"
            | "eleven"
            | "twelve"
            | "noon"
            | "midnight"
    )
}

/// Applies explicit self-corrections: "look at my auth middleware, no wait, the session
/// handler" → "look at the session handler". The correction replaces the phrase before the
/// cue that starts the same way (a determiner for a determiner, a weekday for a weekday,
/// the same word); with no such phrase nearby the text is left for the LLM.
pub fn retract(raw: &str) -> String {
    let mut toks: Vec<String> = raw.split_whitespace().map(String::from).collect();
    let core = |t: &str| {
        t.trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase()
    };
    let mut i = 0;
    while i < toks.len() {
        let words: Vec<String> = toks.iter().map(|t| core(t)).collect();
        let Some(cue) = RETRACT_CUES.iter().find(|cue| {
            i + cue.len() <= words.len() && cue.iter().zip(&words[i..]).all(|(c, w)| c == w)
        }) else {
            i += 1;
            continue;
        };
        let fix = i + cue.len();
        let Some(first) = words.get(fix).filter(|w| !w.is_empty()) else {
            i += 1;
            continue;
        };
        // Look back for where the replaced phrase starts, without crossing a sentence.
        let mut anchor = None;
        for k in (i.saturating_sub(RETRACT_REACH)..i).rev() {
            if toks[k].ends_with(['.', '?', '!']) {
                break;
            }
            if same_slot(&words[k], first) {
                anchor = Some(k);
                break;
            }
        }
        let Some(a) = anchor else {
            i += 1;
            continue;
        };
        // Keep what the dropped phrase's first word was carrying in front ("(my" → "(").
        let lead: String = toks[a]
            .chars()
            .take_while(|c| !c.is_alphanumeric())
            .collect();
        // "We ship Friday, scratch that, next…" keeps the capital the dropped word had.
        let capital = toks[a]
            .chars()
            .find(|c| c.is_alphanumeric())
            .is_some_and(char::is_uppercase);
        let fixed = if capital {
            capitalize_first(&toks[fix])
        } else {
            toks[fix].clone()
        };
        toks.splice(a..=fix, [format!("{lead}{fixed}")]);
        i = a + 1;
    }
    toks.join(" ")
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
    fn strips_leftover_fillers_but_keeps_lines() {
        assert_eq!(
            strip_fillers("Haha yeah, um, I'm late.\n1. Grab a table, uh, please."),
            "Haha yeah, I'm late.\n1. Grab a table, please."
        );
    }

    #[test]
    fn applies_explicit_self_corrections() {
        assert_eq!(
            retract(
                "so can you look at my auth middleware, no wait, the session handler, and tell me why"
            ),
            "so can you look at the session handler, and tell me why"
        );
        assert_eq!(
            retract("move the call to tuesday no wait wednesday at three"),
            "move the call to wednesday at three"
        );
        assert_eq!(
            retract("we ship on friday scratch that we ship next monday"),
            "we ship next monday"
        );
        assert_eq!(retract("Send it at 3, sorry I mean at 4."), "Send it at 4.");
        // Nothing the correction could replace: left for the LLM.
        assert_eq!(retract("ping Sam no wait Priya"), "ping Sam no wait Priya");
        // "no wait" that isn't a correction.
        assert_eq!(
            retract("there's no wait time at all"),
            "there's no wait time at all"
        );
        // Never reaches back into the previous sentence.
        assert_eq!(
            retract("Book the room. No wait, the hall."),
            "Book the room. No wait, the hall."
        );
    }

    #[test]
    fn keeps_deliberate_repeats_across_punctuation() {
        assert_eq!(clean("Bye, bye now"), "Bye, bye now");
    }
}
