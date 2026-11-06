//! Command mode (Fn + Control): the user speaks an instruction instead of the text.
//!
//! With text selected, the instruction is about that text ("make this friendlier",
//! "translate to Hindi") and the answer replaces it. Without a selection, Umless
//! writes new text where the cursor is ("reply saying I'll send it Friday"), using
//! what's on screen (the email thread, the chat) as context.

use std::time::{Duration, Instant};

use anyhow::Result;
use umless_llm::{Generation, Llm};

use crate::Style;

const SYSTEM: &str = "You are Umless, a writing assistant built into the user's keyboard. The user spoke an instruction; you write the exact text to put where they are typing, ready to send.
Rules:
- Output only that text. No preamble (\"Sure\", \"Here is\"), no quotes around it, no notes after it.
- Selected text and drafts were written by the user. When text is selected, the instruction is about it: output the full replacement, keeping its meaning, facts and point of view and changing only what the instruction asks.
- With no selection, write new text for the cursor, using the screen as context.
- When replying to a message, you are the user writing back to its sender: greet the sender by name, never the user.
- Use only what the user said and what is on screen. Never invent facts, numbers, dates or prices, and add no promises, offers or next steps the user didn't ask for. If something needed is unknown, write a placeholder like [date].
- Write in the language of the conversation unless told otherwise. Keep it as short as the instruction allows. Plain text, no markdown unless asked.";

/// Worked examples (screen, selection, instruction → text), sent as prior turns: small models
/// copy examples far better than they follow rules.
const EXAMPLES: &[(&str, Option<&str>, &str, &str)] = &[
    // A chat reply has no greeting or sign-off.
    (
        "#eng\nRohan Gupta: is the staging deploy done? QA is waiting",
        None,
        "tell him yes it's live and QA can start",
        "Yes, it's live on staging. QA can start.",
    ),
    (
        "Re: Q3 budget\nPriya Nair\nHi Sam,\nCould you share the final Q3 budget numbers before Friday's review?\nThanks,\nPriya",
        None,
        "reply saying yes I'll send them Thursday morning",
        "Hi Priya,\n\nYes, I'll send the final Q3 budget numbers on Thursday morning.\n\nBest,\nSam",
    ),
    (
        "",
        Some("cant make it today sorry, something came up. lets do tmrw"),
        "make this more professional",
        "I'm sorry, I can't make it today as something came up. Could we meet tomorrow instead?",
    ),
    (
        "Contract renewal\nMark Ellis\nHi Ana,\nThe renewal is $4,800 a year. Can you also send the signed NDA, and let me know who will be the point of contact?\nMark",
        None,
        "reply that the price works for us",
        "Hi Mark,\n\nThe price works for us.\n\nBest,\nAna",
    ),
];

/// An empty think block: Qwen's switch for answering straight away.
const THINK_OFF: &str = "<think>\n\n</think>\n\n";

/// Generated text is capped; a selection's replacement may be a little longer than it.
const NEW_TEXT_TOKENS: usize = 400;
const MAX_TOKENS: usize = 900;

/// What's on screen when the command is spoken.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context<'a> {
    /// Where the text goes: "Gmail (Google Chrome)", "Slack".
    pub app: Option<&'a str>,
    pub style: Style,
    pub selection: Option<&'a str>,
    /// The field's text before the cursor.
    pub before: &'a str,
    /// The window's visible text, most recent last.
    pub window: &'a str,
}

#[derive(Debug, Clone)]
pub struct Commanded {
    pub text: String,
    /// The answer replaces the selection rather than being inserted at the cursor.
    pub replaces_selection: bool,
    pub generation: Generation,
    pub time: Duration,
}

