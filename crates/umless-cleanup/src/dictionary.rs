//! The user's dictionary: names, products and jargon, spelled the way they want.
//!
//! Speech models usually *hear* a rare name right and only *spell* it wrong
//! ("Subflow" for SubFlo, "firman" for foreman). So instead of biasing the
//! decoder (fragile: a strong boost makes it hallucinate the term) we fix the
//! spelling afterwards: a run of transcript words is replaced by a dictionary
//! term when it sounds the same and is spelled nearly the same. This works the
//! same on every tier, whatever the ASR backend.

/// Edits allowed per letter of the term (rounded down), when the two sound alike.
const MAX_EDIT_SHARE: f32 = 0.34;
/// Terms at least this many letters long may differ by one consonant sound.
const LONG_TERM: usize = 8;

#[derive(Debug, Clone, Default)]
pub struct Dictionary {
    terms: Vec<Term>,
    /// What the speech model hears instead of a term, written by the user as
    /// "Gaurav = Gorov, Barraf": exact word runs (lowercase, no punctuation) → term.
    aliases: Vec<(Vec<String>, String)>,
}

#[derive(Debug, Clone)]
struct Term {
    text: String,
    words: usize,
    letters: String,
    sound: String,
    /// Read off the screen (a name in the email thread) rather than the user's own list:
    /// it only fixes a misspelling, never re-cases a word ("mark" stays a verb).
    screen: bool,
}

impl Dictionary {
    pub fn new<S: AsRef<str>>(terms: impl IntoIterator<Item = S>) -> Self {
        let mut terms: Vec<Term> = terms
            .into_iter()
            .map(|t| t.as_ref().trim().to_string())
            .filter(|t| !t.is_empty())
            .map(|text| Term {
                words: text.split_whitespace().count(),
                letters: letters(&text),
                sound: sound(&text),
                screen: false,
                text,
            })
            .filter(|t| t.letters.len() >= 3)
            .collect();
        // Longer terms first, so "lien waivers" wins over "lien".
        terms.sort_by_key(|t| std::cmp::Reverse(t.letters.len()));
        Self {
            terms,
            aliases: Vec::new(),
        }
    }

    /// One term per line; blank lines and `#` comments are ignored. "Term = heard, heard"
    /// also lists what the speech model types instead of it.
    pub fn parse(text: &str) -> Self {
        let lines: Vec<&str> = text
            .lines()
            .map(|l| l.split('#').next().unwrap_or("").trim())
            .filter(|l| !l.is_empty())
            .collect();
        let mut dict = Self::new(
            lines
                .iter()
                .map(|l| l.split('=').next().unwrap_or("").trim()),
        );
        for line in lines {
            let Some((term, heard)) = line.split_once('=') else {
                continue;
            };
            for alias in heard.split(',') {
                let words: Vec<String> = alias.split_whitespace().map(alias_word).collect();
                if !words.is_empty() && !term.trim().is_empty() {
                    dict.aliases.push((words, term.trim().to_string()));
                }
            }
        }
        // Longest first, so "Jonathan Mishla" wins over "Jonathan".
        dict.aliases
            .sort_by_key(|(w, _)| std::cmp::Reverse(w.len()));
        dict
    }

    /// Only the "heard as" entries: no near-miss respelling of the terms themselves.
    pub fn without_terms(mut self) -> Self {
        self.terms.clear();
        self
    }

    /// This dictionary with more terms; its "heard as" entries are kept.
    pub fn with_terms<S: AsRef<str>>(&self, extra: impl IntoIterator<Item = S>) -> Self {
        let mut d = Self::new(
            self.terms
                .iter()
                .filter(|t| !t.screen)
                .map(|t| t.text.clone())
                .chain(extra.into_iter().map(|s| s.as_ref().to_string())),
        );
        d.aliases = self.aliases.clone();
        d
    }

