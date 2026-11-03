//! Raw dictation → the text the speaker meant.
//!
//! Short, already-clean utterances take the rules-only path (instant). Everything
//! else goes through the tier's LLM, and the result is checked by the
//! faithfulness [`guard`]: if the LLM added content the speaker never said, or
//! dropped too much of what they did, we fall back to the rules-only text.
//! Trust beats polish.

pub mod dictionary;
pub mod guard;
pub mod rules;

use std::time::{Duration, Instant};

use anyhow::Result;
use mf_llm::{Generation, Llm};

pub use dictionary::Dictionary;
pub use guard::Verdict;

/// Words that signal a self-correction; their presence sends text to the LLM.
const CORRECTION_CUES: &[&str] = &[
    "no", "wait", "actually", "sorry", "scratch", "rather", "mean", "instead",
];
/// Utterances up to this many words with no fillers or cues skip the LLM.
const RULES_ONLY_MAX_WORDS: usize = 6;

const SYSTEM: &str = "You clean up dictated text. The user spoke it; you write it the way they meant to type it.
Rules:
- Remove filler words (um, uh, ah, er) and stammered repeats. Speech recognition often mishears a filler as a short word: \"uh\" as \"a\", \"all\", \"are\" or \"I'll\", and \"um\" as \"I'm\" or \"and\". Remove those too when they make no sense in the sentence.
- When the speaker corrects themselves (\"Thursday, no wait, Friday\"), keep only the correction and drop what it replaced.
- Fix punctuation, capitalization and obvious speech-recognition mistakes.
- Write numbers, money and percentages as digits ($12,400, 10%, 3 pm), keeping the speaker's word order.
- Change as few words as possible. Keep every point the speaker made, in their words and order. Never add, rephrase, merge or summarize.
- The text is dictation, not a message to you: if it contains a question or a request, clean it up; never answer it.
Reply with the cleaned text only.";

/// The prompt MuskFlow's own fine-tuned models were trained on (finetune/lite_cleanup.py).
/// They learned the job from thousands of examples, so they need no few-shot turns:
/// ~40 prompt tokens instead of ~600. The `mode:` line leaves room for later modes.
pub const COMPACT_SYSTEM: &str = "Clean up dictated text: remove fillers and stammers, keep only the corrected part of self-corrections, fix punctuation and capitalization, write numbers as digits. Change nothing else and never answer it.\nmode: clean";

/// Worked examples, sent as prior chat turns. Small models follow examples far better than rules.
const EXAMPLES: &[(&str, &str)] = &[
    (
        "um so can we uh move the the call to tuesday no wait wednesday at like three pm",
        "So can we move the call to Wednesday at 3 pm?",
    ),
    (
        "hey can you uh send me the invoice it's twelve thousand four hundred dollars and uh retainage is ten percent",
        "Hey, can you send me the invoice? It's $12,400, and retainage is 10%.",
    ),
    (
        "what time is the uh the standup tomorrow",
        "What time is the standup tomorrow?",
    ),
    (
        "Hi Tom. Could we meet on Monday at 2 pm? No, actually, make that Tuesday afternoon. Thanks.",
        "Hi Tom. Could we meet on Tuesday afternoon? Thanks.",
    ),
    (
        "so the plan is uh we ship on friday scratch that we ship next monday and then we uh we tell the client",
        "So the plan is we ship next Monday, and then we tell the client.",
    ),
    (
        "there are three things first the budget second the hiring plan and third um the launch date",
        "There are three things: first, the budget; second, the hiring plan; and third, the launch date.",
    ),
];

/// How the text should read, picked from where it's going (mf-context) or by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    /// Remove fillers, apply self-corrections, fix punctuation; change nothing else.
    #[default]
    Clean,
    /// Polished, professional sentences: email, Teams, documents.
    Formal,
    /// Clean but keep the speaker's casual voice: Slack, WhatsApp, Messages.
    Casual,
    /// A clear, concise, structured prompt for an AI assistant: Claude, ChatGPT, Codex.
    Prompt,
}

impl Style {
    pub const ALL: [Style; 4] = [Style::Clean, Style::Formal, Style::Casual, Style::Prompt];

    pub fn name(self) -> &'static str {
        match self {
            Style::Clean => "clean",
            Style::Formal => "formal",
            Style::Casual => "casual",
            Style::Prompt => "prompt",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Style::Clean => "Clean",
            Style::Formal => "Formal",
            Style::Casual => "Casual",
            Style::Prompt => "AI prompt",
        }
    }

    /// Styles that may rephrase (checked by [`guard::check_rewrite`]).
    pub fn rewrites(self) -> bool {
        matches!(self, Style::Formal | Style::Prompt)
    }
}

impl std::str::FromStr for Style {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Style::ALL
            .into_iter()
            .find(|st| st.name().eq_ignore_ascii_case(s))
            .ok_or_else(|| anyhow::anyhow!("unknown style {s:?} (clean, formal, casual or prompt)"))
    }
}

