//! What the user is typing into, and the writing style that calls for.
//!
//! The app comes from the frontmost application; its focused window's title
//! (read through Accessibility, which MuskFlow already has for typing) tells a
//! browser tab apart: "Gmail" is email, "ChatGPT" is an AI chat. Reading the
//! text around the cursor comes later (ARCHITECTURE.md §6).

use mf_cleanup::Style;

/// The app and window under the user's cursor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Focus {
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

#[cfg(target_os = "macos")]
mod imp {
    use std::ptr::NonNull;

    use objc2_app_kit::NSWorkspace;
    use objc2_application_services::{AXError, AXUIElement};
    use objc2_core_foundation::{CFRetained, CFString, CFType};

    use super::Focus;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focus(bundle_id: &str, app: &str, title: Option<&str>) -> Focus {
        Focus {
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
    fn titles_only_count_in_browsers() {
        // A Notes document titled "claude ideas" is still Notes.
        assert_eq!(
            focus("com.apple.Notes", "Notes", Some("claude ideas")).style(),
            Style::Clean
        );
    }
}
