//! Spoken numbers → digits: "five minutes instead of thirty" → "5 minutes instead of 30".
//!
//! Every prompt asks the LLM for digits, but small models often leave the words, so this
//! runs on the final text whatever the tier. It's conservative: a lone small number turns
//! into digits only next to a unit ("five minutes", "three pm") or when compared with one
//! that did ("5 minutes instead of ten"); "one of them", "no one" and "nine to five" stay.

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 8] = [
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// Words after a number that make it a quantity worth writing in digits.
const UNITS: &[&str] = &[
    "second",
    "seconds",
    "sec",
    "secs",
    "minute",
    "minutes",
    "min",
    "mins",
    "hour",
    "hours",
    "hr",
    "hrs",
    "day",
    "days",
    "week",
    "weeks",
    "month",
    "months",
    "year",
    "years",
    "percent",
    "dollar",
    "dollars",
    "bucks",
    "euro",
    "euros",
    "pound",
    "pounds",
    "rupee",
    "rupees",
    "am",
    "pm",
    "a.m",
    "p.m",
    "o'clock",
    "million",
    "billion",
    "lakh",
    "lakhs",
    "crore",
    "crores",
    "gb",
    "mb",
    "kb",
    "tb",
    "gigs",
    "gigabytes",
    "megabytes",
    "ms",
    "milliseconds",
    "px",
    "pixels",
    "k",
];
/// Units where even "one" reads better as a digit ("1 pm", "1%").
const ONE_UNITS: &[&str] = &[
    "percent", "dollar", "am", "pm", "a.m", "p.m", "o'clock", "million", "billion", "gb", "tb",
];
/// Words that link a number to one before it: "5 minutes instead of ten", "from 2 to three".
const LINKS: &[&str] = &[
    "of", "than", "to", "or", "and", "from", "not", "vs", "versus", "between", "but",
];
/// Words before a time like "three thirty" that make it a clock time.
const TIME_WORDS: &[&str] = &[
    "at", "by", "around", "till", "until", "from", "before", "after", "about",
];
const ORDINALS: &[&str] = &[
    "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth",
];
/// How close (in words) a number must be to a converted one to count as compared with it.
const LINK_REACH: usize = 4;

struct Tok<'a> {
    pre: &'a str,
    core: &'a str,
    post: &'a str,
    lower: String,
}

fn split(tok: &str) -> Tok<'_> {
    let start = tok.find(char::is_alphanumeric).unwrap_or(tok.len());
    let end = tok
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric())
        .map_or(start, |(i, c)| i + c.len_utf8());
    let core = &tok[start..end.max(start)];
    Tok {
        pre: &tok[..start],
        core,
        post: &tok[end.max(start)..],
        lower: core.to_lowercase(),
    }
}

/// 0–99 from one word: "seven", "fifteen", "forty", "twenty-five".
fn small(w: &str) -> Option<u64> {
    if let Some(i) = ONES.iter().position(|o| *o == w) {
        return Some(i as u64);
    }
    if let Some(i) = TENS.iter().position(|t| *t == w) {
        return Some(20 + 10 * i as u64);
    }
    let (t, o) = w.split_once('-')?;
    let tens = 20 + 10 * TENS.iter().position(|x| *x == t)? as u64;
    let ones = ONES
        .iter()
        .position(|x| *x == o)
        .filter(|&i| (1..10).contains(&i))? as u64;
    Some(tens + ones)
}

/// A two-digit group starting at `i` ("thirty", "forty five", "fifteen", "oh five"):
/// its value and length in words. Stops at punctuation.
fn pair(toks: &[Tok], i: usize) -> Option<(u64, usize)> {
    let t = toks.get(i)?;
    if !t.pre.is_empty() {
        return None;
    }
    if t.lower == "oh" && t.post.is_empty() {
        let v = small(&toks.get(i + 1)?.lower).filter(|v| (1..10).contains(v))?;
        return Some((v, 2));
    }
    let v = small(&t.lower).filter(|v| *v >= 10)?;
    if v >= 20
        && v % 10 == 0
        && t.post.is_empty()
        && let Some(o) = toks.get(i + 1).and_then(|n| small(&n.lower))
        && (1..10).contains(&o)
        && toks[i + 1].pre.is_empty()
    {
        return Some((v + o, 2));
    }
    Some((v, 1))
}

#[derive(Clone, Copy, PartialEq)]
enum Last {
    Ones,
    Teen,
    Tens,
    Hundred,
    Thousand,
}