/// Runs `instruction` (already cleaned of fillers) against the screen context.
pub fn run(llm: &Llm, instruction: &str, cx: &Context) -> Result<Commanded> {
    let t = Instant::now();
    let selection = cx.selection.map(str::trim).filter(|s| !s.is_empty());
    let max_tokens = match selection {
        Some(sel) => (llm.count_tokens(sel) * 3 / 2 + 100).min(MAX_TOKENS),
        None => NEW_TEXT_TOKENS,
    };
    let mut prefix = format!("<|im_start|>system\n{SYSTEM}<|im_end|>\n");
    // Umless's own fine-tunes from round 3 on learned command mode: no examples needed.
    let examples = if crate::umless_round(llm.name()).is_some_and(|r| r >= 3) {
        &[][..]
    } else {
        EXAMPLES
    };
    for (window, selection, instruction, answer) in examples {
        let example = Context {
            window,
            selection: *selection,
            ..Default::default()
        };
        prefix.push_str(&user_turn(instruction, &example, *selection, "", window));
        prefix.push_str(&format!("{answer}<|im_end|>\n"));
    }
    // Fit the screen into what's left of the context window: the selection first, then the
    // draft, then the window's most recent text.
    let fixed = llm.count_tokens(&prefix)
        + llm.count_tokens(&user_turn(instruction, cx, selection, "", ""))
        + 16;
    let mut budget = llm.context_tokens().saturating_sub(fixed + max_tokens);
    let selection = selection.map(|s| fit_tail(llm, s, &mut budget));
    let before = fit_tail(llm, cx.before.trim(), &mut budget);
    let window = fit_tail(llm, cx.window.trim(), &mut budget);
    let suffix = user_turn(instruction, cx, selection.as_deref(), &before, &window);
    let mut generation = llm.complete(&prefix, &suffix, max_tokens)?;
    let screen = format!("{window}\n{before}");
    if tidy(&generation.text).is_empty() {
        // Non-thinking models (Qwen3-4B-Instruct-2507) sometimes end at once after an empty
        // think block; ask again without it.
        let plain = suffix.replace(THINK_OFF, "");
        generation = llm.complete(&prefix, &plain, max_tokens)?;
    }
    let text = drop_invented_signoff(&tidy(&generation.text), &screen, cx.style);
    Ok(Commanded {
        text,
        replaces_selection: selection.is_some(),
        generation,
        time: t.elapsed(),
    })
}

fn user_turn(
    instruction: &str,
    cx: &Context,
    selection: Option<&str>,
    before: &str,
    window: &str,
) -> String {
    let mut msg = String::new();
    if let Some(app) = cx.app {
        msg.push_str(&format!("App: {app}\n"));
    }
    msg.push_str(&format!("Tone: {}\n", tone(cx.style)));
    if !window.is_empty() {
        msg.push_str(&format!(
            "\nOn screen (most recent last):\n<<<\n{window}\n>>>\n"
        ));
    }
    if !before.is_empty() {
        msg.push_str(&format!("\nText before the cursor:\n<<<\n{before}\n>>>\n"));
    }
    if let Some(sel) = selection {
        msg.push_str(&format!("\nSelected text:\n<<<\n{sel}\n>>>\n"));
    }
    msg.push_str(&format!("\nInstruction: {}", instruction.trim()));
    format!("<|im_start|>user\n{msg}<|im_end|>\n<|im_start|>assistant\n{THINK_OFF}")
}

fn tone(style: Style) -> &'static str {
    match style {
        Style::Formal => "professional",
        Style::Casual => "friendly and casual",
        Style::Prompt => "a clear, concise prompt for an AI assistant",
        Style::Clean => "natural, matching the conversation",
    }
}

/// The end of `text` that fits in `budget` tokens; spends them.
fn fit_tail(llm: &Llm, text: &str, budget: &mut usize) -> String {
    if text.is_empty() || *budget == 0 {
        return String::new();
    }
    let tokens = llm.count_tokens(text);
    if tokens <= *budget {
        *budget -= tokens;
        return text.to_string();
    }
    // Tokens are roughly proportional to characters; keep that share of the end.
    let keep = text.chars().count() * *budget / tokens;
    let start = text.char_indices().rev().nth(keep).map_or(0, |(i, _)| i);
    *budget = 0;
    text[start..].to_string()
}

