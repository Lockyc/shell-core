//! Tell the chrome when the pointer leaves it for a content webview.
//!
//! The sidebar's hover affordances (row hover, the resize grip, the pop-out control) are CSS
//! `:hover`, which the chrome page recomputes only when it receives a pointer event. Content
//! webviews are `add_child` siblings composited *above* the full-window chrome webview, so the
//! instant the pointer crosses into the content hole the topmost webview takes it and the chrome
//! sees nothing more: a fast exit leaves `:hover` frozen on whatever it last saw. The chrome can't
//! observe its own exit, so the host reports it: [`POINTER_AWAY_EVENT`] to the window's chrome,
//! which calls chrome-core's `pointerAway()`.
//!
//! Each content webview gets an `NSTrackingArea` owned by one app-lifetime [`Watcher`]. AppKit
//! computes tracking areas by geometry, not hit-testing, so the entered event fires even though
//! the webview's own event handling is WebKit's. `InVisibleRect` keeps the area matched to the
//! view's frame across every hole change without re-installing it, and a hidden tab's area is
//! inert while hidden. The window is resolved at event time, so a webview re-parented into a
//! popped-out window reports there (no chrome listens, so it is inert).

use std::ffi::c_void;
use std::sync::OnceLock;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{class, define_class, msg_send, AnyThread};
use tauri::{AppHandle, Emitter, Manager};

/// The event emitted to a window's chrome (the webview labelled as the window) when the pointer
/// enters one of its content webviews. Each app's chrome page maps it to `pointerAway()`.
pub const POINTER_AWAY_EVENT: &str = "pointer-away";

/// `NSTrackingMouseEnteredAndExited | NSTrackingActiveAlways | NSTrackingInVisibleRect`.
/// Active-always because the chrome shows hover in a background window too, so it can freeze there.
const TRACKING_OPTIONS: usize = 0x01 | 0x80 | 0x200;

/// Captured on the first [`watch`] call — the owner's callbacks come from AppKit with no context
/// of their own.
static APP: OnceLock<AppHandle> = OnceLock::new();

/// The single tracking-area owner. `NSTrackingArea` holds its owner unretained, so it lives here.
static WATCHER: OnceLock<Retained<Watcher>> = OnceLock::new();

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ShellCorePointerWatcher"]
    struct Watcher;

    unsafe impl NSObjectProtocol for Watcher {}

    impl Watcher {
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &AnyObject) {
            let ns_window: *mut AnyObject = unsafe { msg_send![event, window] };
            if let Some(app) = APP.get() {
                pointer_entered_content(app, ns_window.cast());
            }
        }

        // `EnteredAndExited` sends both; an owner that doesn't answer `mouseExited:` aborts on an
        // unrecognised selector. Leaving content needs no report — the chrome sees its own entry.
        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &AnyObject) {}
    }
);

/// The pointer moved onto a content webview in `ns_window`: that window's chrome lost it.
fn pointer_entered_content(app: &AppHandle, ns_window: *mut c_void) {
    if ns_window.is_null() {
        return;
    }
    let label = app
        .windows()
        .into_iter()
        .find(|(_, w)| w.ns_window().is_ok_and(|p| p == ns_window))
        .map(|(label, _)| label);
    if let Some(label) = label {
        let _ = app.emit_to(label.as_str(), POINTER_AWAY_EVENT, ());
    }
}

/// Install the pointer-entered tracking area on content webview `webview`.
/// [`super::content::create_content_webview`] calls it for every webview it builds; an app that
/// composites a webview over its chrome some other way calls it itself.
pub fn watch(webview: &tauri::Webview) {
    let app = webview.app_handle().clone();
    let _ = webview.with_webview(move |pw| install(pw.inner(), &app));
}

fn install(webview_ptr: *mut c_void, app: &AppHandle) {
    let _ = APP.set(app.clone());
    let watcher = WATCHER.get_or_init(|| unsafe { msg_send![Watcher::alloc(), init] });
    unsafe {
        let Some(view) = (webview_ptr as *mut AnyObject).as_ref() else {
            return;
        };
        // The rect is ignored under `InVisibleRect`.
        let rect = objc2_foundation::NSRect::ZERO;
        let alloc: Allocated<AnyObject> = msg_send![class!(NSTrackingArea), alloc];
        let area: Option<Retained<AnyObject>> = msg_send![
            alloc,
            initWithRect: rect,
            options: TRACKING_OPTIONS,
            owner: &**watcher,
            userInfo: std::ptr::null::<AnyObject>(),
        ];
        if let Some(area) = area {
            let _: () = msg_send![view, addTrackingArea: &*area];
        }
    }
}
