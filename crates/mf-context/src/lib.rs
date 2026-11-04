//! What the user is typing into, and the writing style that calls for.
//!
//! The app comes from the frontmost application; its focused window's title
//! (read through Accessibility, which MuskFlow already has for typing) tells a
//! browser tab apart: "Gmail" is email, "ChatGPT" is an AI chat.
//!
//! [`screen`] reads the text itself, through the same Accessibility permission:
//! the selection, the field around the cursor and the visible text of the window
//! (the email thread, the chat). It feeds command mode, spells names the way the
//! screen does, and tells a casual Teams chat from a formal one. It never leaves
//! this Mac and is never saved.

pub mod text;

use mf_cleanup::Style;

/// The app and window under the user's cursor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Focus {
    pub pid: i32,
    pub bundle_id: String,
    /// The app's display name, e.g. "Slack".
    pub app: String,
    /// The focused window's title, when Accessibility allows reading it.
    pub title: Option<String>,
}

impl Focus {
    /// Where the text goes, for history: "Gmail (Chrome)", "Slack".
    pub fn label(&self) -> String {
        match self.web_app() {
            Some(site) if site != self.app => format!("{site} ({})", self.app),
            _ => self.app.clone(),
        }
    }

    /// A known web app named in a browser window's title.
    fn web_app(&self) -> Option<&'static str> {
        if !BROWSERS.contains(&self.bundle_id.as_str()) {
            return None;
        }
        let title = self.title.as_deref()?.to_lowercase();
        WEB_APPS
            .iter()
            .find(|(needle, _, _)| title.contains(needle))
            .map(|(_, name, _)| *name)
    }

    /// The style for this destination, adjusted to the conversation on screen: a chat
    /// app's formal or casual default gives way when the thread is clearly the other.
    pub fn style_for(&self, screen: &Screen) -> Style {
        let style = self.style();
        match (style, text::tone(&screen.window)) {
            (Style::Formal | Style::Casual, Some(tone)) => tone,
            _ => style,
        }
    }

    /// The style this destination calls for.
    pub fn style(&self) -> Style {
        if let Some((_, _, style)) = self
            .web_app()
            .and_then(|name| WEB_APPS.iter().find(|(_, n, _)| *n == name))
        {
            return *style;
        }
        APPS.iter()
            .find(|(id, _)| self.bundle_id.eq_ignore_ascii_case(id))
            .map(|(_, style)| *style)
            .unwrap_or_default()
    }
}

/// Native apps by bundle id. Terminals count as AI apps: that's where Claude Code,
/// Codex and Gemini CLI run.
const APPS: &[(&str, Style)] = &[
    // AI assistants and agents
    ("com.anthropic.claudefordesktop", Style::Prompt),
    ("com.openai.chat", Style::Prompt),
    ("com.openai.codex", Style::Prompt),
    ("com.todesktop.230313mzl4w4u92", Style::Prompt), // Cursor
    ("com.exafunction.windsurf", Style::Prompt),
    ("ai.perplexity.mac", Style::Prompt),
    ("com.apple.Terminal", Style::Prompt),
    ("com.googlecode.iterm2", Style::Prompt),
    ("dev.warp.Warp-Stable", Style::Prompt),
    ("com.mitchellh.ghostty", Style::Prompt),
    // Email and formal work chat
    ("com.apple.mail", Style::Formal),
    ("com.microsoft.Outlook", Style::Formal),
    ("com.microsoft.teams2", Style::Formal),
    ("com.microsoft.teams", Style::Formal),
    ("com.superhuman.electron", Style::Formal),
    ("com.readdle.smartemail-Mac", Style::Formal),
    ("com.microsoft.Word", Style::Formal),
    // Casual chat
    ("com.tinyspeck.slackmacgap", Style::Casual),
    ("net.whatsapp.WhatsApp", Style::Casual),
    ("desktop.WhatsApp", Style::Casual),
    ("com.apple.MobileSMS", Style::Casual),
    ("com.hnc.Discord", Style::Casual),
    ("ru.keepcoder.Telegram", Style::Casual),
    ("com.facebook.archon", Style::Casual), // Messenger
];

const BROWSERS: &[&str] = &[
    "com.google.Chrome",
    "com.apple.Safari",
    "company.thebrowser.Browser", // Arc
    "com.brave.Browser",
    "com.microsoft.edgemac",
    "org.mozilla.firefox",
    "com.vivaldi.Vivaldi",
    "com.operasoftware.Opera",
    "app.zen-browser.zen",
];

