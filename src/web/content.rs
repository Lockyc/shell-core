//! The shared content-webview builder: one WKWebView hosting a remote site, built and destroyed
//! only through this module. Everything app-specific (awareness, notifications, inspector
//! watchers) is a hook on [`ContentHooks`] or something the caller does with the returned
//! [`tauri::Webview`]; everything here is identical for any app that hosts remote sites.

use std::sync::{Arc, OnceLock};

use tauri::{
    webview::{DownloadEvent, NewWindowResponse, WebviewBuilder},
    AppHandle, LogicalPosition, LogicalSize, Manager, WebviewUrl, Window,
};
use url::Url;

use super::{escape, session};
use crate::compositing::HoleRect;

/// Where a link or navigation came from, as handed to [`ContentHooks::intercept`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkOrigin {
    /// A `target=_blank` / `window.open` request.
    NewWindow,
    /// A plain click on a cross-host link, decoded from the escape sentinel (routed).
    Click,
    /// The page's own same-tab navigation.
    Navigation,
}

/// Everything [`create_content_webview`] needs to know about one tab.
pub struct ContentSpec<'a> {
    /// The webview label.
    pub label: &'a str,
    /// The page to load.
    pub url: &'a Url,
    /// The tab's same-site anchor for [`escape::route_link`].
    pub home_url: &'a str,
    /// Seed for [`session::data_store_id`]: tabs sharing a seed share a login.
    pub session: &'a str,
    /// The window's current content-hole rect, passed in (not read from app state) because
    /// callers already hold their windows lock when they create a tab — re-locking it here would
    /// self-deadlock the non-reentrant mutex.
    pub hole: HoleRect,
    /// Loading-bar colour as sRGB rgba (0–1); shell-core does not parse colours.
    pub bar_rgba: (f64, f64, f64, f64),
    /// App shims appended after the shared ones; `__SHELL_KEY__` is substituted in each.
    pub extra_init: &'a [&'a str],
}

type InterceptFn = dyn Fn(&AppHandle, &str, &Url, LinkOrigin) -> bool + Send + Sync;
type AppSentinelFn = dyn Fn(&AppHandle, &str, &Url) + Send + Sync;
type TitleFn = dyn Fn(&tauri::Webview, &str) + Send + Sync;
type DownloadFn = dyn Fn(&AppHandle, &str, &str, bool) + Send + Sync;

/// The app's say in a content webview's behaviour. All optional; the default does nothing extra.
#[derive(Default, Clone)]
pub struct ContentHooks {
    /// `(app, label, url, origin)`; return true to consume the link/navigation (nothing else
    /// happens). Consulted before any shell-core routing, in this order: first in `on_new_window`
    /// ([`LinkOrigin::NewWindow`]); for a routed escape-sentinel click, before
    /// [`escape::route_link`] ([`LinkOrigin::Click`]); and at the top of `on_navigation` for
    /// non-sentinel URLs, before the redirector check ([`LinkOrigin::Navigation`]).
    pub intercept: Option<Arc<InterceptFn>>,
    /// `(app, label, url)`: a key-verified sentinel that isn't the escape sentinel (the app's own
    /// notify/badge/… hosts). The navigation is swallowed either way.
    pub on_app_sentinel: Option<Arc<AppSentinelFn>>,
    /// The document title changed.
    pub on_title: Option<Arc<TitleFn>>,
    /// Finished download: `(app, label, file name or URL, success)`.
    pub on_download: Option<Arc<DownloadFn>>,
}

/// The frozen `AppleWebKit`/`Safari` build token every modern Safari UA carries. Safari stopped
/// advancing it years ago (it is a compatibility constant, not a real build number), so unlike the
/// marketing version below it does not decay.
const SAFARI_WEBKIT_TOKEN: &str = "605.1.15";
/// Used when [`installed_safari_version`] can't read a version — Safari removed, an unreadable
/// bundle, an unparseable string. Only a floor: a stale fallback behaves exactly like the old
/// pinned UA did (an "unsupported browser" banner), which is the failure this derivation removes,
/// so bump it when you notice, but the normal path never reads it.
const SAFARI_VERSION_FALLBACK: &str = "27.0";

