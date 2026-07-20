//! Native mouse side-button (back/forward) navigation for content webviews — shared by curator and
//! lector (warden hosts native terminal surfaces with no page history and does not use it).
//!
//! macOS delivers a mouse's back/forward side buttons in one of two ways, and WKWebView acts on
//! neither — nor does it forward them to the DOM, which is why a page-level JS `mouseup` handler
//! never sees them (the reason the earlier injected-JS approach never worked). So we install a
//! local `NSEvent` monitor and drive the focused tab's WKWebView history natively
//! (`goBack`/`goForward`), the layer a real browser handles them at:
//!
//! - A plain mouse (no driver) sends them as `otherMouseDown` with buttonNumber 3 (back) / 4
//!   (forward).
//! - A mouse driver may instead route them through `systemDefined` subtype 7
//!   (`NX_SUBTYPE_AUX_MOUSE_BUTTONS`), whose `data1` is the bitmask of buttons that changed and
//!   `data2` the bitmask currently down — with the same button indices (bit 3 = back, bit 4 =
//!   forward).
//!
//! The core owns the monitor, the event decode, and the native call; the app supplies a `resolver`
//! returning the [`tauri::Webview`] to act on (its focused window's active content tab) — the one
//! piece that differs per app.

/// Install the process-wide mouse side-button navigation monitor. `resolver` is called on the main
/// thread when a back/forward button is pressed and returns the content webview to navigate (the
/// focused window's active tab), or `None` to do nothing. Call once from the Tauri setup hook,
/// which runs on the main thread as `NSEvent` monitors require.
#[cfg(target_os = "macos")]
pub fn install<F>(resolver: F)
where
    F: Fn() -> Option<tauri::Webview> + 'static,
{
    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};
    use std::ptr::NonNull;

    // Standard back/forward button indices — also the bit positions in the aux-mouse button masks.
    const BACK_BUTTON: isize = 3;
    const FWD_BUTTON: isize = 4;
    const BACK_MASK: isize = 1 << BACK_BUTTON; // 0x8
    const FWD_MASK: isize = 1 << FWD_BUTTON; // 0x10
    const AUX_MOUSE_SUBTYPE: i16 = 7; // NX_SUBTYPE_AUX_MOUSE_BUTTONS

    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        let ev = unsafe { event.as_ref() };
        let ty = ev.r#type();

        if ty == NSEventType::OtherMouseDown {
            // Plain-mouse path: the side buttons arrive as ordinary "other" mouse presses.
            match ev.buttonNumber() {
                BACK_BUTTON => navigate(&resolver, Nav::Back),
                FWD_BUTTON => navigate(&resolver, Nav::Forward),
                _ => {}
            }
        } else if ty == NSEventType::SystemDefined && ev.subtype().0 == AUX_MOUSE_SUBTYPE {
            // Driver path: aux-mouse-button state. Navigate on the press (down) transition only —
            // the button's bit is both freshly changed (data1) and currently down (data2).
            let changed = ev.data1();
            let down = ev.data2();
            if changed & BACK_MASK != 0 && down & BACK_MASK != 0 {
                navigate(&resolver, Nav::Back);
            } else if changed & FWD_MASK != 0 && down & FWD_MASK != 0 {
                navigate(&resolver, Nav::Forward);
            }
        }
        // Never consume: we act only on the press, so there is exactly one navigation per press, and
        // passing the event through avoids interfering with anything else that observes it.
        event.as_ptr()
    });

    // Keep both the block and the returned monitor object alive for the process lifetime: dropping
    // the returned `Retained` tears the monitor down immediately.
    let mask = NSEventMask(NSEventMask::OtherMouseDown.0 | NSEventMask::SystemDefined.0);
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    std::mem::forget(block);
    std::mem::forget(monitor);
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
enum Nav {
    Back,
    Forward,
}

/// Resolve the webview to act on and call `goBack`/`goForward` on its underlying WKWebView. Runs on
/// the main thread (from the NSEvent monitor), where `with_webview` resolves inline.
#[cfg(target_os = "macos")]
fn navigate<F>(resolver: &F, dir: Nav)
where
    F: Fn() -> Option<tauri::Webview>,
{
    let Some(wv) = resolver() else {
        return;
    };
    let _ = wv.with_webview(move |pw| unsafe {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;
        let Some(view) = (pw.inner() as *mut AnyObject).as_ref() else {
            return;
        };
        match dir {
            Nav::Back => {
                let _: *mut AnyObject = msg_send![view, goBack];
            }
            Nav::Forward => {
                let _: *mut AnyObject = msg_send![view, goForward];
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn install<F>(_resolver: F)
where
    F: Fn() -> Option<tauri::Webview> + 'static,
{
}