/// Web apps by a lowercase fragment of the browser window's title.
const WEB_APPS: &[(&str, &str, Style)] = &[
    ("chatgpt", "ChatGPT", Style::Prompt),
    ("claude", "Claude", Style::Prompt),
    ("gemini", "Gemini", Style::Prompt),
    ("perplexity", "Perplexity", Style::Prompt),
    ("copilot", "Copilot", Style::Prompt),
    ("ai studio", "AI Studio", Style::Prompt),
    ("v0", "v0", Style::Prompt),
    ("lovable", "Lovable", Style::Prompt),
    ("gmail", "Gmail", Style::Formal),
    ("outlook", "Outlook", Style::Formal),
    ("microsoft teams", "Teams", Style::Formal),
    ("linkedin", "LinkedIn", Style::Formal),
    ("google docs", "Google Docs", Style::Formal),
    ("slack", "Slack", Style::Casual),
    ("whatsapp", "WhatsApp", Style::Casual),
    ("discord", "Discord", Style::Casual),
    ("messenger", "Messenger", Style::Casual),
    ("telegram", "Telegram", Style::Casual),
];

/// The frontmost app and its focused window, if macOS says.
pub fn focused() -> Option<Focus> {
    imp::focused()
}

/// The text on screen around where the user is typing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Screen {
    /// The selected text in the focused field, if any.
    pub selection: Option<String>,
    /// The focused field's text before the cursor (a draft being written), at most ~2,000 characters.
    pub before: String,
    /// The window's visible text, most recent last (an email thread, a chat), at most ~4,000 characters.
    pub window: String,
    /// Whether the focused field's text could be read at all. When it couldn't, an empty
    /// `selection` means "unknown" rather than "nothing selected".
    pub readable: bool,
}

impl Screen {
    /// Everything readable, for finding names in.
    pub fn all_text(&self) -> String {
        let mut s = self.window.clone();
        for part in [
            self.before.as_str(),
            self.selection.as_deref().unwrap_or(""),
        ] {
            if !part.is_empty() {
                s.push('\n');
                s.push_str(part);
            }
        }
        s
    }
}

/// Reads the selection, the text before the cursor and the visible text of app `pid`'s
/// focused window. Takes up to ~300 ms on a busy window: call it off the hot path.
pub fn screen(pid: i32) -> Screen {
    imp::screen(pid)
}

/// The whole text of app `pid`'s focused field, to see what the user changed after we typed.
pub fn field_text(pid: i32) -> Option<String> {
    imp::field_text(pid)
}

