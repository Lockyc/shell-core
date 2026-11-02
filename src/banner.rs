//! Native macOS banners, posted through **UserNotifications** (`UNUserNotificationCenter`) under the
//! app's own bundle identity. `tauri-plugin-notification` is no substitute: it posts through the
//! deprecated `NSUserNotification`, which on macOS 26 returns `Ok`, delivers nothing and never
//! registers the app in Notification Center.
//!
//! [`init`] requests authorization once and installs a delegate that presents banners even while
//! the app is frontmost (the system otherwise drops them for the foreground app, which is exactly
//! when a hidden tab raises one). Each [`Banner`] carries the window and tab that raised it, plus
//! an optional app `context`; a click hands them back to the app's `on_click` as a [`Click`],
//! which raises a window ([`raise`]) and selects the tab.

use tauri::{AppHandle, Manager};

/// A banner to post.
pub struct Banner<'a> {
    pub title: &'a str,
    pub body: &'a str,
    /// The label of the window that raised it.
    pub window: &'a str,
    /// The tab that raised it, in the app's own terms.
    pub tab: &'a str,
    /// Anything else the app needs back on a click.
    pub context: Option<&'a str>,
}

/// A clicked banner's [`Banner`] fields, handed to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Click {
    pub window: String,
    pub tab: String,
    pub context: Option<String>,
}

/// What the app does when a banner is clicked.
pub type OnClick = dyn Fn(&AppHandle, &Click) + Send + Sync;

/// Where [`set_trace`] sends each step.
pub type Trace = dyn Fn(&str) + Send + Sync;

/// Bring window `label` forward, unminimized, activating the app. A window closed since does
/// nothing.
pub fn raise(app: &AppHandle, label: &str) {
    if let Some(win) = app.get_window(label) {
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Banner, Click, OnClick, Trace};
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
    use tauri::AppHandle;

    /// Set once [`init`] has requested authorization and installed the delegate. Stays false in
    /// dev: with no app bundle, `currentNotificationCenter` throws on the nil bundle identifier.
    static READY: AtomicBool = AtomicBool::new(false);

    /// The click delegate runs with no context of its own; it reaches the app through this.
    static CLICK: OnceLock<(AppHandle, Box<OnClick>)> = OnceLock::new();

    /// The app's trace sink ([`super::set_trace`]).
    static TRACE: OnceLock<Box<Trace>> = OnceLock::new();

    /// Request identifiers must differ, or a new banner replaces the pending one.
    static SEQ: AtomicU64 = AtomicU64::new(0);

    const WINDOW_KEY: &str = "window";
    const TAB_KEY: &str = "tab";
    const CONTEXT_KEY: &str = "context";

    pub fn trace(msg: &str) {
        if let Some(f) = TRACE.get() {
            f(msg);
        }
    }

    pub fn set_trace(f: Box<Trace>) {
        let _ = TRACE.set(f);
    }

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
                trace("willPresent fired -> returning .banner | .sound (the app's side is done)");
                let opts = UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::Sound;
                completion_handler.call((opts,));
            }

            // Only a click on the banner body reaches the app; a dismiss or custom action does
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
                        if let Some((app, on_click)) = CLICK.get() {
                            let context = dict_str(&info, CONTEXT_KEY);
                            on_click(
                                app,
                                &Click {
                                    window,
                                    tab,
                                    context,
                                },
                            );
                        }
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

    pub fn fire(b: &Banner) {
        if !READY.load(Ordering::Acquire) {
            trace("fire: not ready -> no-op (dev build, or before init)");
            return;
        }
        trace(&format!(
            "fire: posting banner for {}/{} title={:?}",
            b.window, b.tab, b.title
        ));
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let content = UNMutableNotificationContent::init(UNMutableNotificationContent::alloc());
        content.setTitle(&NSString::from_str(b.title));
        content.setBody(&NSString::from_str(b.body));

        let mut pairs = vec![(WINDOW_KEY, b.window), (TAB_KEY, b.tab)];
        pairs.extend(b.context.map(|c| (CONTEXT_KEY, c)));
        let keys: Vec<Retained<NSString>> =
            pairs.iter().map(|(k, _)| NSString::from_str(k)).collect();
        let vals: Vec<Retained<NSString>> =
            pairs.iter().map(|(_, v)| NSString::from_str(v)).collect();
        let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let info: Retained<NSDictionary<NSString, NSString>> =
            NSDictionary::from_retained_objects(&key_refs, &vals);
        // SAFETY: only the phantom generic parameters differ; the object is the same.
        let info: Retained<NSDictionary> = unsafe { Retained::cast_unchecked(info) };
        // SAFETY: NSString pairs are property-list types, valid `userInfo`.
        unsafe { content.setUserInfo(&info) };

        let ident = NSString::from_str(&format!("banner-{}", SEQ.fetch_add(1, Ordering::Relaxed)));
        let request =
            UNNotificationRequest::requestWithIdentifier_content_trigger(&ident, &content, None);
        // Traced, the completion records whether usernoted accepted the request: the boundary
        // past which a missing banner is the OS's, not the app's.
        if TRACE.get().is_some() {
            let cb = RcBlock::new(|err: *mut NSError| {
                trace(&format!(
                    "fire: addNotificationRequest accepted by usernoted (err_null={})",
                    err.is_null()
                ));
            });
            center.addNotificationRequest_withCompletionHandler(&request, Some(&cb));
        } else {
            center.addNotificationRequest_withCompletionHandler(&request, None);
        }
    }
}

/// Request authorization and install the click delegate. Call once from the app's setup, on the
/// main thread. A no-op for banners in dev (no app bundle).
pub fn init(app: &AppHandle, on_click: impl Fn(&AppHandle, &Click) + Send + Sync + 'static) {
    #[cfg(target_os = "macos")]
    imp::init(app, Box::new(on_click));
    #[cfg(not(target_os = "macos"))]
    let _ = (app, on_click);
}

/// Show `banner`. A no-op until [`init`] has run (and in dev). Main thread only.
pub fn fire(banner: &Banner) {
    #[cfg(target_os = "macos")]
    imp::fire(banner);
    #[cfg(not(target_os = "macos"))]
    let _ = banner;
}

/// Send a line per step of the banner path (posted, accepted by the OS, presented) to `f`: a
/// diagnostic for a banner that never shows. Set once, before [`init`]; unset, nothing is traced.
pub fn set_trace(f: impl Fn(&str) + Send + Sync + 'static) {
    #[cfg(target_os = "macos")]
    imp::set_trace(Box::new(f));
    #[cfg(not(target_os = "macos"))]
    let _ = f;
}

/// Trace `msg` through the [`set_trace`] sink, so an app's own steps on the banner path land in
/// the same log.
pub fn trace(msg: &str) {
    #[cfg(target_os = "macos")]
    imp::trace(msg);
    #[cfg(not(target_os = "macos"))]
    let _ = msg;
}
