//! The hold key for Windows and Linux, which have no Fn key software can see.
//!
//! The OS hooks report physical key presses as [`Key`]s; [`Chord`] turns them into
//! the same Fn-style signals the macOS tap gives [`Tracker`]: "the hold key went
//! down/up", "the command modifier is held", "some other key was pressed".

use super::{Event, Tracker};

/// A key the hold-key logic cares about. Everything else is [`Key::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    LCtrl,
    RCtrl,
    LMeta,
    RMeta,
    LAlt,
    RAlt,
    LShift,
    RShift,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Ctrl,
    Meta,
    Alt,
    Shift,
    RCtrl,
    RAlt,
}

impl Group {
    fn matches(self, k: Key) -> bool {
        match self {
            Group::Ctrl => matches!(k, Key::LCtrl | Key::RCtrl),
            Group::Meta => matches!(k, Key::LMeta | Key::RMeta),
            Group::Alt => matches!(k, Key::LAlt | Key::RAlt),
            Group::Shift => matches!(k, Key::LShift | Key::RShift),
            Group::RCtrl => k == Key::RCtrl,
            Group::RAlt => k == Key::RAlt,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Group::Ctrl => "Ctrl",
            Group::Meta if cfg!(windows) => "Win",
            Group::Meta => "Super",
            Group::Alt => "Alt",
            Group::Shift => "Shift",
            Group::RCtrl => "Right Ctrl",
            Group::RAlt => "Right Alt",
        }
    }
}

/// The key (or chord) to hold for dictation, from `UMLESS_HOLD_KEY`:
/// `ctrl+win` (the default; `ctrl+super` and `ctrl+meta` are the same), `right-ctrl`,
/// `right-alt`, `ctrl+alt`, `ctrl+shift`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldKey {
    keys: Vec<Group>,
}

impl Default for HoldKey {
    fn default() -> Self {
        Self {
            keys: vec![Group::Ctrl, Group::Meta],
        }
    }
}

impl HoldKey {
    pub fn from_env() -> Self {
        std::env::var("UMLESS_HOLD_KEY")
            .ok()
            .and_then(|s| Self::parse(&s))
            .unwrap_or_default()
    }

    pub fn parse(s: &str) -> Option<Self> {
        let mut keys = Vec::new();
        for part in s.split('+') {
            let g = match part
                .trim()
                .to_ascii_lowercase()
                .replace(['_', ' '], "-")
                .as_str()
            {
                "ctrl" | "control" => Group::Ctrl,
                "win" | "super" | "meta" | "cmd" => Group::Meta,
                "alt" | "option" => Group::Alt,
                "shift" => Group::Shift,
                "right-ctrl" | "rctrl" | "right-control" => Group::RCtrl,
                "right-alt" | "ralt" | "altgr" => Group::RAlt,
                _ => return None,
            };
            if !keys.contains(&g) {
                keys.push(g);
            }
        }
        (!keys.is_empty()).then_some(Self { keys })
    }

    /// "Ctrl + Win".
    pub fn label(&self) -> String {
        self.keys
            .iter()
            .map(|g| g.label())
            .collect::<Vec<_>>()
            .join(" + ")
    }

    /// Whether the hold key uses the Win/Super key (whose release opens the Start menu).
    pub fn uses_meta(&self) -> bool {
        self.keys.contains(&Group::Meta)
    }

    /// Held together with the hold key: command mode. Alt, or Shift if Alt is taken.
    fn command(&self) -> Group {
        if self
            .keys
            .iter()
            .any(|g| matches!(g, Group::Alt | Group::RAlt))
        {
            Group::Shift
        } else {
            Group::Alt
        }
    }

    fn is_part(&self, k: Key) -> bool {
        self.keys.iter().any(|g| g.matches(k))
    }
}

/// Turns raw key presses into hold-key [`Event`]s.
#[derive(Debug)]
pub struct Chord {
    hold: HoldKey,
    down: Vec<Key>,
    active: bool,
    tracker: Tracker,
}

impl Chord {
    pub fn new(hold: HoldKey) -> Self {
        Self {
            hold,
            down: Vec::new(),
            active: false,
            tracker: Tracker::default(),
        }
    }

    #[allow(dead_code)] // used by the Windows hook
    pub fn hold(&self) -> &HoldKey {
        &self.hold
    }

    fn satisfied(&self) -> bool {
        self.hold
            .keys
            .iter()
            .all(|g| self.down.iter().any(|k| g.matches(*k)))
    }