    /// This dictionary plus names read off the screen. The user's own terms win a tie.
    pub fn with_screen_terms<S: AsRef<str>>(&self, names: impl IntoIterator<Item = S>) -> Self {
        let mut extra = Self::new(names).terms;
        extra.retain(|t| !self.terms.iter().any(|u| u.letters == t.letters));
        for t in &mut extra {
            t.screen = true;
        }
        let mut terms = self.terms.clone();
        terms.extend(extra);
        terms.sort_by_key(|t| (std::cmp::Reverse(t.letters.len()), t.screen));
        Self {
            terms,
            aliases: self.aliases.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.aliases.is_empty()
    }

    pub fn terms(&self) -> impl Iterator<Item = &str> {
        self.terms.iter().map(|t| t.text.as_str())
    }

    /// Respells transcript words that are a near miss for a dictionary term.
    pub fn apply(&self, text: &str) -> String {
        let text = &self.apply_aliases(text);
        if self.terms.is_empty() {
            return text.to_string();
        }
        let toks: Vec<&str> = text.split_whitespace().collect();
        let mut out: Vec<String> = Vec::with_capacity(toks.len());
        let mut i = 0;
        while i < toks.len() {
            match self.best_at(&toks, i) {
                // If the same term matches better one word later, this word isn't part of it
                // ("to turn a construction" → keep "to").
                Some(m)
                    if !self
                        .best_at(&toks, i + 1)
                        .is_some_and(|next| next.term == m.term && next.edits < m.edits) =>
                {
                    out.push(respell(&toks[i..i + m.len], &self.terms[m.term].text));
                    i += m.len;
                }
                _ => {
                    out.push(toks[i].to_string());
                    i += 1;
                }
            }
        }
        out.join(" ")
    }

    /// Replaces each "heard as" run with its term, keeping the punctuation around it.
    fn apply_aliases(&self, text: &str) -> String {
        if self.aliases.is_empty() {
            return text.to_string();
        }
        let toks: Vec<&str> = text.split_whitespace().collect();
        let words: Vec<String> = toks.iter().map(|t| alias_word(t)).collect();
        let mut out: Vec<String> = Vec::with_capacity(toks.len());
        let mut i = 0;
        'next: while i < toks.len() {
            for (heard, term) in &self.aliases {
                let n = heard.len();
                // Only the last word of the run may carry punctuation after it.
                if i + n <= toks.len()
                    && words[i..i + n] == heard[..]
                    && toks[i..i + n - 1]
                        .iter()
                        .all(|t| t.ends_with(char::is_alphanumeric))
                {
                    out.push(respell(&toks[i..i + n], term));
                    i += n;
                    continue 'next;
                }
            }
            out.push(toks[i].to_string());
            i += 1;
        }
        out.join(" ")
    }

    /// The closest term starting at word `i`: fewest edits, then the longest term.
    fn best_at(&self, toks: &[&str], i: usize) -> Option<Match> {
        let mut best: Option<Match> = None;
        for (t, term) in self.terms.iter().enumerate() {
            // The ASR may split a term ("Sub flow") or merge one ("lienwaivers").
            for len in [term.words, term.words + 1, term.words.saturating_sub(1)] {
                if len == 0 || i + len > toks.len() {
                    continue;
                }
                let window = &toks[i..i + len];
                // A clause break inside the window means these aren't one term.
                if window[..len - 1]
                    .iter()
                    .any(|w| w.ends_with(['.', ',', '?', '!', ';', ':']))
                {
                    continue;
                }
                if let Some(edits) = edits_to(term, &window.join(" ")) {
                    // Terms are sorted longest first, so `<` keeps the longer on a tie.
                    if best.is_none_or(|b| edits < b.edits) {
                        best = Some(Match {
                            term: t,
                            len,
                            edits,
                        });
                    }
                }
            }
        }
        best
    }
}

#[derive(Debug, Clone, Copy)]
struct Match {
    term: usize,
    /// Transcript words covered.
    len: usize,
    edits: usize,
}

/// Letter edits from `said` to the term, if it's a plausible mishearing of it.
fn edits_to(term: &Term, said: &str) -> Option<usize> {
    let l = letters(said);
    if term.screen && (l == term.letters || crate::guard::is_everyday_word(&l)) {
        return None;
    }
    if l == term.letters {
        return Some(0); // only case or spacing differs
    }
    // An inflection of the term is a different word: "pay applications" stays plural.
    if let Some(rest) = l.strip_prefix(term.letters.as_str())
        && matches!(rest, "s" | "es" | "ed" | "ing")
    {
        return None;
    }
    if l.chars().next() != term.letters.chars().next() {
        return None;
    }
    // Long terms may lose or gain one consonant ("turn a construction").
    let sound_slack = usize::from(term.letters.len() >= LONG_TERM);
    if levenshtein(&sound(said), &term.sound) > sound_slack {
        return None;
    }
    let allowed = (term.letters.chars().count() as f32 * MAX_EDIT_SHARE) as usize;
    let edits = levenshtein(&l, &term.letters);
    (edits <= allowed.max(1)).then_some(edits)
}