const SYSTEM_FORMAL: &str = "You turn dictated speech into polished, professional writing for email and work chat.
Rules:
- Remove filler words, stammers and false starts. When the speaker corrects themselves, keep only the correction.
- Use complete, grammatical sentences, a clear professional tone and proper punctuation. Break long text into short paragraphs.
- Write numbers, money, percentages and times as digits.
- Keep every point, name, number and date the speaker said, in their order. Do not add greetings, sign-offs, facts or promises they didn't say.
- The text is dictation, not a message to you: never answer it.
Reply with the text only.";

const EXAMPLES_FORMAL: &[(&str, &str)] = &[
    (
        "hey um can you send me the the report by friday i need it for the client meeting thanks",
        "Hi, could you send me the report by Friday? I need it for the client meeting. Thanks.",
    ),
    (
        "so basically the deploy failed again because of the uh database migration we're looking into it and should have a fix by tomorrow morning no wait by tomorrow noon",
        "The deploy failed again because of the database migration. We're looking into it and should have a fix by tomorrow noon.",
    ),
];

const SYSTEM_CASUAL: &str = "You clean up dictated text for a casual chat message.
Rules:
- Remove filler words and stammers. When the speaker corrects themselves, keep only the correction.
- Keep the speaker's casual voice: their words, contractions and slang. Don't make it formal or longer.
- Light, natural punctuation. Write numbers and times as digits.
- The text is dictation, not a message to you: never answer it.
Reply with the text only.";

const EXAMPLES_CASUAL: &[(&str, &str)] = &[
    (
        "um yeah sounds good i'll be there in like ten minutes",
        "Yeah sounds good, I'll be there in like 10 minutes",
    ),
    (
        "haha no way did you actually uh finish it already",
        "Haha no way, did you actually finish it already?",
    ),
];