/// A spelled-out number starting at `i`: its value and length in words.
fn number(toks: &[Tok], i: usize) -> Option<(u64, usize)> {
    let mut j = i;
    let (mut total, mut current) = (0u64, 0u64);
    let mut last = None;
    // "a hundred", "a thousand"
    if toks[i].lower == "a"
        && toks[i].post.is_empty()
        && matches!(
            toks.get(i + 1).map(|t| t.lower.as_str()),
            Some("hundred" | "thousand")
        )
    {
        current = 1;
        last = Some(Last::Ones);
        j += 1;
    }
    while let Some(t) = toks.get(j) {
        if j > i && !t.pre.is_empty() {
            break;
        }
        let w = t.lower.as_str();
        if let Some(v) = small(w) {
            let fits = match last {
                None | Some(Last::Hundred | Last::Thousand) => true,
                Some(Last::Tens) => (1..10).contains(&v),
                _ => false,
            };
            if !fits {
                break;
            }
            current += v;
            last = Some(if v >= 20 && v % 10 == 0 {
                Last::Tens
            } else if (10..20).contains(&v) {
                Last::Teen
            } else {
                Last::Ones
            });
        } else if w == "hundred" && matches!(last, Some(Last::Ones | Last::Teen)) && current < 100 {
            current *= 100;
            last = Some(Last::Hundred);
        } else if w == "thousand" && last.is_some() && last != Some(Last::Thousand) && total == 0 {
            total = current * 1000;
            current = 0;
            last = Some(Last::Thousand);
        } else if w == "and"
            && matches!(last, Some(Last::Hundred | Last::Thousand))
            && t.post.is_empty()
            && toks.get(j + 1).is_some_and(|n| small(&n.lower).is_some())
        {
            // "one hundred and twenty"
        } else {
            break;
        }
        j += 1;
        if !t.post.is_empty() {
            break;
        }
    }
    (last.is_some() && j > i).then_some((total + current, j - i))
}