/// Keeps the last `max` characters of `s`, starting at a line or word break.
fn tail(s: &str, max: usize) -> String {
    let Some((start, _)) = s.char_indices().rev().nth(max) else {
        return s.to_string();
    };
    let rest = &s[start..];
    if s[..start].ends_with(char::is_whitespace) {
        return rest.to_string();
    }
    let cut = rest
        .find('\n')
        .or_else(|| rest.find(' '))
        .map(|i| i + 1)
        .unwrap_or(0);
    rest[cut..].to_string()
}

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::HashSet;
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use objc2_app_kit::NSWorkspace;
    use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
    use objc2_core_foundation::{CFArray, CFBoolean, CFRange, CFRetained, CFString, CFType};

    use super::{Focus, Screen, tail};

    /// Limits for walking a window's accessibility tree: big web pages have tens of thousands of nodes.
    const MAX_NODES: usize = 4000;
    const MAX_DEPTH: usize = 60;
    const WALK_BUDGET: Duration = Duration::from_millis(300);
    const WINDOW_CHARS: usize = 4000;
    const BEFORE_CHARS: usize = 2000;

    pub fn focused() -> Option<Focus> {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let bundle_id = app
            .bundleIdentifier()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let name = app
            .localizedName()
            .map(|s| s.to_string())
            .unwrap_or_default();
        Some(Focus {
            pid: app.processIdentifier(),
            bundle_id,
            app: name,
            title: window_title(app.processIdentifier()),
        })
    }

    /// The focused window's title via Accessibility; `None` without the permission.
    fn window_title(pid: i32) -> Option<String> {
        // SAFETY: AX calls on an element we own; attribute values are checked before use.
        unsafe {
            let app = AXUIElement::new_application(pid);
            let window = copy(&app, "AXFocusedWindow")?
                .downcast::<AXUIElement>()
                .ok()?;
            let title = copy(&window, "AXTitle")?.downcast::<CFString>().ok()?;
            Some(title.to_string()).filter(|t| !t.is_empty())
        }
    }

    /// Apps that already exposed their content to us (see `expose`).
    static EXPOSED: Mutex<Option<HashSet<i32>>> = Mutex::new(None);

    /// Electron apps (Slack, Teams, VS Code, Discord…) and Chromium build their accessibility
    /// tree only when asked; this asks, once per process.
    fn expose(app: &AXUIElement, pid: i32) {
        let mut seen = EXPOSED.lock().unwrap();
        if seen.get_or_insert_with(HashSet::new).insert(pid) {
            // SAFETY: setting a boolean attribute on an element we own; failure is harmless.
            unsafe {
                app.set_attribute_value(
                    &CFString::from_str("AXManualAccessibility"),
                    CFBoolean::new(true),
                );
            }
        }
    }

    fn focused_element(pid: i32) -> Option<CFRetained<AXUIElement>> {
        // SAFETY: AX calls on elements we own; values are type-checked before use.
        unsafe {
            let app = AXUIElement::new_application(pid);
            app.set_messaging_timeout(0.2);
            expose(&app, pid);
            copy(&app, "AXFocusedUIElement")?
                .downcast::<AXUIElement>()
                .ok()
        }
    }

    fn string(el: &AXUIElement, attribute: &str) -> Option<String> {
        // SAFETY: see `copy`; the value is checked to be a CFString.
        let value = unsafe { copy(el, attribute)? };
        let s = value.downcast::<CFString>().ok()?.to_string();
        Some(s).filter(|s| !s.trim().is_empty())
    }

    pub fn field_text(pid: i32) -> Option<String> {
        string(&*focused_element(pid)?, "AXValue")
    }

    pub fn screen(pid: i32) -> Screen {
        let mut screen = Screen::default();
        if let Some(field) = focused_element(pid) {
            screen.selection = string(&field, "AXSelectedText");
            screen.readable = screen.selection.is_some() || selected_range(&field).is_some();
            if let (Some(value), Some(range)) = (string(&field, "AXValue"), selected_range(&field))
            {
                // AX ranges count UTF-16 units.
                let utf16: Vec<u16> = value.encode_utf16().collect();
                let cursor = (range.location.max(0) as usize).min(utf16.len());
                screen.before = tail(&String::from_utf16_lossy(&utf16[..cursor]), BEFORE_CHARS);
            }
        }
        // SAFETY: AX calls on elements we own.
        let window = unsafe {
            let app = AXUIElement::new_application(pid);
            app.set_messaging_timeout(0.2);
            copy(&app, "AXFocusedWindow").and_then(|w| w.downcast::<AXUIElement>().ok())
        };
        if let Some(window) = window {
            let mut walk = Walk {
                out: String::new(),
                nodes: 0,
                deadline: Instant::now() + WALK_BUDGET,
            };
            walk.visit(&window, 0);
            screen.window = tail(&walk.out, WINDOW_CHARS);
        }
        screen
    }

    fn selected_range(el: &AXUIElement) -> Option<CFRange> {
        // SAFETY: the value is checked to be an AXValue holding a CFRange before it's read.
        unsafe {
            let value = copy(el, "AXSelectedTextRange")?
                .downcast::<AXValue>()
                .ok()?;
            let mut range = CFRange {
                location: 0,
                length: 0,
            };
            value
                .value(
                    AXValueType::CFRange,
                    NonNull::from(&mut range).cast::<c_void>(),
                )
                .then_some(range)
        }
    }

    /// Depth-first walk collecting visible text in reading order.
    struct Walk {
        out: String,
        nodes: usize,
        deadline: Instant,
    }

    impl Walk {
        fn visit(&mut self, el: &AXUIElement, depth: usize) {
            if depth > MAX_DEPTH || self.nodes >= MAX_NODES || Instant::now() > self.deadline {
                return;
            }
            self.nodes += 1;
            let role = string(el, "AXRole").unwrap_or_default();
            match role.as_str() {
                // Text leaves. Text fields hold drafts; their text is read too.
                "AXStaticText" | "AXTextArea" | "AXTextField" => {
                    if let Some(text) = string(el, "AXValue").or_else(|| string(el, "AXTitle")) {
                        self.push(&text);
                    }
                    if role == "AXStaticText" {
                        return;
                    }
                }
                // Chrome: menus, toolbars and the tab strip are noise.
                "AXMenuBar" | "AXMenu" | "AXToolbar" | "AXTabGroup" | "AXScrollBar" => return,
                _ => {}
            }
            // SAFETY: AXChildren is an array of AX elements; each is type-checked.
            let children = unsafe { copy(el, "AXChildren") };
            let Some(children) = children.and_then(|c| c.downcast::<CFArray>().ok()) else {
                return;
            };
            // SAFETY: AXChildren holds AXUIElements.
            let children: CFRetained<CFArray<AXUIElement>> =
                unsafe { CFRetained::cast_unchecked(children) };
            for i in 0..children.len() {
                if let Some(child) = children.get(i) {
                    self.visit(&child, depth + 1);
                }
            }
        }

        fn push(&mut self, text: &str) {
            let text = text.trim();
            if text.is_empty() || self.out.ends_with(text) {
                return;
            }
            if !self.out.is_empty() {
                self.out.push('\n');
            }
            self.out.push_str(text);
        }
    }

    unsafe fn copy(el: &AXUIElement, attribute: &str) -> Option<CFRetained<CFType>> {
        let mut value: *const CFType = std::ptr::null();
        // SAFETY: `value` is a valid out-pointer; on success we own the returned reference.
        let err = unsafe {
            el.copy_attribute_value(&CFString::from_str(attribute), NonNull::from(&mut value))
        };
        if err != AXError::Success {
            return None;
        }
        NonNull::new(value as *mut CFType).map(|v| unsafe { CFRetained::from_raw(v) })
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn focused() -> Option<super::Focus> {
        None
    }

    pub fn screen(_pid: i32) -> super::Screen {
        super::Screen::default()
    }

    pub fn field_text(_pid: i32) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focus(bundle_id: &str, app: &str, title: Option<&str>) -> Focus {
        Focus {
            pid: 0,
            bundle_id: bundle_id.into(),
            app: app.into(),
            title: title.map(String::from),
        }
    }

    #[test]
    fn native_apps_pick_their_style() {
        assert_eq!(
            focus("com.anthropic.claudefordesktop", "Claude", None).style(),
            Style::Prompt
        );
        assert_eq!(
            focus("com.apple.Terminal", "Terminal", None).style(),
            Style::Prompt
        );
        assert_eq!(
            focus("com.microsoft.teams2", "Microsoft Teams", None).style(),
            Style::Formal
        );
        assert_eq!(
            focus("com.tinyspeck.slackmacgap", "Slack", None).style(),
            Style::Casual
        );
        assert_eq!(
            focus("com.apple.Notes", "Notes", None).style(),
            Style::Clean
        );
    }

    #[test]
    fn browser_tabs_go_by_title() {
        let gmail = focus(
            "com.google.Chrome",
            "Google Chrome",
            Some("Inbox (3) - jatin@x.com - Gmail"),
        );
        assert_eq!(gmail.style(), Style::Formal);
        assert_eq!(gmail.label(), "Gmail (Google Chrome)");
        let chat = focus("com.apple.Safari", "Safari", Some("ChatGPT"));
        assert_eq!(chat.style(), Style::Prompt);
        let other = focus("com.google.Chrome", "Google Chrome", Some("Rust docs"));
        assert_eq!(other.style(), Style::Clean);
        assert_eq!(other.label(), "Google Chrome");
    }

    #[test]
    fn the_conversation_overrides_a_chat_apps_default() {
        let teams = focus("com.microsoft.teams2", "Microsoft Teams", None);
        let friendly = Screen {
            window: "haha yeah 😂 see u there lol".into(),
            ..Default::default()
        };
        assert_eq!(teams.style_for(&friendly), Style::Casual);
        assert_eq!(teams.style_for(&Screen::default()), Style::Formal);
        // An AI chat stays a prompt whatever the tone.
        let claude = focus("com.anthropic.claudefordesktop", "Claude", None);
        assert_eq!(claude.style_for(&friendly), Style::Prompt);
    }

    #[test]
    fn tail_starts_at_a_break() {
        assert_eq!(tail("one two three four", 9), "three four");
        assert_eq!(tail("short", 100), "short");
    }

    #[test]
    fn titles_only_count_in_browsers() {
        // A Notes document titled "claude ideas" is still Notes.
        assert_eq!(
            focus("com.apple.Notes", "Notes", Some("claude ideas")).style(),
            Style::Clean
        );
    }
}
