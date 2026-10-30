//! Leave the page's global `ipc` name free.
//!
//! wry installs `window.ipc` on every webview with an IPC handler (Tauri gives every webview one)
//! through `Object.defineProperty(window, 'ipc', { value: … })`, which makes it non-configurable
//! and read-only. A classic script that declares a top-level `function ipc` then throws at
//! instantiation — "Can't declare global function 'ipc'" — and none of that script runs. Google
//! Sheets ships builds whose minified bundle declares exactly that global, and the half-loaded
//! page shows its "Loading issue" dialog.
//!
//! Content pages never use Tauri IPC (the shell talks to them through sentinel navigations), so
//! the property is made configurable and writable: the page may take the name, and nothing breaks
//! when it does. The definition can't be changed once a page has run it, so [`relax`] rewrites
//! wry's user script in the webview's content controller before the first real load
//! ([`super::content::create_content_webview`] starts each webview on `about:blank` for this).

use std::ffi::c_void;

use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::NSString;

/// The start of wry's `window.ipc` definition (`src/wkwebview/mod.rs`, 0.57.1).
const WRY_IPC_DEFINE: &str = "Object.defineProperty(window, 'ipc', { value:";

/// `source` with the `ipc` property made configurable and writable, when it is wry's definition.
fn relaxed(source: &str) -> Option<String> {
    let rest = source.strip_prefix(WRY_IPC_DEFINE)?;
    Some(format!(
        "Object.defineProperty(window, 'ipc', {{ configurable: true, writable: true, value:{rest}"
    ))
}

/// Rewrite wry's `window.ipc` user script on `webview_ptr` (a `WKWebView`) as [`relaxed`]. Every
/// script is re-added in its original order. No-op when wry's script isn't found.
pub(crate) fn relax(webview_ptr: *mut c_void) {
    unsafe {
        let Some(view) = (webview_ptr as *mut AnyObject).as_ref() else {
            return;
        };
        let Some(cls) = AnyClass::get(c"WKUserScript") else {
            return;
        };
        let config: Retained<AnyObject> = msg_send![view, configuration];
        let controller: Retained<AnyObject> = msg_send![&*config, userContentController];
        let scripts: Retained<AnyObject> = msg_send![&*controller, userScripts];
        let count: usize = msg_send![&*scripts, count];
        let mut kept: Vec<Retained<AnyObject>> = Vec::with_capacity(count);
        let mut found = false;
        for i in 0..count {
            let script: Retained<AnyObject> = msg_send![&*scripts, objectAtIndex: i];
            let source: Retained<NSString> = msg_send![&*script, source];
            match relaxed(&source.to_string()) {
                Some(new_source) if !found => {
                    found = true;
                    let time: isize = msg_send![&*script, injectionTime];
                    let main_only: bool = msg_send![&*script, isForMainFrameOnly];
                    let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
                    let replaced: Retained<AnyObject> = msg_send![
                        alloc,
                        initWithSource: &*NSString::from_str(&new_source),
                        injectionTime: time,
                        forMainFrameOnly: main_only,
                    ];
                    kept.push(replaced);
                }
                _ => kept.push(script),
            }
        }
        if !found {
            return;
        }
        let _: () = msg_send![&*controller, removeAllUserScripts];
        for script in &kept {
            let _: () = msg_send![&*controller, addUserScript: &**script];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wry 0.57.1's exact script.
    const WRY_0_57_1: &str = r#"Object.defineProperty(window, 'ipc', { value: Object.freeze({ postMessage: function(s) { window.webkit.messageHandlers.ipc.postMessage(s) } }) });"#;

    #[test]
    fn wry_ipc_definition_becomes_configurable_and_writable() {
        assert_eq!(
            relaxed(WRY_0_57_1).as_deref(),
            Some(
                r#"Object.defineProperty(window, 'ipc', { configurable: true, writable: true, value: Object.freeze({ postMessage: function(s) { window.webkit.messageHandlers.ipc.postMessage(s) } }) });"#
            )
        );
    }

    #[test]
    fn other_scripts_are_left_alone() {
        assert_eq!(relaxed("window.__TAURI_INTERNALS__ = {};"), None);
    }
}
