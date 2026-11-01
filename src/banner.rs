//! Native macOS banners, posted through **UserNotifications** (`UNUserNotificationCenter`) under the
//! app's own bundle identity. `tauri-plugin-notification` is no substitute: it posts through the
//! deprecated `NSUserNotification`, which on macOS 26 returns `Ok`, delivers nothing and never
//! registers the app in Notification Center.
//!
//! [`init`] requests authorization once and installs a delegate that presents banners even while
//! the app is frontmost (the system otherwise drops them for the foreground app, which is exactly
//! when a hidden tab raises one). Each banner carries the `(window label, tab label)` that raised
//! it; clicking it raises that window and hands both to the app's `on_click`, which selects the
//! tab. Every content webview's finished downloads are reported here (`web::content`).

use tauri::AppHandle;

/// What the app does when a banner is clicked, after its window has been raised:
/// `(app, window label, tab label)`.
pub type OnClick = dyn Fn(&AppHandle, &str, &str) + Send + Sync;

/// The banner reporting a finished download of `name`: `(title, body)`.
pub fn download_text(name: &str, success: bool) -> (&'static str, String) {
    if success {
        ("Downloaded", format!("{name} — saved to Downloads"))
    } else {
        ("Download failed", name.to_string())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::OnClick;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::OnceLock;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, AnyThread};
    use objc2_foundation::{NSDictionary, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
        UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
        UNNotificationRequest, UNNotificationResponse, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };
    use tauri::{AppHandle, Manager};

    /// Set once [`init`] has requested authorization and installed the delegate. Stays false in
    /// dev: with no app bundle, `currentNotificationCenter` throws on the nil bundle identifier.
    static READY: AtomicBool = AtomicBool::new(false);

    /// The click delegate runs with no context of its own; it reaches the app through this.
    static CLICK: OnceLock<(AppHandle, Box<OnClick>)> = OnceLock::new();

    /// Request identifiers must differ, or a new banner replaces the pending one.
    static SEQ: AtomicU64 = AtomicU64::new(0);

    const WINDOW_KEY: &str = "window";
    const TAB_KEY: &str = "tab";

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "ShellCoreBannerDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                let opts = UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::Sound;
                completion_handler.call((opts,));
            }

            // Only a click on the banner body surfaces the tab; a dismiss or custom action does
            // nothing. Delivered on the main thread. The completion handler must always be called.
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                completion_handler: &block2::DynBlock<dyn Fn()>,
            ) {
                if &*response.actionIdentifier() == unsafe { UNNotificationDefaultActionIdentifier }
                {
                    let info = response.notification().request().content().userInfo();
                    if let (Some(window), Some(tab)) =
                        (dict_str(&info, WINDOW_KEY), dict_str(&info, TAB_KEY))
                    {
                        clicked(&window, &tab);
                    }
                }
                completion_handler.call(());
            }
        }
    );

    /// A string out of a banner's `userInfo`, which [`fire`] fills with `NSString` pairs only.
    fn dict_str(info: &NSDictionary, key: &str) -> Option<String> {
        // SAFETY: every key and value `fire` stamps is an `NSString`.
        let typed: &NSDictionary<NSString, NSString> =
            unsafe { &*(info as *const NSDictionary as *const NSDictionary<NSString, NSString>) };
        typed
            .objectForKey(&NSString::from_str(key))
            .map(|s| s.to_string())
    }

    /// Raise `window` (gone since → nothing raised) and hand the click to the app.
    fn clicked(window: &str, tab: &str) {
        let Some((app, on_click)) = CLICK.get() else {
            return;
        };
        if let Some(win) = app.get_window(window) {
            let _ = win.unminimize();
            let _ = win.set_focus();
        }
        on_click(app, window, tab);
    }

    pub fn init(app: &AppHandle, on_click: Box<OnClick>) {
        let _ = CLICK.set((app.clone(), on_click));
        if tauri::is_dev() {
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        // The center holds its delegate weakly: leak it as an app-lifetime singleton.
        let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        std::mem::forget(delegate);

        let opts = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound;
        let handler = RcBlock::new(|granted: Bool, _err: *mut NSError| {
            if !granted.as_bool() {
                eprintln!("notification authorization not granted: banners are suppressed");
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(opts, &handler);
        READY.store(true, Ordering::Release);
    }

    pub fn fire(title: &str, body: &str, window: &str, tab: &str) {
        if !READY.load(Ordering::Acquire) {
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let content = UNMutableNotificationContent::init(UNMutableNotificationContent::alloc());
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));

        let keys = [NSString::from_str(WINDOW_KEY), NSString::from_str(TAB_KEY)];
        let vals = [NSString::from_str(window), NSString::from_str(tab)];
        let info: Retained<NSDictionary<NSString, NSString>> =
            NSDictionary::from_retained_objects(&[&*keys[0], &*keys[1]], &vals);
        // SAFETY: only the phantom generic parameters differ; the object is the same.
        let info: Retained<NSDictionary> = unsafe { Retained::cast_unchecked(info) };
        // SAFETY: NSString pairs are property-list types, valid `userInfo`.
        unsafe { content.setUserInfo(&info) };

        let ident = NSString::from_str(&format!("banner-{}", SEQ.fetch_add(1, Ordering::Relaxed)));
        let request =
            UNNotificationRequest::requestWithIdentifier_content_trigger(&ident, &content, None);
        center.addNotificationRequest_withCompletionHandler(&request, None);
    }
}

/// Request authorization and install the click delegate; `on_click` runs after a clicked banner's
/// window is raised. Call once from the app's setup, on the main thread. A no-op for banners in
/// dev (no app bundle).
pub fn init(app: &AppHandle, on_click: impl Fn(&AppHandle, &str, &str) + Send + Sync + 'static) {
    #[cfg(target_os = "macos")]
    imp::init(app, Box::new(on_click));
    #[cfg(not(target_os = "macos"))]
    let _ = (app, on_click);
}

/// Show a banner raised by tab `tab` in window `window`. A no-op until [`init`] has run (and in
/// dev). Main thread only.
pub fn fire(title: &str, body: &str, window: &str, tab: &str) {
    #[cfg(target_os = "macos")]
    imp::fire(title, body, window, tab);
    #[cfg(not(target_os = "macos"))]
    let _ = (title, body, window, tab);
}

#[cfg(test)]
mod tests {
    use super::download_text;

    #[test]
    fn a_download_banner_names_the_file() {
        assert_eq!(
            download_text("Bulletin.pdf", true),
            ("Downloaded", "Bulletin.pdf — saved to Downloads".into())
        );
        assert_eq!(
            download_text("Bulletin.pdf", false),
            ("Download failed", "Bulletin.pdf".into())
        );
    }
}
