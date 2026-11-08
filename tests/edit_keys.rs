//! `edit_keys::offer` against this Mac's own WebKit, in a real `WKWebView` that is its (never
//! shown) window's first responder, under a main menu whose Edit › Undo is ⌘Z:
//!
//! - **page takes it** — a page that handles ⌘Z gets the keydown, and the menu never acts.
//! - **page leaves it** — a page that ignores ⌘Z still gets the keydown, then WebKit hands the
//!   chord back and the menu's Undo acts, so a plain text field still undoes.
//! - **not a webview** — any other responder is left to the menu.
//!
//! `harness = false` (see Cargo.toml): WebKit must run on the process's main thread, which libtest
//! never gives a test.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    mac::run();
}

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBackingStoreType, NSEvent, NSEventModifierFlags, NSEventType, NSMenu,
        NSMenuItem, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{NSDate, NSObject, NSPoint, NSRect, NSRunLoop, NSSize, NSString};
    use objc2_web_kit::{WKWebView, WKWebViewConfiguration};
    use shell_core::edit_keys::offer;

    static UNDOS: AtomicUsize = AtomicUsize::new(0);

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "ShellCoreEditKeysUndoTarget"]
        struct UndoTarget;

        impl UndoTarget {
            #[unsafe(method(undo:))]
            fn undo(&self, _sender: Option<&AnyObject>) {
                UNDOS.fetch_add(1, Ordering::SeqCst);
            }
        }
    );

    /// Counts ⌘Z keydowns in the title; handles them (`preventDefault`) when `TAKE`.
    const PAGE: &str = r#"<!doctype html><textarea id=t></textarea><script>
var TAKE = __TAKE__, n = 0;
addEventListener('keydown', function (e) {
  if (!e.metaKey || e.key.toLowerCase() !== 'z') return;
  n++; if (TAKE) e.preventDefault(); document.title = 'SEEN ' + n;
}, true);
addEventListener('load', function () { document.getElementById('t').focus(); document.title = 'READY'; });
</script>"#;

    fn ns(s: &str) -> Retained<NSString> {
        NSString::from_str(s)
    }

    fn pump_until(webview: &WKWebView, want: &str, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.05));
            let title = unsafe { webview.title() }
                .map(|t| t.to_string())
                .unwrap_or_default();
            if title == want {
                return;
            }
            assert!(Instant::now() < deadline, "{what}: title stayed {title:?}");
        }
    }

    fn settle() {
        NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.5));
    }

    fn cmd_z(window: &NSWindow) -> Retained<NSEvent> {
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            NSPoint::ZERO,
            NSEventModifierFlags::Command,
            0.0,
            window.windowNumber(),
            None,
            &ns("z"),
            &ns("z"),
            false,
            6,
        )
        .expect("a ⌘Z key event")
    }

    /// A first-responder webview showing [`PAGE`], in a window that is never ordered on screen.
    fn page(mtm: MainThreadMarker, take: bool) -> (Retained<NSWindow>, Retained<WKWebView>) {
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(400.0, 300.0));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        let webview = unsafe {
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config)
        };
        window.setContentView(Some(&webview));
        assert!(window.makeFirstResponder(Some(&webview)));
        let html = PAGE.replace("__TAKE__", if take { "true" } else { "false" });
        unsafe { webview.loadHTMLString_baseURL(&ns(&html), None) };
        pump_until(&webview, "READY", "load");
        (window, webview)
    }

    pub fn run() {
        let mtm = MainThreadMarker::new().expect("harness = false runs main() on the main thread");
        let app = NSApplication::sharedApplication(mtm);

        let target: Retained<UndoTarget> = unsafe { msg_send![UndoTarget::alloc(mtm), init] };
        let undo = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &ns("Undo"),
                Some(sel!(undo:)),
                &ns("z"),
            )
        };
        unsafe { undo.setTarget(Some(&target)) };
        let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), &ns("Edit"));
        edit.addItem(&undo);
        let holder = NSMenuItem::new(mtm);
        holder.setSubmenu(Some(&edit));
        let main_menu = NSMenu::new(mtm);
        main_menu.addItem(&holder);
        app.setMainMenu(Some(&main_menu));

        let (window, webview) = page(mtm, true);
        assert!(offer(&webview, &cmd_z(&window)), "WebKit refused the chord");
        pump_until(&webview, "SEEN 1", "page takes it");
        settle();
        assert_eq!(
            UNDOS.load(Ordering::SeqCst),
            0,
            "the menu acted on a handled chord"
        );
        println!("test edit_keys::page_takes_it ... ok");

        let (window, webview) = page(mtm, false);
        assert!(offer(&webview, &cmd_z(&window)), "WebKit refused the chord");
        pump_until(&webview, "SEEN 1", "page leaves it");
        settle();
        assert_eq!(
            UNDOS.load(Ordering::SeqCst),
            1,
            "the menu never got the chord the page left"
        );
        println!("test edit_keys::page_leaves_it ... ok");

        assert!(!offer(&window, &cmd_z(&window)));
        println!("test edit_keys::not_a_webview ... ok");
    }
}
