//! Accept self-signed / invalid TLS certificates for explicitly allowlisted hosts (homelab
//! devices). WKWebView rejects invalid certs and wry's navigation delegate has no
//! authentication-challenge handler, so we add `webView:didReceiveAuthenticationChallenge:`
//! to that delegate's class at runtime — scoped to the configured hosts only.

use block2::Block;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{ffi, msg_send, sel};
use objc2_foundation::NSString;
use std::ffi::c_void;
use std::sync::{Once, RwLock};

static ALLOW: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Set which hosts may use self-signed certs, replacing the previous list. Call at startup and
/// on every config reload: a host removed from the config stops being trusted at once, and one
/// added works without a restart.
pub fn set_allowlist(hosts: Vec<String>) {
    *ALLOW.write().unwrap_or_else(|e| e.into_inner()) = hosts;
}

fn host_allowed(host: &str) -> bool {
    ALLOW
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|h| h == host)
}

// NSURLSessionAuthChallengeDisposition values.
const USE_CREDENTIAL: isize = 0;
const PERFORM_DEFAULT_HANDLING: isize = 1;

/// `- (void)webView:didReceiveAuthenticationChallenge:completionHandler:` implementation.
unsafe extern "C-unwind" fn did_receive_auth_challenge(
    _this: *mut AnyObject,
    _cmd: Sel,
    _webview: *mut AnyObject,
    challenge: *mut AnyObject,
    completion: *mut Block<dyn Fn(isize, *mut AnyObject)>,
) {
    let accept = || -> Option<*mut AnyObject> {
        let challenge = challenge.as_ref()?;
        let ps: Retained<AnyObject> = msg_send![challenge, protectionSpace];
        let method: Retained<NSString> = msg_send![&*ps, authenticationMethod];
        if method.to_string() != "NSURLAuthenticationMethodServerTrust" {
            return None;
        }
        let host: Retained<NSString> = msg_send![&*ps, host];
        if !host_allowed(&host.to_string()) {
            return None;
        }
        let trust: *mut c_void = msg_send![&*ps, serverTrust];
        if trust.is_null() {
            return None;
        }
        let cls = AnyClass::get(c"NSURLCredential")?;
        let cred: *mut AnyObject = msg_send![cls, credentialForTrust: trust];
        Some(cred)
    };

    let (disposition, credential) = match accept() {
        Some(cred) => (USE_CREDENTIAL, cred),
        None => (PERFORM_DEFAULT_HANDLING, std::ptr::null_mut()),
    };
    if let Some(block) = completion.as_ref() {
        block.call((disposition, credential));
    }
}

/// Add the auth-challenge handler to the content webview's navigation-delegate class. Idempotent
/// (only adds once). Installed even while the allowlist is empty — a reload can add a host later,
/// and the handler falls through to default handling for every host not on the list.
pub fn ensure_patched(webview_ptr: *mut c_void) {
    static ONCE: Once = Once::new();
    unsafe {
        let Some(webview) = (webview_ptr as *mut AnyObject).as_ref() else {
            return;
        };
        let delegate: *mut AnyObject = msg_send![webview, navigationDelegate];
        let Some(delegate) = delegate.as_ref() else {
            return;
        };
        let cls: &AnyClass = delegate.class();
        ONCE.call_once(|| {
            let imp: Imp = std::mem::transmute(
                did_receive_auth_challenge
                    as unsafe extern "C-unwind" fn(
                        *mut AnyObject,
                        Sel,
                        *mut AnyObject,
                        *mut AnyObject,
                        *mut Block<dyn Fn(isize, *mut AnyObject)>,
                    ),
            );
            ffi::class_addMethod(
                (core::ptr::from_ref(cls) as *mut AnyClass).cast(),
                sel!(webView:didReceiveAuthenticationChallenge:completionHandler:),
                imp,
                c"v@:@@@?".as_ptr(),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allowlist_is_replaced_on_reload() {
        set_allowlist(vec!["nas.lan".into()]);
        assert!(host_allowed("nas.lan"));
        set_allowlist(vec!["udm.lan".into()]);
        assert!(
            !host_allowed("nas.lan"),
            "a removed host must stop being trusted"
        );
        assert!(host_allowed("udm.lan"));
        set_allowlist(vec![]);
        assert!(!host_allowed("udm.lan"));
    }
}
