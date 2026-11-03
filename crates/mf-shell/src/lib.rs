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
    /// `style` is shown next to "Listening", e.g. "AI prompt".
    Listening { command: bool, style: String },
    /// Fn released: transcribing and cleaning up.
    Working,
    /// Text was typed.
    Typed,
    /// Nothing to type (too short, no speech, cancelled). Shown briefly.
    Nothing(String),
    /// Something needs the user's attention. Shown in the menu and briefly in the pill.
    Error(String),
}

/// Answers the window's requests. Called on the main thread with the request as JSON
/// (`{"id":…,"cmd":…,"args":…}`); returns the reply as JSON (`{"id":…,"result":…}`).
pub type Backend = Box<dyn Fn(&str) -> String + Send + Sync>;

pub struct Config {
    /// Opened by "Edit Dictionary…".
    pub dictionary: PathBuf,
    /// Shown in the menu, e.g. "standard".
    pub tier: String,
    /// The MuskFlow window's page. It talks to `backend` through
    /// `window.webkit.messageHandlers.mf.postMessage(json)` and gets replies via `mf._reply(json)`.
    pub html: &'static str,
    pub backend: Backend,
    /// Open the window as soon as the app starts.
    pub open_window: bool,
}

/// Whether MuskFlow starts when the user logs in. Main thread only.
pub fn launch_at_login() -> bool {
    imp::launch_at_login()
}