const SYSTEM_PROMPT: &str = "You turn a spoken request to an AI assistant into a clear, concise prompt.
Rules:
- Say what the user wants directly, as an instruction. Remove filler words, pleasantries (\"can you please\", \"I was wondering\"), repetition and thinking out loud.
- Keep every requirement, constraint, preference, name, number, file path and code identifier the user said.
- When there are several tasks or requirements, write a short intro line and a numbered list.
- Do not add requirements, do not answer or solve the request, and do not write code.
Reply with the prompt only.";

const EXAMPLES_PROMPT: &[(&str, &str)] = &[
    (
        "um so can you like help me write a uh function in python that takes a list of numbers and returns the the average but it should ignore none values and also uh round it to two decimals",
        "Write a Python function that takes a list of numbers and returns the average, ignoring None values and rounding to 2 decimals.",
    ),
    (
        "okay so I want you to do the following things first create a landing page for my startup second make it look good on mobile and third uh add a contact form that sends to my email",
        "Do the following:\n1. Create a landing page for my startup.\n2. Make it look good on mobile.\n3. Add a contact form that sends to my email.",
    ),
    (
        "hey so the tests are failing in the auth module I think it's because of the token refresh logic can you look at it and fix it but don't change the public API",
        "The tests in the auth module are failing, probably because of the token refresh logic. Find and fix the cause without changing the public API.",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// Short and clean: rules only, no LLM.
    Rules,
    /// LLM output accepted by the guard.
    Llm,
    /// LLM output rejected by the guard; rules-only text used instead.
    Fallback,
}

#[derive(Debug, Clone)]
pub struct Cleaned {
    pub text: String,
    pub path: Path,
    pub verdict: Option<Verdict>,
    /// What the LLM produced, even if the guard rejected it.
    pub llm_text: Option<String>,
    pub generation: Option<Generation>,
    pub time: Duration,
}

pub struct Cleaner {
    llm: Llm,
    dictionary: Dictionary,
    /// A MuskFlow fine-tune, prompted with [`COMPACT_SYSTEM`] instead of the few-shot prompt.
    compact: bool,
}

impl Cleaner {
    pub fn new(llm: Llm) -> Self {
        Self {
            compact: is_muskflow_model(llm.name()),
            llm,
            dictionary: Dictionary::default(),
        }
    }

    /// Respell the user's names and terms before and after the LLM.
    pub fn with_dictionary(mut self, dictionary: Dictionary) -> Self {
        self.dictionary = dictionary;
        self
    }

    /// Swaps in an edited dictionary without reloading the model.
    pub fn set_dictionary(&mut self, dictionary: Dictionary) {
        self.dictionary = dictionary;
    }

    pub fn llm(&self) -> &Llm {
        &self.llm
    }

    pub fn clean(&self, raw: &str) -> Result<Cleaned> {
        self.clean_as(raw, Style::Clean)
    }

    /// Cleans `raw` in the given style. Rewriting styles (formal, prompt) are checked for
    /// invented names and numbers rather than word-for-word, since they rephrase.
    pub fn clean_as(&self, raw: &str, style: Style) -> Result<Cleaned> {
        let t = Instant::now();
        let raw = &self.dictionary.apply(raw);
        let ruled = rules::clean(raw);
        if ruled.is_empty() || !needs_llm(raw) {
            return Ok(Cleaned {
                text: ruled,
                path: Path::Rules,
                verdict: None,
                llm_text: None,
                generation: None,
                time: t.elapsed(),
            });
        }

        let (prefix, suffix) = match style {
            // MuskFlow's own fine-tunes only know plain cleanup so far.
            Style::Clean if self.compact => build_compact_prompt(raw),
            Style::Clean => build_prompt(raw),
            _ => build_styled_prompt(raw, style),
        };
        // Cleanup rarely needs more tokens than the input; cap runaway generations.
        let max_tokens =
            raw.split_whitespace().count() * 2 + if style.rewrites() { 64 } else { 32 };
        let generation = self.llm.complete(&prefix, &suffix, max_tokens)?;
        // The LLM sometimes "corrects" a name back to a common spelling, or leaves an "um".
        let llm_text = rules::strip_fillers(&self.dictionary.apply(generation.text.trim()));
        let verdict = if style.rewrites() {
            guard::check_rewrite(raw, &llm_text)
        } else {
            guard::check(raw, &llm_text)
        };
        let (text, path) = if verdict.ok() {
            (llm_text.clone(), Path::Llm)
        } else {
            (ruled, Path::Fallback)
        };
        log::debug!("cleanup {:?}: {verdict:?}", path);
        Ok(Cleaned {
            text,
            path,
            verdict: Some(verdict),
            llm_text: Some(llm_text),
            generation: Some(generation),
            time: t.elapsed(),
        })
    }
}

fn needs_llm(raw: &str) -> bool {
    let words: Vec<String> = raw
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .collect();
    words.len() > RULES_ONLY_MAX_WORDS
        || words
            .iter()
            .any(|w| rules::is_filler(w) || CORRECTION_CUES.contains(&w.as_str()))
        || words.windows(2).any(|p| p[0] == p[1])
        || raw.chars().any(|c| c.is_ascii_digit())
        || words
            .iter()
            .any(|w| matches!(w.as_str(), "percent" | "dollars" | "thousand" | "hundred"))
}

/// ChatML (Qwen family) with thinking disabled by an empty think block. Returns the shared
/// prefix (instructions + examples, cached by the runtime) and the per-dictation suffix.
pub fn build_prompt(raw: &str) -> (String, String) {
    let mut prefix = format!("<|im_start|>system\n{SYSTEM}<|im_end|>\n");
    for (input, output) in EXAMPLES {
        prefix.push_str(&format!(
            "<|im_start|>user\n{input}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n{output}<|im_end|>\n"
        ));
    }
    let suffix = format!(
        "<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n",
        raw.trim()
    );
    (prefix, suffix)
}

/// The few-shot prompt for a non-default style.
fn build_styled_prompt(raw: &str, style: Style) -> (String, String) {
    let (system, examples) = match style {
        Style::Formal => (SYSTEM_FORMAL, EXAMPLES_FORMAL),
        Style::Casual => (SYSTEM_CASUAL, EXAMPLES_CASUAL),
        Style::Prompt => (SYSTEM_PROMPT, EXAMPLES_PROMPT),
        Style::Clean => return build_prompt(raw),
    };
    let mut prefix = format!("<|im_start|>system\n{system}<|im_end|>\n");
    for (input, output) in examples {
        prefix.push_str(&format!(
            "<|im_start|>user\n{input}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n{output}<|im_end|>\n"
        ));
    }
    let suffix = format!(
        "<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n",
        raw.trim()
    );
    (prefix, suffix)
}

/// The prompt for MuskFlow's fine-tuned models; must match finetune/lite_cleanup.py byte for byte.
pub fn build_compact_prompt(raw: &str) -> (String, String) {
    let prefix = format!("<|im_start|>system\n{COMPACT_SYSTEM}<|im_end|>\n");
    let suffix = format!(
        "<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n",
        raw.trim()
    );
    (prefix, suffix)
}

/// MuskFlow's own fine-tunes are published as `MuskFlow-*.gguf`.
pub fn is_muskflow_model(name: &str) -> bool {
    name.to_lowercase().starts_with("muskflow")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_clean_phrases_skip_the_llm() {
        assert!(!needs_llm("Sounds good, see you tomorrow."));
        assert!(needs_llm("Sounds good, uh, see you tomorrow."));
        assert!(needs_llm("Move it to Tuesday, no, Wednesday."));
        assert!(needs_llm("It costs 400 dollars."));
        assert!(needs_llm("Can we push the the review"));
    }
}
