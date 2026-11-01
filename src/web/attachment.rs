//! Save a `Content-Disposition: attachment` response instead of rendering it. wry's
//! `decidePolicyForNavigationResponse` downloads only what WebKit cannot display, so a displayable
//! attachment (a PDF, an image, text) is shown in place — and when a page fetches it into a hidden
//! iframe, as Google Docs' File → Download does, it renders invisibly and nothing saves. This
//! wraps wry's implementation on its navigation-delegate class: an attachment becomes a download
//! (handed to the `on_download` handler `content` always registers), everything else goes to wry.

use block2::Block;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{ffi, msg_send, sel};
use objc2_foundation::NSString;
use std::ffi::c_void;
use std::sync::{Once, OnceLock};

/// `WKNavigationResponsePolicyAllow` / `…Download`.
const POLICY_ALLOW: isize = 1;
const POLICY_DOWNLOAD: isize = 2;

type DecideFn =
    unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *mut c_void);

/// wry's own implementation, called for every response that is not an attachment.
static WRY_DECIDE: OnceLock<DecideFn> = OnceLock::new();

/// Whether a `Content-Disposition` value asks for the body to be saved (RFC 6266: the
/// disposition type is the first token, case-insensitive).
pub fn is_attachment(disposition: &str) -> bool {
    disposition
        .split(';')
        .next()
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("attachment"))
}

/// The navigation response's `Content-Disposition`, when it is an HTTP response carrying one.
unsafe fn disposition(nav_response: *mut AnyObject) -> Option<String> {
    let nav_response = unsafe { nav_response.as_ref()? };
    let response: Option<Retained<AnyObject>> = unsafe { msg_send![nav_response, response] };
    let response = response?;
    let http = AnyClass::get(c"NSHTTPURLResponse")?;
    let is_http: bool = unsafe { msg_send![&*response, isKindOfClass: http] };
    if !is_http {
        return None;
    }
    let name = NSString::from_str("Content-Disposition");
    let value: Option<Retained<NSString>> =
        unsafe { msg_send![&*response, valueForHTTPHeaderField: &*name] };
    value.map(|v| v.to_string())
}

/// `- webView:decidePolicyForNavigationResponse:decisionHandler:` in front of wry's.
unsafe extern "C-unwind" fn decide(
    this: *mut AnyObject,
    cmd: Sel,
    webview: *mut AnyObject,
    nav_response: *mut AnyObject,
    handler: *mut c_void,
) {
    let attachment = unsafe { disposition(nav_response) }.is_some_and(|d| is_attachment(&d));
    match WRY_DECIDE.get() {
        Some(wry) if !attachment => unsafe { wry(this, cmd, webview, nav_response, handler) },
        _ => {
            let policy = if attachment {
                POLICY_DOWNLOAD
            } else {
                POLICY_ALLOW
            };
            if let Some(block) = unsafe { (handler as *mut Block<dyn Fn(isize)>).as_ref() } {
                block.call((policy,));
            }
        }
    }
}

/// Put [`decide`] in front of wry's response policy on the content webview's navigation-delegate
/// class. Idempotent: the class is wry's one delegate class, patched on the first call.
pub fn ensure_patched(webview_ptr: *mut c_void) {
    unsafe {
        let Some(webview) = (webview_ptr as *mut AnyObject).as_ref() else {
            return;
        };
        let delegate: *mut AnyObject = msg_send![webview, navigationDelegate];
        let Some(delegate) = delegate.as_ref() else {
            return;
        };
        let cls: &AnyClass = delegate.class();
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let sel = sel!(webView:decidePolicyForNavigationResponse:decisionHandler:);
            let method = ffi::class_getInstanceMethod(cls, sel);
            if method.is_null() {
                return;
            }
            let Some(wry) = ffi::method_getImplementation(method) else {
                return;
            };
            let _ = WRY_DECIDE.set(std::mem::transmute::<Imp, DecideFn>(wry));
            let ours: Imp = std::mem::transmute(decide as DecideFn);
            ffi::method_setImplementation(method.cast_mut(), ours);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::is_attachment;

    #[test]
    fn the_first_token_decides() {
        for d in [
            "attachment",
            "attachment; filename=\"Bulletin.pdf\"",
            "Attachment;filename*=UTF-8''Bulletin.pdf",
            "  attachment ; filename=x",
        ] {
            assert!(is_attachment(d), "{d}");
        }
        for d in [
            "inline",
            "inline; filename=\"a.pdf\"",
            "",
            "attachments",
            "x; attachment",
        ] {
            assert!(!is_attachment(d), "{d}");
        }
    }
}
