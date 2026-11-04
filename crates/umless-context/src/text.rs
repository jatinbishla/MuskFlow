//! What the text on screen tells us: the names in it, and how formal the conversation is.

use umless_cleanup::Style;
use umless_cleanup::guard::is_everyday_word;

/// At most this many names are taken from the screen, the most recent first.
const MAX_NAMES: usize = 120;

/// Names and terms on screen, spelled the way the screen spells them: "Priya", "SubFlo",
/// "Turner Construction". A capitalised word counts only away from a sentence start, so
/// "Thanks" opening a line isn't taken for a name. Most recent (end of the text) first.
pub fn names(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in text.lines().rev() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let mut run: Vec<&str> = Vec::new();
        let flush = |run: &mut Vec<&str>, found: &mut Vec<String>| {
            if run.len() >= 2 {
                push(found, run.join(" "));
            }
            for w in run.iter() {
                push(found, w.to_string());
            }
            run.clear();
        };
        for (i, raw) in words.iter().enumerate() {
            let w = raw.trim_matches(|c: char| !c.is_alphanumeric());
            let starts_sentence = i == 0 || words[i - 1].ends_with(['.', '?', '!', ':']);
            if is_name(w) && (!starts_sentence || has_inner_capital(w)) {
                run.push(w);
                // "Construction," ends the run after it.
                if raw.ends_with([',', '.', '?', '!', ';', ':']) {
                    flush(&mut run, &mut found);
                }
            } else {
                flush(&mut run, &mut found);
            }
        }
        flush(&mut run, &mut found);
        if found.len() >= MAX_NAMES {
            break;
        }
    }
    found.truncate(MAX_NAMES);
    found
}

fn push(found: &mut Vec<String>, name: String) {
    if !found.contains(&name) {
        found.push(name);
    }
}

fn is_name(w: &str) -> bool {
    let letters = w.chars().filter(|c| c.is_alphabetic()).count();
    letters >= 3
        && w.chars().next().is_some_and(char::is_uppercase)
        && w.chars().all(|c| c.is_alphanumeric() || c == '\'' || c == '-')
        && !is_everyday_word(&w.to_lowercase())
        && !WEEKDAYS_MONTHS.contains(&w.to_lowercase().as_str())
        // All-caps words are shouting or acronyms; the speech model spells acronyms fine.
        && !w.chars().all(|c| !c.is_lowercase())
}

/// "SubFlo", "iPhone": a capital after the first letter marks a name even at a sentence start.
fn has_inner_capital(w: &str) -> bool {
    w.chars().skip(1).any(char::is_uppercase) && w.chars().any(char::is_lowercase)
}

const WEEKDAYS_MONTHS: &[&str] = &[
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "january",
    "february",
    "march",
    "april",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

/// The tone of a conversation, when the recent text makes it clear: a Teams chat full of
/// "haha" and emoji is casual even though Teams is a work app, and a Slack thread that
/// opens with "Dear" and closes with "Kind regards" is formal.
pub fn tone(text: &str) -> Option<Style> {
    // The most recent part of the conversation counts.
    let start = text
        .char_indices()
        .rev()
        .nth(1500)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let recent = &text[start..];
    let lower = recent.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .collect();
    let casual = words.iter().filter(|w| CASUAL_WORDS.contains(w)).count()
        + recent.chars().filter(|c| is_emoji(*c)).count()
        + lower.matches("!!").count()
        + lower.matches(":)").count()
        + lower.matches(":d").count();
    let formal = FORMAL_PHRASES
        .iter()
        .map(|p| lower.matches(p).count())
        .sum::<usize>();
    if casual >= 2 && casual > formal * 2 {
        Some(Style::Casual)
    } else if formal >= 2 && formal > casual * 2 {
        Some(Style::Formal)
    } else {
        None
    }
}

const CASUAL_WORDS: &[&str] = &[
    "lol", "haha", "hahaha", "lmao", "omg", "gonna", "wanna", "gotta", "yeah", "yep", "nah", "bro",
    "dude", "btw", "tbh", "idk", "thx", "pls", "np", "ya", "cool", "awesome", "ok", "k", "sup",
    "hehe",
];

const FORMAL_PHRASES: &[&str] = &[
    "dear ",
    "regards",
    "sincerely",
    "please find",
    "attached",
    "i hope this",
    "thank you for",
    "could you please",
    "kindly",
    "at your earliest",
    "further to",
    "as discussed",
    "please let me know",
    "looking forward",
];

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F300..=0x1FAFF | 0x2600..=0x27BF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_mid_sentence_capitals() {
        let thread = "Hi Jatin,\nThanks for the update. Can you loop in Priya from Turner Construction, and send the SubFlo deck?\nBest, Gaurav";
        let names = names(thread);
        for n in [
            "Jatin",
            "Priya",
            "Turner Construction",
            "Turner",
            "SubFlo",
            "Gaurav",
        ] {
            assert!(names.contains(&n.to_string()), "missing {n}: {names:?}");
        }
        // Sentence starts and everyday words aren't names.
        for n in ["Thanks", "Can", "Hi", "Best"] {
            assert!(!names.contains(&n.to_string()), "{n} isn't a name");
        }
    }

    #[test]
    fn newest_names_first() {
        let names = names("met Alice today\nthen Bob called");
        assert_eq!(names[0], "Bob");
    }

    #[test]
    fn days_and_acronyms_are_left_alone() {
        assert!(names("see you on Friday at the AWS summit").is_empty());
    }

    #[test]
    fn tone_follows_the_conversation() {
        assert_eq!(
            tone("haha yeah that was wild 😂\nlol ok see you at 5"),
            Some(Style::Casual)
        );
        assert_eq!(
            tone("Dear Mr. Shah,\nPlease find attached the revised invoice.\nKind regards,\nJatin"),
            Some(Style::Formal)
        );
        assert_eq!(tone("The build is green. Shipping it tomorrow."), None);
    }
}