/// Desktop UA for content webviews, derived once from the Safari installed on this machine.
///
/// WKWebView's native UA carries no `Version/… Safari/…` token, which Google Workspace (and
/// others) read as an unsupported browser — hence a spoof. It presents as **Safari, not Chrome**:
/// the engine genuinely is WebKit, so an honest UA gets the code path the renderer can actually
/// run (a Chrome UA invites Chrome-only paths that break here), and — the reason this is derived
/// rather than pinned — the version tracks macOS updates instead of decaying into that same
/// unsupported-browser banner a couple of years after someone last hard-coded it.
fn desktop_ua() -> &'static str {
    static UA: OnceLock<String> = OnceLock::new();
    UA.get_or_init(|| {
        let version =
            installed_safari_version().unwrap_or_else(|| SAFARI_VERSION_FALLBACK.to_string());
        format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
             AppleWebKit/{SAFARI_WEBKIT_TOKEN} (KHTML, like Gecko) \
             Version/{version} Safari/{SAFARI_WEBKIT_TOKEN}"
        )
    })
}

/// The installed Safari's marketing version, in the `major.minor` shape Safari's own UA uses.
/// `None` if Safari is absent or its version isn't plain numeric (see [`major_minor`]).
#[cfg(target_os = "macos")]
fn installed_safari_version() -> Option<String> {
    use objc2_foundation::{NSBundle, NSString};

    let bundle = NSBundle::bundleWithPath(&NSString::from_str("/Applications/Safari.app"))?;
    let value =
        bundle.objectForInfoDictionaryKey(&NSString::from_str("CFBundleShortVersionString"))?;
    major_minor(&value.downcast::<NSString>().ok()?.to_string())
}

#[cfg(not(target_os = "macos"))]
fn installed_safari_version() -> Option<String> {
    None
}

/// Trim a marketing version to the two components Safari's UA carries (`27.0.1` → `27.0`,
/// `27` → `27.0`). `None` unless both components are plain digits — a garbage version spliced
/// into the UA is worse than [`SAFARI_VERSION_FALLBACK`].
pub(crate) fn major_minor(raw: &str) -> Option<String> {
    let mut parts = raw.split('.');
    let major = parts.next()?;
    let minor = parts.next().unwrap_or("0");
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (numeric(major) && numeric(minor)).then(|| format!("{major}.{minor}"))
}

/// Click-interceptor that reroutes cmd/middle-clicks through the escape sentinel.
const ESCAPE_CLICK_JS: &str = include_str!("inject/escape-click.js");
/// Drives WebKit's `visibilitychange`/`focus` so live services keep syncing while hidden.
const VISIBILITY_SHIM_JS: &str = include_str!("inject/visibility.js");
/// Shows a hovered link's destination in a status overlay at the bottom of the tab.
const LINK_STATUS_JS: &str = include_str!("inject/link-status.js");

/// The placeholder every shim that emits a sentinel navigation carries for the per-webview key.
const KEY_PLACEHOLDER: &str = "__SHELL_KEY__";