/// Turns Launch at Login on or off. Main thread only.
pub fn set_launch_at_login(on: bool) -> anyhow::Result<()> {
    imp::set_launch_at_login(on)
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

    use block2::RcBlock;
    use dispatch2::DispatchQueue;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBezierPath, NSBox,
        NSBoxType, NSColor, NSFont, NSImage, NSMenu, NSMenuItem, NSPanel, NSScreen, NSSound,
        NSStatusBar, NSStatusItem, NSStatusWindowLevel, NSTextAlignment, NSTextField,
        NSTitlePosition, NSVariableStatusItemLength, NSWindow, NSWindowCollectionBehavior,
        NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use objc2_service_management::{SMAppService, SMAppServiceStatus};
    use objc2_web_kit::{
        WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKWebView,
        WKWebViewConfiguration,
    };

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
        handler: Retained<Handler>,
        /// Created the first time it's opened, then kept (closing only hides it).
        window: RefCell<Option<(Retained<NSWindow>, Retained<WKWebView>)>>,
    }

    define_class!(
        // Receives menu clicks.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "MFMenuHandler"]
        struct Handler;

        unsafe impl NSObjectProtocol for Handler {}

        unsafe impl WKScriptMessageHandler for Handler {
            #[unsafe(method(userContentController:didReceiveScriptMessage:))]
            fn did_receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
                // SAFETY: reading the message body on the main thread.
                let body = unsafe { message.body() };
                let Ok(request) = body.downcast::<NSString>() else { return };
                let Some(cfg) = CONFIG.get() else { return };
                let reply = (cfg.backend)(&request.to_string());
                eval_js(&format!("mf._reply({reply})"));
            }
        }

        impl Handler {
            #[unsafe(method(openWindow:))]
            fn open_window_action(&self, _sender: Option<&AnyObject>) {
                open_window(self.mtm());
            }

            #[unsafe(method(editDictionary:))]
            fn edit_dictionary(&self, _sender: Option<&AnyObject>) {
                if let Some(cfg) = CONFIG.get() {
                    // -t: open in the default text editor.
                    let _ = std::process::Command::new("open").arg("-t").arg(&cfg.dictionary).spawn();
                }
            }

            #[unsafe(method(toggleLogin:))]
            fn toggle_login(&self, _sender: Option<&AnyObject>) {
                if let Err(e) = set_launch_at_login(!launch_at_login()) {
                    log::warn!("{e:#}");
                }
            }
        }
    );

    impl Handler {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            unsafe { msg_send![Self::alloc(mtm), init] }
        }
    }

    pub fn launch_at_login() -> bool {
        login_enabled()
    }

    pub fn set_launch_at_login(on: bool) -> anyhow::Result<()> {
        // SAFETY: plain ServiceManagement calls on the app's own login item.
        let res = unsafe {
            let svc = SMAppService::mainAppService();
            if on {
                svc.registerAndReturnError()
            } else {
                svc.unregisterAndReturnError()
            }
        };
        SHELL.with(|s| {
            if let Some(sh) = s.borrow().as_ref() {
                sh.login.setState(if login_enabled() { 1 } else { 0 });
            }
        });
        res.map_err(|e| anyhow::anyhow!("launch at login: {}", e.localizedDescription()))
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

        app.setMainMenu(Some(&edit_menu(mtm)));
        let shell = build(mtm, &tier);
        SHELL.with(|s| *s.borrow_mut() = Some(shell));
        apply(State::Loading, mtm);
        if CONFIG.get().is_some_and(|c| c.open_window) {
            open_window(mtm);
        }

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

        let open = menu_item(mtm, "Open MuskFlow…", Some(sel!(openWindow:)));
        unsafe { open.setTarget(Some(&handler)) };
        menu.addItem(&open);

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
            handler,
            window: RefCell::new(None),
        }
    }

    /// A menu-bar app has no menus of its own, and without an Edit menu ⌘C/⌘V/⌘A do
    /// nothing in the window's text fields. This one is never shown, only its shortcuts.
    fn edit_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
        let main = NSMenu::new(mtm);
        let edit = NSMenu::new(mtm);
        for (title, action, key) in [
            ("Undo", sel!(undo:), "z"),
            ("Redo", sel!(redo:), "Z"),
            ("Cut", sel!(cut:), "x"),
            ("Copy", sel!(copy:), "c"),
            ("Paste", sel!(paste:), "v"),
            ("Select All", sel!(selectAll:), "a"),
            ("Close Window", sel!(performClose:), "w"),
        ] {
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    Some(action),
                    &NSString::from_str(key),
                )
            };
            edit.addItem(&item);
        }
        let holder = menu_item(mtm, "Edit", None);
        holder.setSubmenu(Some(&edit));
        main.addItem(&holder);
        main
    }

    fn open_window(mtm: MainThreadMarker) {
        SHELL.with(|s| {
            let s = s.borrow();
            let Some(sh) = s.as_ref() else { return };
            let Some(cfg) = CONFIG.get() else { return };
            let mut slot = sh.window.borrow_mut();
            if slot.is_none() {
                *slot = Some(build_window(mtm, &sh.handler, cfg.html));
            }
            if let Some((window, _)) = slot.as_ref() {
                window.makeKeyAndOrderFront(None);
                #[allow(deprecated)]
                NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
            }
        });
    }

    fn build_window(
        mtm: MainThreadMarker,
        handler: &Retained<Handler>,
        html: &str,
    ) -> (Retained<NSWindow>, Retained<WKWebView>) {
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(980.0, 660.0));
        // SAFETY: standard WebKit setup on the main thread.
        let web = unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            config.userContentController().addScriptMessageHandler_name(
                ProtocolObject::from_ref(&**handler),
                &NSString::from_str("mf"),
            );
            let web = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), rect, &config);
            web.loadHTMLString_baseURL(&NSString::from_str(html), None);
            web
        };
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("MuskFlow"));
        window.setContentMinSize(NSSize::new(760.0, 520.0));
        window.setContentView(Some(&web));
        window.center();
        (window, web)
    }

    /// Runs a snippet in the window's page, if the window exists.
    fn eval_js(js: &str) {
        SHELL.with(|s| {
            let s = s.borrow();
            let Some(sh) = s.as_ref() else { return };
            if let Some((_, web)) = sh.window.borrow().as_ref() {
                // SAFETY: main thread; no completion handler.
                unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(js), None) };
            }
        });
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
            let (icon, status, pill, sound): (Icon, String, Option<String>, Option<&str>) =
                match &state {
                    State::Loading => (Icon::Working, "Loading models…".into(), None, None),
                    State::Ready => (Icon::Idle, "Ready".into(), None, None),
                    State::Listening { command, style } => (
                        Icon::Listening,
                        format!("Listening · {style}"),
                        Some(if *command {
                            "●  Command".to_string()
                        } else {
                            format!("●  Listening · {style}")
                        }),
                        Some("Tink"),
                    ),
                    State::Working => (
                        Icon::Working,
                        "Writing…".into(),
                        Some("Writing…".into()),
                        None,
                    ),
                    State::Typed => (Icon::Idle, "Ready".into(), None, Some("Pop")),
                    State::Nothing(why) => (Icon::Idle, "Ready".into(), Some(why.clone()), None),
                    State::Error(msg) => (
                        Icon::Error,
                        format!("⚠ {msg}"),
                        Some(format!("⚠ {}", short(msg))),
                        Some("Basso"),
                    ),
                };
            if let Some(button) = sh.item.button(mtm)
                && let Some(img) = icon_image(icon)
            {
                button.setImage(Some(&img));
            }
            sh.status.setTitle(&NSString::from_str(&status));
            if let Some((_, web)) = sh.window.borrow().as_ref() {
                let js = format!(
                    "window.mf && mf._event({:?}, {:?})",
                    event_name(&state),
                    status
                );
                // SAFETY: main thread; no completion handler.
                unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(&js), None) };
            }
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

    #[derive(Clone, Copy)]
    enum Icon {
        Idle,
        Listening,
        Working,
        Error,
    }

    /// MuskFlow's mark: an "M" drawn as five sound-wave bars hanging from the top, so the
    /// inner bars form the letter's V. While listening the bars centre into a live waveform.
    /// Returns bar heights (points, of 18) and whether they hang from the top.
    fn bars(icon: Icon) -> ([f64; 5], bool) {
        match icon {
            Icon::Idle | Icon::Error => ([15.0, 8.0, 12.0, 8.0, 15.0], true),
            Icon::Listening => ([8.0, 15.0, 6.0, 15.0, 8.0], false),
            Icon::Working => ([10.0, 10.0, 10.0, 10.0, 10.0], false),
        }
    }

    /// A template image, so macOS tints it for light and dark menu bars.
    fn icon_image(icon: Icon) -> Option<Retained<NSImage>> {
        if let Icon::Error = icon {
            let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("exclamationmark.triangle"),
                Some(&NSString::from_str("MuskFlow needs attention")),
            )?;
            img.setTemplate(true);
            return Some(img);
        }
        const SIZE: f64 = 18.0;
        const BAR: f64 = 2.4;
        const GAP: f64 = 1.1;
        const TALLEST: f64 = 15.0;
        let (heights, hang) = bars(icon);
        let draw = RcBlock::new(move |_rect: NSRect| -> Bool {
            NSColor::blackColor().setFill();
            let x0 = (SIZE - (5.0 * BAR + 4.0 * GAP)) / 2.0;
            for (i, h) in heights.iter().enumerate() {
                let rect = NSRect::new(
                    NSPoint::new(
                        x0 + i as f64 * (BAR + GAP),
                        // y grows upwards: hanging bars share the top edge of the tallest.
                        if hang {
                            (SIZE + TALLEST) / 2.0 - h
                        } else {
                            (SIZE - h) / 2.0
                        },
                    ),
                    NSSize::new(BAR, *h),
                );
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, BAR / 2.0, BAR / 2.0)
                    .fill();
            }
            Bool::YES
        });
        let img =
            NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(SIZE, SIZE), false, &draw);
        img.setTemplate(true);
        Some(img)
    }

    fn event_name(state: &State) -> &'static str {
        match state {
            State::Typed => "typed",
            State::Error(_) => "error",
            _ => "state",
        }
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

    pub fn launch_at_login() -> bool {
        false
    }

    pub fn set_launch_at_login(_on: bool) -> anyhow::Result<()> {
        anyhow::bail!("launch at login is macOS-only for now")
    }

    pub fn run(_config: Config, work: impl FnOnce(Ui) + Send + 'static) -> ! {
        work(Ui(()));
        std::process::exit(0)
    }
}