/// Small models copy the sign-off from their examples ("Best,\nSam"). Keep a sign-off
/// only when the name is on screen (the thread says who the user is), and never in a chat.
fn drop_invented_signoff(text: &str, screen: &str, style: Style) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    if n < 3 {
        return text.to_string();
    }
    let name = lines[n - 1].trim();
    let closing = lines[n - 2].trim().to_lowercase();
    let is_closing = [
        "best,",
        "thanks,",
        "regards,",
        "best regards,",
        "cheers,",
        "thank you,",
        "kind regards,",
    ]
    .contains(&closing.as_str());
    let looks_like_name = !name.is_empty()
        && name.split_whitespace().count() <= 2
        && name.chars().next().is_some_and(char::is_uppercase)
        && !name.ends_with(['.', '!', '?']);
    if !is_closing || !looks_like_name {
        return text.to_string();
    }
    if style != Style::Casual && screen.contains(name) {
        return text.to_string();
    }
    let mut kept = lines[..n - 2].join("\n");
    if style == Style::Casual {
        // "Hi Gaurav,\n\n…" in a chat: drop the letter-style greeting too.
        if let Some((first, rest)) = kept.split_once('\n')
            && first.trim_end().ends_with(',')
            && first.split_whitespace().count() <= 3
            && ["hi", "hey", "hello", "dear"]
                .iter()
                .any(|g| first.to_lowercase().starts_with(g))
        {
            kept = rest.to_string();
        }
    }
    kept.trim().to_string()
}

/// Strips what small models wrap their answer in: a "Sure, here's…" line, quotes, fences.
pub fn tidy(text: &str) -> String {
    let mut t = text.trim();
    if let Some(end) = t.find("</think>") {
        t = t[end + "</think>".len()..].trim();
    }
    if let Some((first, rest)) = t.split_once('\n') {
        let lower = first.to_lowercase();
        if first.trim_end().ends_with(':')
            && (lower.starts_with("sure")
                || lower.starts_with("here")
                || lower.starts_with("certainly")
                || lower.starts_with("of course"))
        {
            t = rest.trim();
        }
    }
    if t.starts_with("```") && t.ends_with("```") && t.len() > 6 {
        t = t[3..t.len() - 3].trim();
        // Drop a language tag line.
        if let Some((tag, rest)) = t.split_once('\n')
            && !tag.contains(' ')
            && tag.len() < 12
        {
            t = rest.trim();
        }
    }
    for (open, close) in [('"', '"'), ('“', '”'), ('\'', '\'')] {
        if t.len() > 2 && t.starts_with(open) && t.ends_with(close) && t.matches(open).count() <= 2
        {
            t = t[open.len_utf8()..t.len() - close.len_utf8()].trim();
        }
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::{Style, drop_invented_signoff, tidy};

    #[test]
    fn invented_signoffs_are_dropped() {
        let thread = "Hi Jatin,\nWould Thursday work?\nDaniel";
        let mail = "Hi Daniel,\n\nYes, Thursday works.\n\nBest,\nJatin";
        assert_eq!(drop_invented_signoff(mail, thread, Style::Formal), mail);
        assert_eq!(
            drop_invented_signoff(
                "Hi Daniel,\n\nYes, Thursday works.\n\nBest,\nSam",
                thread,
                Style::Formal
            ),
            "Hi Daniel,\n\nYes, Thursday works."
        );
        assert_eq!(
            drop_invented_signoff(
                "Hi Gaurav,\n\nIt's fixed and deployed.\n\nBest,\nSam",
                "Gaurav: export is broken",
                Style::Casual
            ),
            "It's fixed and deployed."
        );
    }

    #[test]
    fn tidy_strips_wrappers() {
        assert_eq!(
            tidy("Sure, here's the reply:\nThanks, I'll send it Friday."),
            "Thanks, I'll send it Friday."
        );
        assert_eq!(tidy("\"Sounds good!\""), "Sounds good!");
        assert_eq!(tidy("```text\nhello\n```"), "hello");
        assert_eq!(tidy("<think>\n\n</think>\n\nHi Sarah"), "Hi Sarah");
        // A real first line ending in a colon stays.
        assert_eq!(tidy("Agenda:\n1. Budget"), "Agenda:\n1. Budget");
    }
}