/// A per-webview anti-forgery key, baked into that webview's injected shims as a function-local
/// literal (never exposed on `window`, so page scripts can't read it) and required on every
/// sentinel navigation. Drawn straight from the OS CSPRNG (`getrandom`) — 128 bits a page can't
/// guess to forge a banner/badge/browser-escape by navigating to a sentinel host directly. (Using
/// the CSPRNG directly, rather than hashing under `RandomState`, keeps the unforgeability property
/// from resting on any assumption about SipHash-as-PRF.)
fn random_nonce() -> String {
    use std::fmt::Write as _;
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("OS CSPRNG (getrandom) must succeed on macOS");
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// The full initialization script: the shared shims, then the app's `extra`, each with the key
/// placeholder replaced by `nonce`.
fn compose_init(nonce: &str, extra: &[&str]) -> String {
    [ESCAPE_CLICK_JS, VISIBILITY_SHIM_JS]
        .into_iter()
        .chain(extra.iter().copied())
        .chain([LINK_STATUS_JS])
        .map(|js| js.replace(KEY_PLACEHOLDER, nonce))
        .collect::<Vec<_>>()
        .join("\n;\n")
}

/// What a non-new-window navigation resolves to, before any side effect runs.
#[derive(Debug, PartialEq, Eq)]
enum NavAction {
    /// A sentinel that is forged, unkeyed, or undecodable: refuse and do nothing.
    Swallow,
    /// A key-verified escape-sentinel click.
    Escape(escape::LinkClick),
    /// A key-verified app-defined sentinel: refuse the navigation, tell the app.
    AppSentinel,
    /// A link redirector pointing off the tab's site: open the target in the browser.
    EscapeToBrowser(Url),
    /// Let the tab follow it.
    Allow,
}

/// Classify a navigation in a content webview. Sentinel hosts drive native banners / unread
/// badges / browser-escape, so they are honoured only when the navigation carries this webview's
/// secret `nonce` — otherwise a page could forge one by navigating to the host directly. Forged
/// or unrecognised sentinels are swallowed (never navigated to the dead host).
///
/// A link redirector reached in the *same* tab is still a leave-this-app intent, just one wearing
/// the tab's own domain — a service that wires its links without `target="_blank"` lands here
/// instead of `on_new_window`. It escapes to the browser rather than letting the tab follow the
/// bounce out, but only when the unwrapped destination is genuinely cross-site: a redirector
/// pointing back at `home_url`'s own site is in-app navigation and stays.
fn classify_navigation(url: &Url, nonce: &str, home_url: &str) -> NavAction {
    if escape::is_sentinel_host(url) {
        if !escape::sentinel_key_ok(url, nonce) {
            return NavAction::Swallow;
        }
        if url.host_str() == Some(escape::ESCAPE_SENTINEL_HOST) {
            return escape::escape_sentinel(url).map_or(NavAction::Swallow, NavAction::Escape);
        }
        return NavAction::AppSentinel;
    }
    if let Some(target) = escape::redirector_target(url) {
        if !escape::same_site(home_url, &target) {
            return NavAction::EscapeToBrowser(target);
        }
    }
    if escape::allow_same_tab_navigation(url.as_str()) {
        NavAction::Allow
    } else {
        NavAction::Swallow
    }
}

/// Act on a [`escape::LinkRoute`] for the tab `label`: navigate it, hand off to the browser, or
/// drop. The in-app navigation is deferred off the calling navigation-policy callback rather than
/// run re-entrantly from inside it.
fn apply_route(app: &AppHandle, label: &str, route: escape::LinkRoute) {
    match route {
        escape::LinkRoute::InApp(target) => {
            let app = app.clone();
            let label = label.to_string();
            let _ = app.clone().run_on_main_thread(move || {
                if let Some(wv) = app.get_webview(&label) {
                    let _ = wv.navigate(target);
                }
            });
        }
        escape::LinkRoute::Browser(target) => escape::escape_to_default_browser(target.as_str()),
        escape::LinkRoute::Refuse => {}
    }
}

/// Create a content webview for `spec` in `window`. Its login store is keyed on `spec.session`
/// (tabs sharing a seed share a login). Every tab gets the full shim set, so any loaded tab can
/// fire native banners and report unread — whether it notifies in the background is purely a
/// function of whether the app keeps it live.
///
/// Returns the webview so the app can attach its own watchers.
pub fn create_content_webview(
    window: &Window,
    spec: &ContentSpec,
    hooks: ContentHooks,
) -> tauri::Result<tauri::Webview> {
    // Per-webview secret keying the sentinel handlers; substituted into the shims that emit
    // sentinel navigations so only our own injected code can trigger them.
    let nonce = random_nonce();
    let init = compose_init(&nonce, spec.extra_init);

    let label = spec.label.to_string();
    // The tab's configured URL — its "home base" for the same-site test. Both handlers classify
    // against it (a link redirector can arrive as either a new-window request or a same-tab
    // navigation, depending on how the service wires its links), so each gets its own clone.
    let nav_home = spec.home_url.to_string();
    let open_home = spec.home_url.to_string();
    let nav_app = window.app_handle().clone();
    let open_app = nav_app.clone();
    let dl_app = nav_app.clone();
    let nav_label = label.clone();
    let open_label = label.clone();
    let dl_label = label.clone();
    let (nav_hooks, open_hooks, title_hooks, dl_hooks) =
        (hooks.clone(), hooks.clone(), hooks.clone(), hooks.clone());

    let builder = WebviewBuilder::new(spec.label, WebviewUrl::External(spec.url.clone()))
        // Let WKWebView deliver native OS file drops to the page's own HTML5 drop targets
        // (attach-to-compose, upload boxes, …). Tauri's default drag-drop handler consumes the
        // drop (emits a `tauri://drag-drop` event and returns `true`), which stops WKWebView from
        // ever seeing it. The drop lands on the active tab only: the app raises it to the front
        // of the superview, occluding the live-but-background tabs across the content rect.
        .disable_drag_drop_handler()
        .data_store_identifier(session::data_store_id(spec.session))
        .user_agent(desktop_ua())
        .initialization_script(&init)
        .on_new_window(move |url, _features| {
            if let Some(f) = &open_hooks.intercept {
                if f(&open_app, &open_label, &url, LinkOrigin::NewWindow) {
                    return NewWindowResponse::Deny;
                }
            }
            // The app's own popups/auth flows (same site as the tab) navigate the tab itself, so
            // sign-in completes in the tab's own login session; anything else escapes.
            match escape::route_link(&open_home, url) {
                escape::LinkRoute::InApp(url) => {
                    if let Some(wv) = open_app.get_webview(&open_label) {
                        let _ = wv.navigate(url);
                    }
                }
                escape::LinkRoute::Browser(url) => escape::escape_to_default_browser(url.as_str()),
                escape::LinkRoute::Refuse => {}
            }
            NewWindowResponse::Deny
        })
        .on_navigation(move |url| {
            if !escape::is_sentinel_host(url) {
                if let Some(f) = &nav_hooks.intercept {
                    if f(&nav_app, &nav_label, url, LinkOrigin::Navigation) {
                        return false;
                    }
                }
            }
            match classify_navigation(url, &nonce, &nav_home) {
                NavAction::Swallow => false,
                NavAction::Escape(click) => {
                    if !click.routed {
                        escape::escape_to_default_browser(click.target.as_str());
                        return false;
                    }
                    if let Some(f) = &nav_hooks.intercept {
                        if f(&nav_app, &nav_label, &click.target, LinkOrigin::Click) {
                            return false;
                        }
                    }
                    apply_route(
                        &nav_app,
                        &nav_label,
                        escape::route_link(&nav_home, click.target),
                    );
                    false
                }
                NavAction::AppSentinel => {
                    if let Some(f) = &nav_hooks.on_app_sentinel {
                        f(&nav_app, &nav_label, url);
                    }
                    false
                }
                NavAction::EscapeToBrowser(target) => {
                    escape::escape_to_default_browser(target.as_str());
                    false
                }
                NavAction::Allow => true,
            }
        })
        .on_document_title_changed(move |webview, title| {
            if let Some(f) = &title_hooks.on_title {
                f(&webview, &title);
            }
        })
        // Registering a handler at all is what makes WebKit treat attachments as downloads: without
        // one, wry cancels every `download`-attribute/blob link and *allows* an unrenderable
        // `Content-Disposition: attachment` response as a navigation, which WebKit then abandons
        // mid-load — the tab tries to navigate, nothing saves, and the loading bar strands. wry
        // picks a de-duplicated path in `~/Downloads`; the shell only accepts it and reports the
        // end.
        .on_download(move |_webview, event| {
            if let DownloadEvent::Finished { url, path, success } = event {
                let name = path
                    .as_deref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| url.to_string());
                if let Some(f) = &dl_hooks.on_download {
                    f(&dl_app, &dl_label, &name, success);
                }
            }
            true
        });

    let hole = spec.hole;
    let webview = window.add_child(
        builder,
        LogicalPosition::new(hole.x, hole.y),
        LogicalSize::new(hole.width.max(0.0), hole.height.max(0.0)),
    )?;
    #[cfg(target_os = "macos")]
    let _ = webview.with_webview(|pw| super::insecure::ensure_patched(pw.inner()));
    // Thin determinate loading bar at the top of this content webview.
    crate::progress_bar::install(&webview, spec.bar_rgba);
    Ok(webview)
}

