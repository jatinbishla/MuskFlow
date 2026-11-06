//! Deterministic cleanup: always safe, never changes meaning. Used for short
//! utterances and as the fallback when the LLM's output fails the guard.

const FILLERS: &[&str] = &[
    "um", "uh", "uhm", "umm", "uhh", "ah", "er", "erm", "hmm", "mm",
];

pub fn is_filler(word: &str) -> bool {
    FILLERS.contains(&word)
}

/// Words "you know" leads into when it's a real question or clause ("you know what I mean").
const KNOW_OBJECTS: &[&str] = &[
    "that", "what", "how", "why", "who", "where", "when", "whether", "if", "about", "him", "her",
    "them", "it", "me", "this",
];
/// Words before "you know" that make it literal ("do you know", "if you know").
const KNOW_ASKERS: &[&str] = &[
    "do", "did", "don't", "didn't", "if", "as", "would", "will", "you'd", "could", "may", "might",
    "let", "and",
];
/// "like" before these is a hedge, not a verb: "like super slow".
const LIKE_HEDGED: &[&str] = &[
    "super",
    "really",
    "so",
    "way",
    "totally",
    "literally",
    "basically",
    "a",
    "an",
    "about",
    "around",
    "maybe",
];

fn core(tok: &str) -> String {
    tok.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
        .to_lowercase()
}

/// Drops spoken hedges that are never content: "you know" and "like" used as fillers.
fn drop_hedges(raw: &str) -> String {
    let toks: Vec<&str> = raw.split_whitespace().collect();
    let mut keep = vec![true; toks.len()];
    for i in 0..toks.len() {
        let next = toks.get(i + 1).map(|t| core(t)).unwrap_or_default();
        let prev = i.checked_sub(1).map(|j| core(toks[j])).unwrap_or_default();
        match core(toks[i]).as_str() {
            "you" if next == "know" && !toks[i + 1].ends_with('?') => {
                let after = toks.get(i + 2).map(|t| core(t)).unwrap_or_default();
                if !KNOW_ASKERS.contains(&prev.as_str()) && !KNOW_OBJECTS.contains(&after.as_str())
                {
                    keep[i] = false;
                    keep[i + 1] = false;
                }
            }
            "like" => {
                // "it's like super slow"; not "I like it", "looks like a bug", and not
                // "like 90%", which means "about".
                let hedge_after = LIKE_HEDGED.contains(&next.as_str());
                let verb_before = matches!(
                    prev.as_str(),
                    "i" | "you"
                        | "we"
                        | "they"
                        | "would"
                        | "looks"
                        | "look"
                        | "looked"
                        | "seems"
                        | "feel"
                        | "feels"
                        | "sounds"
                        | "just"
                        | "don't"
                        | "didn't"
                        | "really"
                );
                if hedge_after && !verb_before && !matches!(next.as_str(), "a" | "an") {
                    keep[i] = false;
                }
            }
            _ => {}
        }
    }
    toks.iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(t, _)| *t)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Removes fillers, hedges and stammered repeats, tidies spacing, capitalises the start.