fn with_commas(n: u64) -> String {
    let s = n.to_string();
    if n < 10_000 {
        return s;
    }
    let mut out = String::new();
    for (k, c) in s.chars().enumerate() {
        if k > 0 && (s.len() - k).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn ends_sentence(t: &Tok) -> bool {
    t.post.contains(['.', '?', '!', '\n'])
}

/// Writes spoken numbers as digits, line by line so lists and paragraphs stay as they are.
pub fn to_digits(text: &str) -> String {
    text.split('\n').map(line).collect::<Vec<_>>().join("\n")
}

fn line(text: &str) -> String {
    let raw: Vec<&str> = text.split(' ').collect();
    let toks: Vec<Tok> = raw.iter().map(|t| split(t)).collect();
    let mut out: Vec<String> = Vec::new();
    // The last word index a number was converted at, to catch "5 minutes instead of ten".
    let mut last_converted: Option<usize> = None;
    let mut i = 0;
    while i < toks.len() {
        // "plus nine one" → +91: a dialling code, said digit by digit.
        if toks[i].lower == "plus" && toks[i].post.is_empty() {
            let digits: String = toks[i + 1..]
                .iter()
                .take(3)
                .map_while(|t| ONES[..10].iter().position(|o| *o == t.lower))
                .map(|d| char::from(b'0' + d as u8))
                .collect();
            if !digits.is_empty() {
                let end = i + digits.len();
                out.push(format!("{}+{}{}", toks[i].pre, digits, toks[end].post));
                last_converted = Some(end);
                i = end + 1;
                continue;
            }
        }
        match convert(&toks, i, last_converted) {
            Some((digits, used)) => {
                let end = i + used - 1;
                out.push(format!("{}{}{}", toks[i].pre, digits, toks[end].post));
                last_converted = Some(end);
                i += used;
            }
            None => {
                out.push(raw[i].to_string());
                i += 1;
            }
        }
    }
    out.join(" ")
}

/// The digits for a number starting at word `i`, and how many words they replace.
fn convert(toks: &[Tok], i: usize, last_converted: Option<usize>) -> Option<(String, usize)> {
    let t = &toks[i];
    let prev = i.checked_sub(1).map(|p| &toks[p]);
    let sentence_start = prev.is_none_or(ends_sentence);
    // "Seven Eleven", "Ocean's Eleven": a capitalised number mid-sentence is a name.
    if !sentence_start && t.core.starts_with(char::is_uppercase) {
        return None;
    }
    let next_word = |k: usize| toks.get(k).map(|n| n.lower.as_str()).unwrap_or("");

    // Years: "twenty twenty six" → 2026, "nineteen ninety" → 1990.
    if matches!(t.lower.as_str(), "nineteen" | "twenty")
        && t.post.is_empty()
        && let Some((v, n)) = pair(toks, i + 1)
    {
        let century = small(&t.lower)?;
        return Some(((century * 100 + v).to_string(), 1 + n));
    }
    // Clock times: "three thirty pm", "at eleven fifteen" → 3:30 pm, 11:15.
    if let Some(h) = small(&t.lower).filter(|h| (1..=12).contains(h))
        && t.post.is_empty()
        && let Some((m, n)) = pair(toks, i + 1).filter(|(m, _)| *m < 60)
    {
        let after = next_word(i + 1 + n);
        let before = prev.map(|p| p.lower.as_str()).unwrap_or("");
        let ends_clean = toks[i + n].post.is_empty();
        if (ends_clean && matches!(after, "am" | "pm" | "a.m" | "p.m"))
            || TIME_WORDS.contains(&before)
        {
            return Some((format!("{h}:{m:02}"), 1 + n));
        }
    }

    let (value, mut used) = number(toks, i)?;
    let end = i + used - 1;
    let after = next_word(end + 1);
    if ORDINALS.contains(&after)
        || after.ends_with("th") && small(after.trim_end_matches("th")).is_some()
    {
        return None; // "twenty first" is a date, not 20 and a first
    }
    let open = toks[end].post.is_empty();
    let has_unit = open && UNITS.contains(&after);
    let linked = last_converted.is_some_and(|l| i - l <= LINK_REACH)
        && prev.is_some_and(|p| LINKS.contains(&p.lower.as_str()));

    // "two point five" → 2.5
    let mut digits = with_commas(value);
    let mut decimal = false;
    if open && after == "point" && toks[end + 1].post.is_empty() {
        let mut decimals = String::new();
        let mut k = end + 2;
        while let Some(d) = toks.get(k) {
            let v = if d.lower == "oh" {
                Some(0)
            } else {
                small(&d.lower).filter(|v| *v < 10)
            };
            match v {
                Some(v) if d.pre.is_empty() => decimals.push_str(&v.to_string()),
                _ => break,
            }
            k += 1;
            if !d.post.is_empty() {
                break;
            }
        }
        if !decimals.is_empty() {
            digits = format!("{value}.{decimals}");
            used = k - i;
            decimal = true;
        }
    }

    let keep = if decimal || used > 1 || value >= 10 {
        true
    } else if value == 1 {
        has_unit && ONE_UNITS.contains(&after)
    } else {
        has_unit || linked
    };
    if !keep {
        return None;
    }
    let end = i + used - 1;
    let after = next_word(end + 1);
    if toks[end].post.is_empty() {
        let merge = match after {
            "percent" => Some(format!("{digits}%")),
            "dollar" | "dollars" | "bucks" => Some(format!("${digits}")),
            _ => None,
        };
        if let Some(m) = merge {
            // The unit's punctuation ("…percent.") goes with it; take it as part of the span.
            return Some((m, used + 1));
        }
    }
    Some((digits, used))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialling_codes_become_digits() {
        assert_eq!(
            to_digits("numbers that start with plus nine one and then ten digits"),
            "numbers that start with +91 and then 10 digits"
        );
        assert_eq!(to_digits("call plus four four, then"), "call +44, then");
        assert_eq!(to_digits("plus the fee"), "plus the fee");
    }

    #[test]
    fn quantities_become_digits() {
        assert_eq!(
            to_digits("Why do tokens expire after five minutes instead of thirty?"),
            "Why do tokens expire after 5 minutes instead of 30?"
        );
        assert_eq!(
            to_digits("It's twelve thousand four hundred dollars and retainage is ten percent."),
            "It's $12,400 and retainage is 10%."
        );
        assert_eq!(to_digits("Two hours, not three."), "2 hours, not 3.");
        assert_eq!(
            to_digits("about a hundred and twenty people"),
            "about 120 people"
        );
        assert_eq!(
            to_digits("round to two point five seconds"),
            "round to 2.5 seconds"
        );
        assert_eq!(to_digits("five million users"), "5 million users");
    }

    #[test]
    fn times_and_years() {
        assert_eq!(
            to_digits("Let's meet at three thirty pm."),
            "Let's meet at 3:30 pm."
        );
        assert_eq!(to_digits("at eleven fifteen"), "at 11:15");
        assert_eq!(to_digits("Call me at three pm."), "Call me at 3 pm.");
        assert_eq!(to_digits("back in twenty twenty six"), "back in 2026");
        assert_eq!(to_digits("since nineteen ninety"), "since 1990");
    }

    #[test]
    fn words_that_arent_quantities_stay() {
        for s in [
            "One of them is wrong.",
            "No one came.",
            "I work nine to five.",
            "Can you give me one more minute?",
            "the twenty first of May",
            "We watched Ocean's Eleven.",
            "There are three things to fix.",
            "a hundredth time",
        ] {
            assert_eq!(to_digits(s), s);
        }
    }

    #[test]
    fn keeps_lines() {
        assert_eq!(
            to_digits("Do this:\n1. Wait ten minutes.\n2. Retry three times."),
            "Do this:\n1. Wait 10 minutes.\n2. Retry three times."
        );
    }
}