    fn command_held(&self) -> bool {
        let c = self.hold.command();
        self.down.iter().any(|k| c.matches(*k))
    }

    /// A key went down (`pressed`) or up. Auto-repeats should be passed as presses too;
    /// they're ignored for keys already down.
    pub fn key(&mut self, key: Key, pressed: bool) -> Option<Event> {
        if key == Key::Other {
            return if pressed {
                self.tracker.key_down()
            } else {
                None
            };
        }
        if pressed {
            if self.down.contains(&key) {
                return None; // auto-repeat
            }
            self.down.push(key);
            // A modifier that is neither part of the hold key nor the command key
            // (Shift with Ctrl+Win: a shortcut like Ctrl+Win+Shift+…) cancels.
            if self.active && !self.hold.is_part(key) && !self.hold.command().matches(key) {
                return self.tracker.key_down();
            }
        } else {
            self.down.retain(|k| *k != key);
        }
        let now = self.satisfied();
        let changed = now != self.active;
        self.active = now;
        self.tracker.flags(changed, now, self.command_held())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mode;

    #[test]
    fn ctrl_win_dictates_and_alt_upgrades_to_command() {
        let mut c = Chord::new(HoldKey::default());
        assert_eq!(c.key(Key::LCtrl, true), None);
        assert_eq!(c.key(Key::LMeta, true), Some(Event::Down(Mode::Dictate)));
        assert_eq!(c.key(Key::LMeta, true), None); // repeat
        assert_eq!(c.key(Key::LAlt, true), None);
        assert_eq!(c.key(Key::LAlt, false), None);
        assert_eq!(c.key(Key::LCtrl, false), Some(Event::Up(Mode::Command)));
        assert_eq!(c.key(Key::LMeta, false), None);
    }

    #[test]
    fn order_of_keys_does_not_matter() {
        let mut c = Chord::new(HoldKey::default());
        assert_eq!(c.key(Key::RMeta, true), None);
        assert_eq!(c.key(Key::RCtrl, true), Some(Event::Down(Mode::Dictate)));
        assert_eq!(c.key(Key::RMeta, false), Some(Event::Up(Mode::Dictate)));
    }

    #[test]
    fn other_key_cancels_until_release() {
        let mut c = Chord::new(HoldKey::default());
        c.key(Key::LCtrl, true);
        c.key(Key::LMeta, true);
        assert_eq!(c.key(Key::Other, true), Some(Event::Cancel));
        assert_eq!(c.key(Key::Other, false), None);
        assert_eq!(c.key(Key::LCtrl, false), None);
        assert_eq!(c.key(Key::LMeta, false), None);
        c.key(Key::LCtrl, true);
        assert_eq!(c.key(Key::LMeta, true), Some(Event::Down(Mode::Dictate)));
    }

    #[test]
    fn shift_with_ctrl_win_cancels() {
        let mut c = Chord::new(HoldKey::default());
        c.key(Key::LCtrl, true);
        c.key(Key::LMeta, true);
        assert_eq!(c.key(Key::LShift, true), Some(Event::Cancel));
    }

    #[test]
    fn right_ctrl_alone_and_shift_is_not_command() {
        let mut c = Chord::new(HoldKey::parse("right-ctrl").unwrap());
        assert_eq!(c.key(Key::LCtrl, true), None);
        assert_eq!(c.key(Key::LCtrl, false), None);
        assert_eq!(c.key(Key::RCtrl, true), Some(Event::Down(Mode::Dictate)));
        assert_eq!(c.key(Key::LAlt, true), None);
        assert_eq!(c.key(Key::RCtrl, false), Some(Event::Up(Mode::Command)));
    }

    #[test]
    fn right_alt_uses_shift_for_command() {
        let mut c = Chord::new(HoldKey::parse("right-alt").unwrap());
        assert_eq!(c.key(Key::LShift, true), None);
        assert_eq!(c.key(Key::RAlt, true), Some(Event::Down(Mode::Command)));
        assert_eq!(c.key(Key::RAlt, false), Some(Event::Up(Mode::Command)));
    }

    #[test]
    fn parses_and_labels() {
        assert_eq!(HoldKey::parse("Ctrl+Super").unwrap(), HoldKey::default());
        assert_eq!(HoldKey::parse("right ctrl").unwrap().label(), "Right Ctrl");
        assert!(HoldKey::parse("ctrl+banana").is_none());
        assert!(HoldKey::default().uses_meta());
    }
}