/// The term, keeping the window's surrounding punctuation and a sentence-initial capital.
fn alias_word(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

fn respell(window: &[&str], term: &str) -> String {
    let first = window[0];
    let last = window[window.len() - 1];
    let lead: String = first.chars().take_while(|c| !c.is_alphanumeric()).collect();
    let trail: String = {
        let t: Vec<char> = last
            .chars()
            .rev()
            .take_while(|c| !c.is_alphanumeric())
            .collect();
        t.into_iter().rev().collect()
    };
    // "'s" and friends belong to the word, not the term.
    let core = last.trim_end_matches(|c: char| !c.is_alphanumeric());
    // A term ending in "s" absorbs it: "postger's" is Postgres, not "Postgres's".
    let possessive = ["'s", "’s"]
        .iter()
        .find(|s| core.to_lowercase().ends_with(*s) && !term.ends_with('s'))
        .map_or("", |s| *s);
    let mut t = term.to_string();
    // Title case ("Firman, call me") means a sentence start; ALL CAPS is just ASR noise.
    let mut alpha = first.chars().filter(|c| c.is_alphabetic());
    let starts_upper = alpha.next().is_some_and(char::is_uppercase)
        && (first.chars().filter(|c| c.is_alphabetic()).count() == 1
            || alpha.any(char::is_lowercase));
    if starts_upper {
        let mut c = t.chars();
        if let Some(f) = c.next() {
            t = f.to_uppercase().chain(c).collect();
        }
    }
    format!("{lead}{t}{possessive}{trail}")
}

/// Lowercase letters and digits only.
fn letters(s: &str) -> String {
    let s = s.trim_end_matches(|c: char| !c.is_alphanumeric());
    let s = s
        .strip_suffix("'s")
        .or_else(|| s.strip_suffix("’s"))
        .unwrap_or(s);
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A rough consonant skeleton: first letter, then consonants with sound-alikes
/// merged and repeats collapsed. "foreman", "firman" and "firmman" all give "frmn".
fn sound(s: &str) -> String {
    let l = letters(s).replace("ph", "f").replace("ck", "k");
    let mut out = String::new();
    for (i, c) in l.chars().enumerate() {
        let c = match c {
            'c' | 'q' => 'k',
            'z' => 's',
            _ => c,
        };
        if i > 0 && "aeiouyhw".contains(c) {
            continue;
        }
        if out.ends_with(c) {
            continue;
        }
        out.push(c);
    }
    out
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push(
                (prev[j] + usize::from(ca != cb))
                    .min(prev[j + 1] + 1)
                    .min(cur[j] + 1),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_names_fix_misspellings_only() {
        let d = Dictionary::new(["SubFlo"]).with_screen_terms(["Priya", "Mark", "Gaurav"]);
        assert_eq!(
            d.apply("tell pria and garv to mark the subflow invoice"),
            "tell Priya and Gaurav to mark the SubFlo invoice"
        );
    }

    fn dict() -> Dictionary {
        Dictionary::new(["SubFlo", "Jatin", "foreman", "lien waivers", "Kubernetes"])
    }

    #[test]
    fn fixes_near_miss_spellings() {
        let d = dict();
        assert_eq!(
            d.apply("Hi, I'm Jatan, founder of Subflow."),
            "Hi, I'm Jatin, founder of SubFlo."
        );
        assert_eq!(
            d.apply("it talks to the firmman and handles lean waivers"),
            "it talks to the foreman and handles lien waivers"
        );
        assert_eq!(d.apply("Sub flow's agent"), "SubFlo's agent");
        let d2 = Dictionary::new(["pay application", "Postgres"]);
        assert_eq!(
            d2.apply("handles pay applications"),
            "handles pay applications"
        );
        assert_eq!(d2.apply("the postger's replica"), "the Postgres replica");
        assert_eq!(d.apply("Firman, call me."), "Foreman, call me.");
        assert_eq!(d.apply("the LEAN waivers"), "the lien waivers");
        assert_eq!(
            Dictionary::new(["Turner Construction"]).apply("go to turn a construction today"),
            "go to Turner Construction today"
        );
        let d = Dictionary::new(["lien waiver", "lien waivers", "Turner Construction"]);
        assert_eq!(d.apply("the Lian waiver, so"), "the Lien waiver, so");
        assert_eq!(
            d.apply("can go to Turner Construction"),
            "can go to Turner Construction"
        );
    }

    #[test]
    fn leaves_other_words_alone() {
        let d = dict();
        for s in [
            "the former owner",
            "a satin finish",
            "we lean on them",
            "sub, flow",
            "the subway is slow",
        ] {
            assert_eq!(d.apply(s), s);
        }
    }

    #[test]
    fn parses_a_dictionary_file() {
        let d = Dictionary::parse("# names\nSubFlo\n\nlien waivers  # construction\n");
        assert_eq!(d.terms().collect::<Vec<_>>(), ["lien waivers", "SubFlo"]);
    }

    #[test]
    fn heard_as_entries_are_replaced_exactly() {
        let d = Dictionary::parse(
            "Gaurav = Gorov, Barraf\nJatin Bishla = Jonathan Mishla\nretainage = it and age\nSubFlo",
        );
        assert_eq!(
            d.apply("Barraf, can you take it"),
            "Gaurav, can you take it"
        );
        assert_eq!(
            d.apply("I'm Jonathan Mishla, co founder of Subflow"),
            "I'm Jatin Bishla, co founder of SubFlo"
        );
        assert_eq!(
            d.apply("wait 60 days for it and age."),
            "wait 60 days for retainage."
        );
        // A run interrupted by punctuation is not the alias.
        assert_eq!(d.apply("for it. And age is"), "for it. And age is");
        // Adding terms keeps the heard-as entries.
        assert_eq!(d.with_terms(["Priya"]).apply("Gorov"), "Gaurav");
    }
}
