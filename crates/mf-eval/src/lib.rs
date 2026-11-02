//! Scoring for MuskFlow benchmarks.
//!
//! [`normalize`] maps spoken and written forms onto the same words ("90%" and
//! "ninety percent", "$12,400" and "twelve thousand four hundred dollars") so
//! that word error rate measures recognition, not formatting. Fillers are
//! dropped on both sides: removing them is the cleanup LLM's job, not the ASR's.

/// Hesitations that don't count towards WER.
const FILLERS: &[&str] = &["uh", "um", "ah", "er", "erm", "hm", "hmm", "mm", "mhm"];

/// Word error rate: (substitutions + deletions + insertions) / reference words.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Wer {
    pub errors: usize,
    pub ref_words: usize,
}

impl Wer {
    pub fn rate(&self) -> f32 {
        if self.ref_words == 0 {
            return if self.errors == 0 { 0.0 } else { 1.0 };
        }
        self.errors as f32 / self.ref_words as f32
    }
}

impl std::ops::Add for Wer {
    type Output = Wer;
    fn add(self, o: Wer) -> Wer {
        Wer {
            errors: self.errors + o.errors,
            ref_words: self.ref_words + o.ref_words,
        }
    }
}

pub fn wer(reference: &str, hypothesis: &str) -> Wer {
    let r = normalize(reference);
    let h = normalize(hypothesis);
    Wer {
        errors: edit_distance(&r, &h),
        ref_words: r.len(),
    }
}

fn edit_distance(a: &[String], b: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(x != y);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Lowercased words with numbers spelled out, symbols expanded, punctuation and fillers removed.
pub fn normalize(text: &str) -> Vec<String> {
    let lower = text
        .to_lowercase()
        .replace("a.m.", "am")
        .replace("p.m.", "pm");
    // "12,400" → "12400" before commas become word breaks.
    let mut s = String::with_capacity(lower.len());
    let chars: Vec<char> = lower.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let between_digits = i > 0
            && i + 1 < chars.len()
            && chars[i - 1].is_ascii_digit()
            && chars[i + 1].is_ascii_digit();
        if c == ',' && between_digits {
            continue;
        }
        match c {
            '%' => s.push_str(" percent "),
            '&' => s.push_str(" and "),
            '\'' | '’' => {} // "let's" ≡ "lets", "I'm" ≡ "im"
            c if c.is_alphanumeric() || c == '$' => s.push(c),
            _ => s.push(' '),
        }
    }

    let mut out = Vec::new();
    for raw in s.split_whitespace() {
        let (dollars, tok) = match raw.strip_prefix('$') {
            Some(rest) => (true, rest),
            None => (false, raw),
        };
        for part in split_alpha_digits(tok) {
            expand(&part, &mut out);
        }
        if dollars {
            out.push("dollars".into());
        }
    }
    out.retain(|w| !FILLERS.contains(&w.as_str()));
    for w in &mut out {
        if w == "ok" {
            *w = "okay".into();
        }
    }
    out
}

/// "q3" → ["q", "3"], "15th" stays whole (ordinal), "3pm" → ["3", "pm"].
fn split_alpha_digits(tok: &str) -> Vec<String> {
    if ordinal_value(tok).is_some() {
        return vec![tok.to_string()];
    }
    let mut parts: Vec<String> = Vec::new();
    for c in tok.chars() {
        let digit = c.is_ascii_digit();
        match parts.last_mut() {
            Some(p)
                if p.chars()
                    .last()
                    .is_some_and(|l| l.is_ascii_digit() == digit) =>
            {
                p.push(c)
            }
            _ => parts.push(c.to_string()),
        }
    }
    parts
}

fn ordinal_value(tok: &str) -> Option<u64> {
    let n = tok
        .strip_suffix("st")
        .or_else(|| tok.strip_suffix("nd"))
        .or_else(|| tok.strip_suffix("rd"))
        .or_else(|| tok.strip_suffix("th"))?;
    if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
}

fn expand(tok: &str, out: &mut Vec<String>) {
    if let Some(n) = ordinal_value(tok) {
        let mut words = number_words(n);
        if let Some(last) = words.pop() {
            words.push(ordinal_word(&last));
        }
        out.extend(words);
    } else if tok.chars().all(|c| c.is_ascii_digit()) {
        match tok.parse::<u64>() {
            Ok(n) => out.extend(number_words(n)),
            Err(_) => out.push(tok.to_string()),
        }
    } else {
        out.push(tok.to_string());
    }
}

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
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

fn number_words(n: u64) -> Vec<String> {
    if n == 0 {
        return vec!["zero".into()];
    }
    let mut out = Vec::new();
    let mut rest = n;
    for (scale, name) in [
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1_000, "thousand"),
    ] {
        if rest >= scale {
            out.extend(below_thousand(rest / scale));
            out.push(name.to_string());
            rest %= scale;
        }
    }
    if rest > 0 {
        out.extend(below_thousand(rest));
    }
    out
}

fn below_thousand(n: u64) -> Vec<String> {
    let mut out = Vec::new();
    let (h, r) = (n / 100, n % 100);
    if h > 0 {
        out.push(ONES[h as usize].to_string());
        out.push("hundred".into());
    }
    if r >= 20 {
        out.push(TENS[(r / 10) as usize].to_string());
        if r % 10 > 0 {
            out.push(ONES[(r % 10) as usize].to_string());
        }
    } else if r > 0 {
        out.push(ONES[r as usize].to_string());
    }
    out
}

fn ordinal_word(w: &str) -> String {
    match w {
        "one" => "first".into(),
        "two" => "second".into(),
        "three" => "third".into(),
        "five" => "fifth".into(),
        "eight" => "eighth".into(),
        "nine" => "ninth".into(),
        "twelve" => "twelfth".into(),
        w if w.ends_with('y') => format!("{}ieth", &w[..w.len() - 1]),
        w => format!("{w}th"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> String {
        normalize(s).join(" ")
    }

    #[test]
    fn written_and_spoken_forms_match() {
        assert_eq!(n("like 90%,"), n("like ninety percent"));
        assert_eq!(n("$12,400"), "twelve thousand four hundred dollars");
        assert_eq!(
            n("due on the 15th of March"),
            n("due on the fifteenth of march")
        );
        assert_eq!(n("the Q3 roadmap"), n("the Q three roadmap"));
        assert_eq!(n("at 3 p.m."), n("at three pm"));
        assert_eq!(n("Let's go, OK?"), "lets go okay");
    }

    #[test]
    fn fillers_do_not_count() {
        assert_eq!(n("Um, so uh hey"), "so hey");
    }

    #[test]
    fn wer_counts_edits_against_reference_length() {
        let w = wer(
            "tell Gaurav that the migration is done",
            "tell Garov that migration is done now",
        );
        // Garov (sub), the (del), now (ins)
        assert_eq!(
            w,
            Wer {
                errors: 3,
                ref_words: 7
            }
        );
    }
}