pub fn clean(raw: &str) -> String {
    let raw = &drop_hedges(raw);
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

/// Words that open a yes/no question when a pronoun or "there" follows ("can you", "is it").
const QUESTION_AUX: &[&str] = &[
    "can",
    "could",
    "would",
    "will",
    "should",
    "shall",
    "do",
    "does",
    "did",
    "is",
    "are",
    "was",
    "were",
    "have",
    "has",
    "may",
    "might",
    "am",
    "isn't",
    "aren't",
    "don't",
    "doesn't",
    "didn't",
    "can't",
    "won't",
    "wouldn't",
    "shouldn't",
    "couldn't",
];
const QUESTION_SUBJECTS: &[&str] = &[
    "i", "you", "we", "they", "he", "she", "it", "there", "this", "that", "these", "those",
    "anyone", "someone", "everyone",
];
const WH_WORDS: &[&str] = &[
    "what", "why", "how", "when", "where", "who", "which", "whose",
];

/// Whether a sentence reads as a direct question: "Can you check…", "What time is…".
/// "What we need is…" and "How it works:" open with a wh-word but are statements.
fn is_question(sentence: &str) -> bool {
    let words: Vec<String> = sentence
        .split_whitespace()
        .take(5)
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                .to_lowercase()
        })
        .collect();
    let Some(first) = words.first() else {
        return false;
    };
    if QUESTION_AUX.contains(&first.as_str()) {
        return words
            .get(1)
            .is_some_and(|w| QUESTION_SUBJECTS.contains(&w.as_str()));
    }
    if WH_WORDS.contains(&first.as_str()) {
        let next = words.get(1).map(String::as_str).unwrap_or("");
        let subject_first = QUESTION_SUBJECTS.contains(&next) && next != "there";
        return !subject_first
            && words[1..]
                .iter()
                .take(3)
                .any(|w| QUESTION_AUX.contains(&w.as_str()) || w.ends_with("'s"));
    }
    false
}

/// Ends direct questions with "?" where the speech model or a small LLM wrote "." or nothing.
/// Line by line, so lists and paragraphs keep their shape.
pub fn mark_questions(text: &str) -> String {
    text.lines()
        .map(|line| {
            let mut out = String::with_capacity(line.len());
            let mut start = 0;
            let chars: Vec<(usize, char)> = line.char_indices().collect();
            for (i, &(at, c)) in chars.iter().enumerate() {
                let ends = matches!(c, '.' | '!' | '?')
                    && chars.get(i + 1).is_none_or(|&(_, n)| n.is_whitespace());
                let last = i + 1 == chars.len();
                if !ends && !last {
                    continue;
                }
                let end = at + c.len_utf8();
                let sentence = &line[start..end];
                if c == '.' && is_question(sentence) {
                    out.push_str(&sentence[..sentence.len() - 1]);
                    out.push('?');
                } else if !ends && c.is_alphanumeric() && is_question(sentence) {
                    // Small models often drop the final punctuation altogether.
                    out.push_str(sentence);
                    out.push('?');
                } else {
                    out.push_str(sentence);
                }
                start = end;
            }
            out.push_str(&line[start..]);
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

    #[test]
    fn marks_direct_questions() {
        assert_eq!(
            mark_questions("Can you tell me if the contract is ready for review."),
            "Can you tell me if the contract is ready for review?"
        );
        assert_eq!(
            mark_questions("Thanks. What time is the standup tomorrow. See you."),
            "Thanks. What time is the standup tomorrow? See you."
        );
        assert_eq!(
            mark_questions("What is the capital of France"),
            "What is the capital of France?"
        );
        assert_eq!(
            mark_questions("How many days are there in a leap year."),
            "How many days are there in a leap year?"
        );
        assert_eq!(
            mark_questions("Do you think we should hire a designer before the launch or after."),
            "Do you think we should hire a designer before the launch or after?"
        );
        // Statements that only look like questions stay as they are.
        for s in [
            "What we need is a faster build.",
            "How it works is simple.",
            "Can opener is broken.",
            "Analyze the session handler to determine why tokens expire after 5 minutes.",
            "Why tokens expire early.",
            "1. Make it secure\n2. Make it run fast.",
            "The price is 3.5 dollars.",
        ] {
            assert_eq!(mark_questions(s), s);
        }
    }

    #[test]
    fn drops_spoken_hedges() {
        assert_eq!(
            clean("ignore the you know none values and it's like super slow with like 500 rows"),
            "Ignore the none values and it's super slow with like 500 rows"
        );
        for kept in [
            "do you know the answer",
            "you know what I mean",
            "I like it",
            "it looks like a bug",
            "something like a cache",
            "I would like 2 tickets",
        ] {
            assert_eq!(clean(kept).to_lowercase(), kept.to_lowercase());
        }
    }
}
