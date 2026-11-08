//! Undo, redo and select-all go to the focused web page first, and to the Edit menu only when the
//! page leaves them, as in a browser.
//!
//! wry's `WryWebView` refuses every key equivalent in a child webview (its workaround for
//! tauri#9426), so AppKit offers ⌘Z, ⇧⌘Z and ⌘A straight to the main menu, whose Edit items send
//! AppKit's `undo:`/`redo:`/`selectAll:`. Those reach WebKit's own editing only; an editor that
//! keeps its own undo history or selection (Google Docs, Sheets) acts on the keydown, which it then
//! never sees. A local `NSEvent` monitor hands those chords to the focused `WKWebView`'s own
//! `performKeyEquivalent:`, past wry's override, which gives the page the keydown. A keydown the
//! page leaves unhandled WebKit re-sends through `-[NSApplication sendEvent:]`, which no monitor
//! sees, so wry refuses it again and the Edit menu acts on it: a plain text field still undoes.
//!
//! Cut, copy and paste stay with the menu: WebKit turns those into the page's clipboard events, so
//! an editor already gets them. `tests/edit_keys.rs` pins both paths against this Mac's WebKit.

/// Whether the chord with these characters (ignoring modifiers) and modifiers goes to the page
/// first: ⌘Z, ⇧⌘Z and ⌘A, by character so every keyboard layout matches as the menu does.
pub fn page_first(chars: &str, cmd: bool, shift: bool, alt: bool, ctrl: bool) -> bool {
    if !cmd || alt || ctrl {
        return false;
    }
    match chars.to_lowercase().as_str() {
        "z" => true,
        "a" => !shift,
        _ => false,
    }
}

/// Installs the process-wide monitor. Call once on the main thread; [`crate::register_plugins`]
/// does, so every app has it.
#[cfg(target_os = "macos")]
pub fn install() {
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSEventModifierFlags};
    use std::ptr::NonNull;

    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        let ev = unsafe { event.as_ref() };
        let flags = ev.modifierFlags();
        let chars = ev
            .charactersIgnoringModifiers()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let chord = page_first(
            &chars,
            flags.contains(NSEventModifierFlags::Command),
            flags.contains(NSEventModifierFlags::Shift),
            flags.contains(NSEventModifierFlags::Option),
            flags.contains(NSEventModifierFlags::Control),
        );
        let Some(mtm) = MainThreadMarker::new().filter(|_| chord) else {
            return event.as_ptr();
        };
        let responder = NSApplication::sharedApplication(mtm)
            .keyWindow()
            .and_then(|w| w.firstResponder());
        match responder {
            Some(r) if offer(&r, ev) => std::ptr::null_mut(),
            _ => event.as_ptr(),
        }
    });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block)
    };
    std::mem::forget(block);
    std::mem::forget(monitor);
}

/// Offers `event` to `responder`'s page when `responder` is a `WKWebView`, through WKWebView's own
/// `performKeyEquivalent:` (never a subclass override). True when WebKit took it, which it does
/// whenever the webview is its window's first responder; WebKit then answers the page's verdict.
#[cfg(target_os = "macos")]
pub fn offer(responder: &objc2_app_kit::NSResponder, event: &objc2_app_kit::NSEvent) -> bool {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    let Some(wk) = AnyClass::get(c"WKWebView") else {
        return false;
    };
    let obj: &AnyObject = responder.as_ref();
    let is_web: bool = unsafe { msg_send![obj, isKindOfClass: wk] };
    is_web && unsafe { msg_send![super(obj, wk), performKeyEquivalent: event] }
}

#[cfg(not(target_os = "macos"))]
pub fn install() {}

#[cfg(test)]
mod tests {
    use super::page_first;

    #[test]
    fn undo_redo_and_select_all_go_to_the_page() {
        assert!(page_first("z", true, false, false, false));
        assert!(page_first("Z", true, true, false, false));
        assert!(page_first("a", true, false, false, false));
    }

    #[test]
    fn every_other_chord_stays_with_the_menu() {
        assert!(!page_first("z", false, false, false, false));
        assert!(!page_first("z", true, false, true, false));
        assert!(!page_first("z", true, false, false, true));
        assert!(!page_first("A", true, true, false, false));
        for c in ["c", "v", "x", "w", "r", "f"] {
            assert!(!page_first(c, true, false, false, false), "⌘{c}");
        }
    }
}
