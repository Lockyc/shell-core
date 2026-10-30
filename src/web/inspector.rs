//! Keep the Web Inspector out of the hole-punch layout.
//!
//! WebKit's inspector opens *attached* by default: it re-parents into the inspected view's
//! superview and reframes the inspected view from that **superview's** bounds. Content webviews
//! are `add_child` siblings positioned over the chrome's content hole, and they composite above
//! the chrome, so an attached inspector stretches the inspected tab across the whole window —
//! measured: a tab at `(240,0) 1260×1000` became `(0,0) 1500×500`. The sidebar isn't gone, it's
//! covered, which is why it also stops taking clicks.
//!
//! Forcing the inspector into its own window is the fix, not a workaround: an attached inspector
//! has no way to express "share the window with a sidebar it doesn't know about". Detaching alone
//! isn't enough — WebKit restores the view to fill the superview rather than to the hole it came
//! from — so each window's last hole ([`crate::compositing::last_hole`]) is re-applied afterwards.
//!
//! **The correction is here and nowhere else.** An app's "Open Developer Tools" item, WebKit's own
//! context menu ("Inspect Element") and the inspector's dock buttons all attach, and only the
//! first goes through app code, so a fix at a menu item covers one route in three. [`watch`]
//! instead reacts to the *reframe itself*: every content webview posts frame-change notifications,
//! and a frame change while that webview's inspector is visible means WebKit just attached it.
//! That covers every route, including ones that don't exist yet — and no menu handler should also
//! correct inline, because detaching straight after `open_devtools` is the mid-transaction detach
//! described below.
//!
//! **Timing is the whole difficulty here, and all three constraints were measured, not guessed:**
//!
//! - The frame notification is posted *synchronously inside `setFrame:`*, so the handler runs
//!   nested inside whatever Tauri call is positioning a webview. Doing the work there re-enters
//!   Tauri's window registry from inside one of its own webview ops and wedges the app.
//! - `-[_WKInspector detach]` spins a **nested runloop**, which dispatches queued main-thread work
//!   *inside* the detach — so a second pass starts mid-detach and nests again ([`IN_PROGRESS`]).
//! - Detaching while the attach transaction is still in flight never returns at all
//!   ([`SETTLE_MS`]).
//!
//! Hence the shape: notice on the notification, wait [`SETTLE_MS`] off-thread, then do the work in
//! one non-re-entrant pass on the main thread.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, sel, AnyThread};
use objc2_foundation::{NSNotification, NSString};
use tauri::{AppHandle, Manager};

use crate::compositing::{last_hole, layout_webviews};

/// Captured on the first [`watch`] call — the frame observer runs from an AppKit notification with
/// no context of its own, so it reaches the window registry through this.
static APP: OnceLock<AppHandle> = OnceLock::new();

/// The app-lifetime observer instance (AppKit holds notification observers unretained).
static WATCHER: OnceLock<Retained<Watcher>> = OnceLock::new();

/// Labels of the webviews [`watch`] covers: the ones a restore pass detaches.
static WATCHED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

define_class!(
    // Watches content-webview frame changes to catch an inspector attaching from *any* entry
    // point — WebKit's context menu and the inspector's own dock buttons never call app code.
    #[unsafe(super(NSObject))]
    #[name = "ShellCoreInspectorWatcher"]
    struct Watcher;

    unsafe impl NSObjectProtocol for Watcher {}

    impl Watcher {
        #[unsafe(method(frameDidChange:))]
        fn frame_did_change(&self, note: &NSNotification) {
            let view: *mut AnyObject = unsafe { msg_send![note, object] };
            if view.is_null() {
                return;
            }
            // Only an attached inspector reframes a content webview behind our back. Ignoring
            // frame changes while no inspector is visible keeps ordinary sidebar/window resizes
            // (which drive the frame through `layout_webviews` already) completely untouched.
            if !inspector_visible(view) {
                return;
            }
            schedule_restore();
        }
    }
);

/// Set while a restore pass is queued, so a burst of frame notifications (the inspector reframes
/// several views) collapses into one pass.
static PENDING: AtomicBool = AtomicBool::new(false);

/// Set while a restore pass is running. `-[_WKInspector detach]` spins a **nested runloop**, which
/// dispatches queued main-thread work *inside* the detach — so without this a second pass starts
/// mid-detach, detaches again, nests again, and the app stops responding (measured: two nested
/// passes, then a hang). Deferring alone doesn't prevent this; only refusing to re-enter does.
static IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// How long to let WebKit's attach transaction settle before undoing it.
const SETTLE_MS: u64 = 300;

