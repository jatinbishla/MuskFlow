//! MuskFlow's macOS shell: a menu-bar icon, a small "Listening" pill near the
//! bottom of the screen, start/stop sounds, and Launch at Login.
//!
//! AppKit must run on the main thread, so [`run`] takes over the main thread and
//! runs the dictation loop on a worker thread. The worker reports progress through
//! a [`Ui`] handle, which forwards each update to the main queue.

use std::path::PathBuf;

/// What the user should see.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Models are loading.
    Loading,
    /// Waiting for Fn.
    Ready,
    /// Fn is held.
    Listening { command: bool },
    /// Fn released: transcribing and cleaning up.
    Working,
    /// Text was typed.
    Typed,
    /// Nothing to type (too short, no speech, cancelled). Shown briefly.
    Nothing(String),
    /// Something needs the user's attention. Shown in the menu and briefly in the pill.
    Error(String),
}

pub struct Config {
    /// Opened by "Edit Dictionary…".
    pub dictionary: PathBuf,
    /// Shown in the menu, e.g. "standard".
    pub tier: String,
}

/// A cheap handle the worker thread uses to update the UI.
#[derive(Clone)]
pub struct Ui(());

impl Ui {
    pub fn set(&self, state: State) {
        imp::set(state);
    }
}

/// Shows the menu-bar icon, then runs `work` on a worker thread. Never returns;
/// "Quit MuskFlow" ends the process.
pub fn run(config: Config, work: impl FnOnce(Ui) + Send + 'static) -> ! {
    imp::run(config, work)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::RefCell;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use dispatch2::DispatchQueue;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBox, NSBoxType,
        NSColor, NSFont, NSImage, NSMenu, NSMenuItem, NSPanel, NSScreen, NSSound, NSStatusBar,
        NSStatusItem, NSStatusWindowLevel, NSTextAlignment, NSTextField, NSTitlePosition,
        NSVariableStatusItemLength, NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    use super::{Config, State, Ui};

    const PILL_HEIGHT: f64 = 36.0;
    /// Distance from the bottom of the usable screen area.
    const PILL_MARGIN: f64 = 90.0;
    /// How long "nothing to type" and errors stay on screen.
    const FLASH: Duration = Duration::from_millis(2500);

    static CONFIG: OnceLock<Config> = OnceLock::new();
    /// Bumped on every state change, so a delayed hide doesn't hide a newer pill.
    static GENERATION: AtomicU64 = AtomicU64::new(0);

    thread_local! {
        static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
    }

    struct Shell {
        item: Retained<NSStatusItem>,
        status: Retained<NSMenuItem>,
        login: Retained<NSMenuItem>,
        pill: Retained<NSPanel>,
        backdrop: Retained<NSBox>,
        label: Retained<NSTextField>,
        _handler: Retained<Handler>,
    }

    define_class!(
        // Receives menu clicks.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "MFMenuHandler"]
        struct Handler;

        unsafe impl NSObjectProtocol for Handler {}

        impl Handler {
            #[unsafe(method(editDictionary:))]
            fn edit_dictionary(&self, _sender: Option<&AnyObject>) {
                if let Some(cfg) = CONFIG.get() {
                    // -t: open in the default text editor.
                    let _ = std::process::Command::new("open").arg("-t").arg(&cfg.dictionary).spawn();
                }
            }

            #[unsafe(method(toggleLogin:))]
            fn toggle_login(&self, _sender: Option<&AnyObject>) {
                // SAFETY: plain ServiceManagement calls on the app's own login item.
                let res = unsafe {
                    let svc = SMAppService::mainAppService();
                    if login_enabled() {
                        svc.unregisterAndReturnError()
                    } else {
                        svc.registerAndReturnError()
                    }
                };
                if let Err(e) = res {
                    log::warn!("launch at login: {}", e.localizedDescription());
                }
                SHELL.with(|s| {
                    if let Some(sh) = s.borrow().as_ref() {
                        sh.login.setState(if login_enabled() { 1 } else { 0 });
                    }
                });
            }
        }
    );

    impl Handler {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            unsafe { msg_send![Self::alloc(mtm), init] }
        }
    }

    fn login_enabled() -> bool {
        // SAFETY: a read-only status query.
        unsafe { SMAppService::mainAppService().status() == SMAppServiceStatus::Enabled }
    }

    pub fn run(config: Config, work: impl FnOnce(Ui) + Send + 'static) -> ! {
        let mtm = MainThreadMarker::new().expect("mf_shell::run must be called on the main thread");
        let tier = config.tier.clone();
        let _ = CONFIG.set(config);

        let app = NSApplication::sharedApplication(mtm);
        // A menu-bar app: no Dock icon, no app menu.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let shell = build(mtm, &tier);
        SHELL.with(|s| *s.borrow_mut() = Some(shell));
        apply(State::Loading, mtm);

        std::thread::Builder::new()
            .name("muskflow".into())
            .spawn(move || work(Ui(())))
            .expect("could not start the dictation thread");

        app.run();
        std::process::exit(0)
    }

    pub fn set(state: State) {
        DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("main queue runs on the main thread");
            apply(state, mtm);
        });
    }

    fn build(mtm: MainThreadMarker, tier: &str) -> Shell {
        let handler = Handler::new(mtm);

        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let menu = NSMenu::new(mtm);
        let status = menu_item(mtm, "Starting…", None);
        status.setEnabled(false);
        menu.addItem(&status);
        let hint = menu_item(mtm, &format!("Hold Fn to dictate · {tier} tier"), None);
        hint.setEnabled(false);
        menu.addItem(&hint);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let edit = menu_item(mtm, "Edit Dictionary…", Some(sel!(editDictionary:)));
        unsafe { edit.setTarget(Some(&handler)) };
        menu.addItem(&edit);
        let login = menu_item(mtm, "Launch at Login", Some(sel!(toggleLogin:)));
        unsafe { login.setTarget(Some(&handler)) };
        login.setState(if login_enabled() { 1 } else { 0 });
        menu.addItem(&login);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        // No target: goes to NSApp, which quits.
        menu.addItem(&menu_item(mtm, "Quit MuskFlow", Some(sel!(terminate:))));
        item.setMenu(Some(&menu));

        let (pill, backdrop, label) = build_pill(mtm);
        Shell {
            item,
            status,
            login,
            pill,
            backdrop,
            label,
            _handler: handler,
        }
    }

    fn menu_item(
        mtm: MainThreadMarker,
        title: &str,
        action: Option<objc2::runtime::Sel>,
    ) -> Retained<NSMenuItem> {
        unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                action,
                &NSString::from_str(""),
            )
        }
    }

    /// A borderless, click-through panel that floats above everything, on every Space.
    fn build_pill(
        mtm: MainThreadMarker,
    ) -> (Retained<NSPanel>, Retained<NSBox>, Retained<NSTextField>) {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(200.0, PILL_HEIGHT));
        let pill = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        pill.setLevel(NSStatusWindowLevel);
        pill.setOpaque(false);
        pill.setBackgroundColor(Some(&NSColor::clearColor()));
        pill.setHasShadow(true);
        pill.setIgnoresMouseEvents(true);
        pill.setHidesOnDeactivate(false);
        pill.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        unsafe { pill.setReleasedWhenClosed(false) };

        let backdrop = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
        backdrop.setBoxType(NSBoxType::Custom);
        backdrop.setTitlePosition(NSTitlePosition::NoTitle);
        backdrop.setBorderWidth(0.0);
        backdrop.setCornerRadius(PILL_HEIGHT / 2.0);
        backdrop.setFillColor(&NSColor::colorWithWhite_alpha(0.08, 0.88));
        backdrop.setContentViewMargins(NSSize::new(0.0, 0.0));

        let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        label.setTextColor(Some(&NSColor::whiteColor()));
        label.setFont(Some(&NSFont::systemFontOfSize(14.0)));
        label.setAlignment(NSTextAlignment::Center);
        if let Some(content) = backdrop.contentView() {
            content.addSubview(&label);
        }
        pill.setContentView(Some(&backdrop));
        (pill, backdrop, label)
    }

    fn apply(state: State, mtm: MainThreadMarker) {
        let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
        SHELL.with(|s| {
            let s = s.borrow();
            let Some(sh) = s.as_ref() else { return };
            let (icon, status, pill, sound): (&str, String, Option<String>, Option<&str>) =
                match &state {
                    State::Loading => ("waveform", "Loading models…".into(), None, None),
                    State::Ready => ("mic", "Ready".into(), None, None),
                    State::Listening { command: false } => (
                        "mic.fill",
                        "Listening".into(),
                        Some("●  Listening".into()),
                        Some("Tink"),
                    ),
                    State::Listening { command: true } => (
                        "mic.fill",
                        "Listening (command)".into(),
                        Some("●  Command".into()),
                        Some("Tink"),
                    ),
                    State::Working => {
                        ("ellipsis", "Writing…".into(), Some("Writing…".into()), None)
                    }
                    State::Typed => ("mic", "Ready".into(), None, Some("Pop")),
                    State::Nothing(why) => ("mic", "Ready".into(), Some(why.clone()), None),
                    State::Error(msg) => (
                        "exclamationmark.triangle",
                        format!("⚠ {msg}"),
                        Some(format!("⚠ {}", short(msg))),
                        Some("Basso"),
                    ),
                };
            if let Some(button) = sh.item.button(mtm) {
                let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(icon),
                    Some(&NSString::from_str("MuskFlow")),
                );
                if let Some(img) = img {
                    img.setTemplate(true);
                    button.setImage(Some(&img));
                }
            }
            sh.status.setTitle(&NSString::from_str(&status));
            match pill {
                Some(text) => show_pill(sh, &text, mtm),
                None => sh.pill.orderOut(None),
            }
            if let Some(name) = sound
                && let Some(snd) = NSSound::soundNamed(&NSString::from_str(name))
            {
                snd.play();
            }
            if matches!(state, State::Nothing(_) | State::Error(_)) {
                hide_later(generation);
            }
        });
    }

    fn short(msg: &str) -> String {
        let first = msg.split(['.', '\n']).next().unwrap_or(msg);
        if first.chars().count() > 60 {
            format!("{}…", first.chars().take(60).collect::<String>())
        } else {
            first.to_string()
        }
    }

    fn show_pill(sh: &Shell, text: &str, mtm: MainThreadMarker) {
        sh.label.setStringValue(&NSString::from_str(text));
        let text_size = sh.label.fittingSize();
        let width = (text_size.width + 40.0).max(120.0);
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, PILL_HEIGHT));
        sh.backdrop.setFrame(frame);
        sh.label.setFrame(NSRect::new(
            NSPoint::new(0.0, (PILL_HEIGHT - text_size.height) / 2.0),
            NSSize::new(width, text_size.height),
        ));
        if let Some(screen) = NSScreen::mainScreen(mtm) {
            let area = screen.visibleFrame();
            let origin = NSPoint::new(
                area.origin.x + (area.size.width - width) / 2.0,
                area.origin.y + PILL_MARGIN,
            );
            sh.pill
                .setFrame_display(NSRect::new(origin, NSSize::new(width, PILL_HEIGHT)), true);
        }
        sh.pill.orderFrontRegardless();
    }

    fn hide_later(generation: u64) {
        std::thread::spawn(move || {
            std::thread::sleep(FLASH);
            DispatchQueue::main().exec_async(move || {
                if GENERATION.load(Ordering::Relaxed) == generation {
                    SHELL.with(|s| {
                        if let Some(sh) = s.borrow().as_ref() {
                            sh.pill.orderOut(None);
                        }
                    });
                }
            });
        });
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Config, State, Ui};

    pub fn set(state: State) {
        log::info!("{state:?}");
    }

    pub fn run(_config: Config, work: impl FnOnce(Ui) + Send + 'static) -> ! {
        work(Ui(()));
        std::process::exit(0)
    }
}