/// Navigate the content webview `label` to `url` (reset / periodic reload to its canonical URL).
/// No-op if the webview hasn't been created yet (a never-opened lazy tab is skipped).
pub fn navigate_to(window: &Window, label: &str, url: &Url) -> tauri::Result<()> {
    if let Some(wv) = window.get_webview(label) {
        wv.navigate(url.clone())?;
    }
    Ok(())
}

/// Tear down a content webview: end its page, then close it.
///
/// **Footgun: `Webview::close()` does not stop the page.** wry's `InnerWebView::drop`
/// (`src/wkwebview/mod.rs`, 0.55.1) removes the `WKWebView` from its superview and then
/// deliberately over-retains it — `self.webview.retain(); self.manager.retain();`, with no
/// matching release — so the object is detached from the view hierarchy but never deallocated.
/// Its document keeps running: timers fire, sockets stay open, and the app's own injected shims
/// keep navigating the notify/badge sentinels into an `on_navigation` closure that is still alive
/// too. A "closed" tab therefore goes on raising native banners, and since each close leaves
/// another live copy behind, reopening the same tab stacks a second one — which is where
/// duplicate notifications come from. Measured: a tab dropped from the config kept accumulating
/// CPU in its own WebContent process long after its row left the sidebar.
///
/// **Ending the page must therefore be explicit, and it must be synchronous.** Navigating to
/// `about:blank` first does stop a page — but not in front of a close: the close drops wry's
/// navigation delegate mid-flight and the pending load is abandoned, leaving the original document
/// running. `_close` is not cancellable that way; by the time the webview is dropped there is no
/// page left to keep alive. Verified both ways against a tab whose page burns CPU on a timer: with
/// the `_close`, its WebContent process exits on close; without it, the process outlives the tab
/// and goes on accumulating CPU.
///
/// **Every path that destroys a content webview must come through here** — or, when a whole window
/// is going away and its teardown will drop the webviews, [`close_window_pages`]. A bare `close()`
/// silently reintroduces the leak.
pub fn close_content_webview(wv: &tauri::Webview) {
    close_page(wv);
    let _ = wv.close();
}