/// Queue a restore once the attach has settled.
///
/// **Footgun — this must never run inline from the notification**, for the two separate reasons in
/// the module docs: the notification arrives nested inside a `setFrame:` (so touching the window
/// registry re-enters Tauri mid-op), and the detach would land mid-attach and never return. Both
/// were reproduced.
fn schedule_restore() {
    let Some(app) = APP.get() else {
        return;
    };
    if PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    // Off the main thread, so the settle delay costs the UI nothing.
    std::thread::spawn(move || {
        // Let WebKit finish the attach before undoing it: detaching mid-transaction blocks
        // indefinitely (measured — the app wedged with the detach never returning).
        std::thread::sleep(std::time::Duration::from_millis(SETTLE_MS));
        let inner = app.clone();
        let _ = app.run_on_main_thread(move || {
            PENDING.store(false, Ordering::Release);
            restore_all(&inner);
        });
    });
}

/// Put every content webview back where its window's hole is, with no inspector attached.
///
/// Window-wide rather than per-view because the notification's view pointer can't safely outlive
/// the deferral, and because this is idempotent: re-applying a layout that's already correct sets
/// identical frames, which AppKit doesn't post a notification for — so the pass settles instead of
/// re-triggering itself.
fn restore_all(app: &AppHandle) {
    if IN_PROGRESS.swap(true, Ordering::AcqRel) {
        return; // re-entered from detach's nested runloop — the outer pass is still finishing
    }
    let labels: Vec<String> = WATCHED.lock().unwrap().iter().flatten().cloned().collect();
    for label in labels {
        if let Some(webview) = app.get_webview(&label) {
            let _ = webview.with_webview(|pw| force_detached(pw.inner()));
        }
    }
    for (label, window) in app.windows() {
        if let Some(hole) = last_hole(&label) {
            layout_webviews(&window, hole);
        }
    }
    IN_PROGRESS.store(false, Ordering::Release);
}

/// Whether `view`'s Web Inspector exists and is showing.
fn inspector_visible(view: *mut AnyObject) -> bool {
    unsafe {
        let has: bool = msg_send![view, respondsToSelector: sel!(_inspector)];
        if !has {
            return false;
        }
        let inspector: *mut AnyObject = msg_send![view, _inspector];
        if inspector.is_null() {
            return false;
        }
        let visible: bool = msg_send![inspector, isVisible];
        visible
    }
}

/// Start watching the content webview `label` (native view `webview_ptr`) for the
/// inspector-driven reframe described in the module docs. [`super::content`] calls it for every
/// content webview it creates.
pub(crate) fn watch(webview_ptr: *mut c_void, label: &str, app: &AppHandle) {
    let _ = APP.set(app.clone());
    WATCHED
        .lock()
        .unwrap()
        .get_or_insert_with(HashSet::new)
        .insert(label.to_string());
    let watcher = WATCHER.get_or_init(|| unsafe { msg_send![Watcher::alloc(), init] });
    unsafe {
        let Some(view) = (webview_ptr as *mut AnyObject).as_ref() else {
            return;
        };
        let _: () = msg_send![view, setPostsFrameChangedNotifications: true];
        let center: *mut AnyObject = msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
        let name = NSString::from_str("NSViewFrameDidChangeNotification");
        let _: () = msg_send![
            center,
            addObserver: &**watcher,
            selector: sel!(frameDidChange:),
            name: &*name,
            object: view,
        ];
    }
}

/// Force the inspector attached to `webview_ptr` into its own window. No-op when it is already
/// detached, when the inspector doesn't exist yet, or on a WebKit without the SPI.
fn force_detached(webview_ptr: *mut c_void) {
    unsafe {
        let Some(view) = (webview_ptr as *mut AnyObject).as_ref() else {
            return;
        };
        // `_inspector` and `-detach` are WebKit SPI (the same `_inspector` wry's `open_devtools`
        // uses). Guard both on respondsToSelector: a WebKit that renames them then degrades to
        // "the inspector stays attached" instead of aborting on an unrecognised selector.
        let has_inspector: bool = msg_send![view, respondsToSelector: sel!(_inspector)];
        if !has_inspector {
            return;
        }
        let inspector: *mut AnyObject = msg_send![view, _inspector];
        let Some(inspector) = inspector.as_ref() else {
            return;
        };
        let can_detach: bool = msg_send![inspector, respondsToSelector: sel!(detach)];
        if can_detach {
            let _: () = msg_send![inspector, detach];
        }
    }
}