/// End the page of every content webview in `window`, for the paths that close a *window* rather
/// than a tab (user close, ⌘⇧W, a reload dropping the window, a detached window redocking). The
/// window's teardown drops those webviews, which leaks them still running exactly as above — so
/// their pages are ended first. The window's own main webview is its chrome (its label *is* the
/// window's), which the window takes with it; only the content children need this.
pub fn close_window_pages(window: &Window) {
    for (label, wv) in window.app_handle().webviews() {
        if label != window.label() && wv.window().label() == window.label() {
            close_page(&wv);
        }
    }
}

/// End the page a content webview is running, synchronously, via `-[WKWebView _close]`.
///
/// WebKit SPI, guarded on `respondsToSelector:`: a WebKit that renames it degrades to the leak
/// above rather than aborting on an unrecognised selector. There is no public equivalent — every
/// supported route to stopping a page is a navigation, and a navigation is exactly what the close
/// cancels.
fn close_page(wv: &tauri::Webview) {
    #[cfg(target_os = "macos")]
    {
        let _ = wv.with_webview(|pw| {
            use objc2::runtime::AnyObject;
            use objc2::{msg_send, sel};
            unsafe {
                let Some(view) = (pw.inner() as *mut AnyObject).as_ref() else {
                    return;
                };
                let can_close: bool = msg_send![view, respondsToSelector: sel!(_close)];
                if can_close {
                    let _: () = msg_send![view, _close];
                }
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = wv;
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "https://chat.example.com/";
    const NONCE: &str = "abc123";

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn major_minor_trims_and_rejects() {
        assert_eq!(major_minor("27.0"), Some("27.0".into()));
        assert_eq!(major_minor("27.0.1"), Some("27.0".into()));
        assert_eq!(major_minor("27"), Some("27.0".into()));
        assert_eq!(major_minor("Technology Preview"), None);
        assert_eq!(major_minor("27.x"), None);
        assert_eq!(major_minor(""), None);
    }

    #[test]
    fn init_script_substitutes_key_in_every_shim() {
        let extra = ["var k = \"__SHELL_KEY__\"; // app shim"];
        let init = compose_init("deadbeef", &extra);
        assert!(!init.contains("__SHELL_KEY__"));
        assert!(
            init.contains("var KEY = \"deadbeef\";"),
            "escape shim keyed"
        );
        assert!(init.contains("var k = \"deadbeef\";"), "extra shim keyed");
    }

    #[test]
    fn forged_sentinel_is_swallowed() {
        for s in [
            "https://shell.escape.invalid/?u=https%3A%2F%2Fevil.com%2F&r=1&k=wrong",
            "https://shell.escape.invalid/?u=https%3A%2F%2Fevil.com%2F",
            "https://curator.notify.invalid/?t=x&b=y&k=wrong",
        ] {
            assert_eq!(classify_navigation(&u(s), NONCE, HOME), NavAction::Swallow);
        }
    }

    #[test]
    fn keyed_app_sentinel_goes_to_app() {
        let url = u("https://curator.notify.invalid/?t=x&b=y&k=abc123");
        assert_eq!(
            classify_navigation(&url, NONCE, HOME),
            NavAction::AppSentinel
        );
    }

    #[test]
    fn keyed_escape_sentinel_is_escape() {
        let url = u("https://shell.escape.invalid/?u=https%3A%2F%2Fother.org%2Fx&r=1&k=abc123");
        assert_eq!(
            classify_navigation(&url, NONCE, HOME),
            NavAction::Escape(escape::LinkClick {
                target: u("https://other.org/x"),
                routed: true,
            })
        );
    }

    #[test]
    fn cross_site_redirector_escapes() {
        let url = u("https://www.google.com/url?q=https%3A%2F%2Fother.org%2Fx");
        assert_eq!(
            classify_navigation(&url, NONCE, HOME),
            NavAction::EscapeToBrowser(u("https://other.org/x"))
        );
    }

    #[test]
    fn ordinary_navigation_is_allowed() {
        assert_eq!(
            classify_navigation(&u("https://chat.example.com/room/1"), NONCE, HOME),
            NavAction::Allow
        );
        // A redirector back at the tab's own site stays in-app.
        let back = u("https://www.google.com/url?q=https%3A%2F%2Fchat.example.com%2Fa");
        assert_eq!(classify_navigation(&back, NONCE, HOME), NavAction::Allow);
    }
}
