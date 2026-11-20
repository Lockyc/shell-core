//! The detached-tab window: label scheme + the banner-shell surface a popped-out tab's window
//! shows. Mirrors [`crate::home`] throughout — the same custom-protocol-serves-a-static-page shape
//! (`home::serve_static`) and payload embedding (`home::payload_init_script`) — so read that
//! module's docs for the "why" behind each piece here.
//!
//! - The **label scheme** (`DETACH_LABEL_PREFIX`/`detached_label`/`is_detached_label`/
//!   `detach_token`) marks a window as a "popped out" tab so a consuming app's hot-reload
//!   reconcile can skip it. The label is *stable per tab* — every consumer derives `token` from a
//!   durable tab identity — which is what lets [`crate::geometry`] persist a detached window's
//!   size and position like any other window's, rather than excluding it the way
//!   [`crate::home::HOME_LABEL`] is excluded.
//! - The **banner-shell page** (`DETACH_SCHEME`/`DetachSpec`/`register_detach_protocol`) is the
//!   slim identity banner (title + accent stripe) a detached window shows above its transparent
//!   content hole, reporting that hole's rect to the app via `set_hole_rect` — every app already
//!   exposes that command identically. An app that needs more than one hole or its own chrome
//!   supplies its own page through [`DetachSpec::page`] and gets the same `__SHELL_DETACH__`
//!   payload.
//!
//! Touches no config-core symbol — the cores stay mutually independent.

/// Prefix marking a window label as a detached-tab window. A label under this prefix is never a
/// real (config-defined) window label, so a consuming app's reconcile can use [`is_detached_label`]
/// to skip it.
pub const DETACH_LABEL_PREFIX: &str = "shell-detach:";

/// Build the Tauri window label for a detached tab identified by `token` (an opaque,
/// caller-chosen identifier — e.g. the tab's own key).
///
/// **`token` must be derived from a durable tab identity, not a per-session counter or an
/// allocation order.** [`crate::geometry`] keys a detached window's remembered size and position
/// on the resulting label, so a token that changes between sessions silently forgets the tab's
/// geometry and leaves an orphan entry behind each time. Every consumer already does this
/// (warden hashes `origin_label:tab_key`; curator and lector hash the tab's own label).
pub fn detached_label(token: &str) -> String {
    format!("{DETACH_LABEL_PREFIX}{token}")
}

/// Whether `label` names a detached-tab window (as opposed to a real config-defined window or the
/// home surface). A consuming app's reconcile uses this to skip these windows.
pub fn is_detached_label(label: &str) -> bool {
    label.starts_with(DETACH_LABEL_PREFIX)
}

/// The inverse of [`detached_label`]: recover the token from a detached-tab label, for routing an
/// event/command back to the right tab. `None` if `label` isn't a detached-tab label.
pub fn detach_token(label: &str) -> Option<&str> {
    label.strip_prefix(DETACH_LABEL_PREFIX)
}

/// The custom URI scheme [`register_detach_protocol`] registers on the app `Builder`, serving
/// [`DETACH_HTML`]. Mirrors [`crate::home::HOME_SCHEME`] exactly, including the reason: a
/// Builder-registered custom protocol is classified `local` by Tauri's ACL engine, so the
/// detached window's commands (`set_hole_rect`) need no extra capability wiring beyond what each
/// app already ships. See
/// `home::HOME_SCHEME`'s doc for the fuller "why not a `data:` URL" rationale — it applies
/// identically here.
const DETACH_SCHEME: &str = "shell-detach";

/// The embedded banner-shell page. Served at runtime over [`DETACH_SCHEME`] (never via
/// `WebviewUrl::App`, which would require materializing it into each consumer's own
/// `frontendDist`).
const DETACH_HTML: &str = include_str!("detach.html");

/// What a popped-out tab's banner shows, plus the size the detached window should open at.
/// `colour` is the tab's/window's accent colour (the same hex the sidebar swatch uses); `None`
/// falls back to the page's own default stripe colour.
///
/// `width`/`height` are the **first-pop-out default only**. Once this tab has been popped out and
/// resized, [`crate::geometry`] restores its remembered size and position over them during
/// [`open_detached`]'s `build()`. That is why `birth_content` is handed the window's real size —
/// size docked content from *that*, never from these fields.
pub struct DetachSpec {
    pub title: String,
    pub colour: Option<String>,
    pub width: f64,
    pub height: f64,
    /// The app's own page for this window, replacing the banner shell. `None` serves the
    /// banner shell over [`DETACH_SCHEME`]. Either way the `__SHELL_DETACH__` payload is injected.
    pub page: Option<tauri::WebviewUrl>,
}

/// The URL the detached window's primary webview loads: the app's [`DetachSpec::page`], else the
/// banner shell.
fn detach_url(spec: &DetachSpec) -> tauri::WebviewUrl {
    spec.page.clone().unwrap_or_else(|| {
        let url: tauri::Url = format!("{DETACH_SCHEME}://localhost/")
            .parse()
            .expect("DETACH_SCHEME url is a fixed, valid literal");
        tauri::WebviewUrl::CustomProtocol(url)
    })
}

/// Build the `window.__SHELL_DETACH__` payload `detach.html` reads. Only the page-relevant fields
/// are embedded: `width`/`height` size the *window* at creation time, the page itself never reads
/// them. `label` is the window's own Tauri label, which the page uses to drop an event
/// addressed to a sibling detached window (`emit_to` reaches every webview, so per-window events
/// are stamped and filtered). `loadError` is
/// the app's config load error at open time, so a pop-out opened while the config is broken shows
/// it from its first frame; [`ERROR_EVENT`] keeps it current after.
fn detach_payload_json(
    spec: &DetachSpec,
    app_name: &str,
    label: &str,
    load_error: Option<&str>,
) -> String {
    serde_json::json!({
        "appName": app_name, "title": spec.title, "label": label, "colour": spec.colour,
        "loadError": load_error,
    })
    .to_string()
}

/// Register [`DETACH_SCHEME`], serving [`DETACH_HTML`] for any request. Chained into
/// [`register_plugins`](crate::register_plugins) alongside the home surface's protocol.
pub(crate) fn register_detach_protocol<R: tauri::Runtime>(
    builder: tauri::Builder<R>,
) -> tauri::Builder<R> {
    crate::home::serve_static(builder, DETACH_SCHEME, DETACH_HTML)
}

/// The event [`crate::windows::WindowSet`] emits app-wide whenever it syncs the home surface:
/// `{ "error": <the config load error, or null> }`. Every pop-out shows the same error, so it
/// carries no label. A pop-out has no sidebar error bar and keeps the home surface away, so
/// without this a config broken while only pop-outs are open would be reported nowhere.
pub const ERROR_EVENT: &str = "shell-detach:error";

/// The banner's height in logical px: [`DETACH_HTML`]'s `#banner` (2.25rem at WebKit's default
/// 16px root).
pub const BANNER_H: f64 = 36.0;

/// Where docked content goes before the page's first `set_hole_rect` lands: everything below the
/// banner of a window whose real inner size is `size` (the size [`open_detached`] hands
/// `birth_content`). The page then reports the exact hole; this only keeps the first frame from
/// flashing full-height under the banner.
pub fn birth_hole(size: tauri::LogicalSize<f64>) -> crate::compositing::HoleRect {
    crate::compositing::HoleRect {
        x: 0.0,
        y: BANNER_H,
        width: size.width,
        height: (size.height - BANNER_H).max(0.0),
    }
}

/// Open a detached tab's window: the banner-shell surface (this window's *primary* webview,
/// by default serving [`DETACH_HTML`] over [`DETACH_SCHEME`]; [`DetachSpec::page`] replaces it)
/// plus whatever content the caller docks into it.
///
/// Mirrors [`crate::home::show_home`]'s construction exactly, for the same reason
/// [`crate::home::close_home`]'s doc records: a bare `WindowBuilder` + `add_child`ed webview is
/// confirmed broken on macOS 26 (`close()` returns `Ok` but the window stays painted on screen).
/// So this — like every real content window in every consumer — gives the window its webview at
/// construction via `WebviewWindowBuilder`, never adds one on afterward.
///
/// `token` identifies the detached tab (becomes [`detached_label`]); `spec` supplies the banner's
/// title/colour and the window's initial size; `app_name` flows into the payload the same way it
/// does for the home surface. After the window is built, `birth_content` gets a chance to dock the
/// app's own content into it (e.g. `add_child` a second webview, or hand a native surface its
/// rect) — on `Err`, the freshly-built window is closed and the error propagated, so a failed dock
/// never leaves an empty banner-only window behind. On success, returns the window's label so the
/// caller can look it up again (e.g. to call [`wire_return`]).
///
/// **`birth_content` is handed the window's real logical inner size, and must size its content
/// from that rather than from the `spec` it passed in.** By the time the closure runs,
/// [`crate::geometry`] has already restored this tab's remembered size over `spec`'s — its
/// `on_window_ready` hook runs inside `build()` — so `spec.width`/`spec.height` describe only the
/// never-popped-before case. Every consumer computes a birth rect for its docked content; handing
/// over the real size means a caller cannot use the stale value without reaching past the one it
/// was given. Falls back to `spec`'s size if the window's geometry can't be queried.
pub fn open_detached<R, F>(
    app: &tauri::AppHandle<R>,
    token: &str,
    spec: &DetachSpec,
    app_name: &str,
    birth_content: F,
) -> tauri::Result<String>
where
    R: tauri::Runtime,
    F: FnOnce(&tauri::WebviewWindow<R>, tauri::LogicalSize<f64>) -> tauri::Result<()>,
{
    use tauri::Manager as _;
    let label = detached_label(token);
    let load_error = app
        .try_state::<crate::windows::WindowSet>()
        .and_then(|set| set.load_error());
    let payload = detach_payload_json(spec, app_name, &label, load_error.as_deref());

    let window = tauri::WebviewWindowBuilder::new(app, &label, detach_url(spec))
        .title(&spec.title)
        .inner_size(spec.width, spec.height)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .transparent(true)
        .initialization_script(crate::home::payload_init_script(
            "__SHELL_DETACH__",
            &payload,
        ))
        .focused(crate::focus::focused())
        .build()?;

    // The size the window ACTUALLY has, which is `spec`'s only when this tab has no remembered
    // geometry — see this function's doc. A failed query falls back to `spec`'s size rather than
    // erroring: a birth rect that is merely stale costs one frame (`detach.html`'s `set_hole_rect`
    // corrects it), where refusing to dock would cost the whole pop-out.
    let real_size = window
        .scale_factor()
        .and_then(|scale| window.inner_size().map(|s| s.to_logical::<f64>(scale)))
        .unwrap_or_else(|_| tauri::LogicalSize::new(spec.width, spec.height));

    if let Err(e) = birth_content(&window, real_size) {
        let _ = window.close();
        return Err(e);
    }

    Ok(label)
}

/// Install the detached window's return orchestration: when the window (named by `label`) closes,
/// run `on_close`.
///
/// shell-core owns only the *when* — the window's `Destroyed` event — never the *what*. The app's
/// `on_close` closure owns all origin bookkeeping: reopening the origin window if the user closed
/// it while the tab was detached, redocking the content back into that origin, clearing the app's
/// own "this tab is detached" placeholder, and rebuilding the menu to drop the now-gone window's
/// entry. shell-core does not track which origin window/tab a detached window came from anywhere
/// in its own state — that association lives entirely in the app's manager, captured by the
/// closure the app passes in here. Mirrors how `home.rs` leaves "Create a starter config" to the
/// app: shell-core wires the moment, the app supplies the behaviour.
///
/// **Resolves the window by label via [`tauri::Manager::get_window`], deliberately — never
/// `get_webview_window`.** An app that docks its content into the detached window as an *added
/// child webview* (curator recreates a content webview; lector recreates its render webview) turns
/// it into a *multi-webview* window, and Tauri exposes multi-webview windows only through
/// `get_window`/`windows` — `get_webview_window`/`webview_windows` track single-webview windows
/// only and return `None`/empty for them, so a return wired off a `get_webview_window` lookup never
/// fires and the popped-out tab never redocks. `get_window` is correct for both
/// that case and the single-webview case (warden's detached window hosts a re-parented *native*
/// surface, not an added webview), and `Destroyed` is a window event regardless — so doing the one
/// correct lookup here, in the shared owner, keeps every consumer from having to pick it (and from
/// picking the wrong one). A no-op if no window carries `label` (already gone).
pub fn wire_return<R, F>(app: &tauri::AppHandle<R>, label: &str, on_close: F)
where
    R: tauri::Runtime,
    F: Fn() + Send + 'static,
{
    use tauri::Manager as _;
    if let Some(window) = app.get_window(label) {
        window.on_window_event(move |e| {
            if let tauri::WindowEvent::Destroyed = e {
                on_close();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_with(page: Option<tauri::WebviewUrl>) -> DetachSpec {
        DetachSpec {
            title: "t".into(),
            colour: None,
            width: 800.0,
            height: 600.0,
            page,
        }
    }

    /// The page subscribes by string literals it can't import, so a renamed constant would
    /// compile and silently stop `ERROR_EVENT` reaching it.
    #[test]
    fn detach_page_listens_for_the_error_event() {
        assert!(DETACH_HTML.contains("'shell-detach:error'"));
        assert_eq!(ERROR_EVENT, "shell-detach:error");
    }

    #[test]
    fn url_is_the_detach_page_by_default() {
        match detach_url(&spec_with(None)) {
            tauri::WebviewUrl::CustomProtocol(u) => {
                assert_eq!(u.as_str(), "shell-detach://localhost/")
            }
            other => panic!("expected the custom protocol, got {other:?}"),
        }
    }

    #[test]
    fn url_is_the_apps_page_when_supplied() {
        let spec = spec_with(Some(tauri::WebviewUrl::App("index.html".into())));
        match detach_url(&spec) {
            tauri::WebviewUrl::App(p) => assert_eq!(p, std::path::PathBuf::from("index.html")),
            other => panic!("expected the app page, got {other:?}"),
        }
    }

    #[test]
    fn banner_height_matches_the_page() {
        // `#banner`'s height in rem at the WebKit default 16px root (the page sets no root size).
        assert!(DETACH_HTML.contains("#banner { position: relative; height: 2.25rem;"));
        assert_eq!(BANNER_H, 2.25 * 16.0);
    }

    #[test]
    fn birth_hole_is_the_window_below_the_banner() {
        let h = birth_hole(tauri::LogicalSize::new(1000.0, 700.0));
        assert_eq!(
            (h.x, h.y, h.width, h.height),
            (0.0, BANNER_H, 1000.0, 700.0 - BANNER_H)
        );
        // A window shorter than the banner gets an empty hole, never a negative one.
        assert_eq!(birth_hole(tauri::LogicalSize::new(400.0, 10.0)).height, 0.0);
    }

    #[test]
    fn labels_round_trip() {
        let l = detached_label("abc123");
        assert_eq!(l, "shell-detach:abc123");
        assert!(is_detached_label(&l));
        assert_eq!(detach_token(&l), Some("abc123"));
    }

    #[test]
    fn real_window_labels_are_not_detached() {
        assert!(!is_detached_label("w1a2b3"));
        assert!(!is_detached_label(crate::home::HOME_LABEL));
        assert_eq!(detach_token("w1a2b3"), None);
    }

    #[test]
    fn payload_escapes_every_string_field() {
        // title/colour are attacker-shaped input (a config title, a hex the user typed) landing
        // inside an inline <script> — same threat model as home::payload_json.
        let s = detach_payload_json(
            &DetachSpec {
                title: "a\"b".into(),
                colour: Some("#fff".into()),
                width: 800.0,
                height: 600.0,
                page: None,
            },
            "warden",
            "shell-detach:x",
            Some("bad \"key\""),
        );
        assert!(s.contains("a\\\"b"));
        assert!(s.contains("\"loadError\":\"bad \\\"key\\\"\""));
        assert!(!s.contains("a\"b"));
        assert!(s.contains("\"colour\":\"#fff\""));
    }

    #[test]
    fn payload_colour_none_serialises_null() {
        let s = detach_payload_json(
            &DetachSpec {
                title: "t".into(),
                colour: None,
                width: 1.0,
                height: 1.0,
                page: None,
            },
            "lector",
            "shell-detach:x",
            None,
        );
        assert!(s.contains("\"colour\":null"));
        assert!(s.contains("\"loadError\":null"));
    }

    #[test]
    fn payload_has_no_pane_keys() {
        let json = detach_payload_json(&spec_with(None), "warden", "shell-detach:x", None);
        assert!(!json.contains("\"panes\""), "got: {json}");
        assert!(!json.contains("\"paneBand\""), "got: {json}");
    }

    #[test]
    fn payload_carries_the_window_label_for_event_filtering() {
        // The page drops an event stamped for a sibling window; that needs its own label,
        // which only the Rust side knows at open time.
        let spec = spec_with(None);
        let json = detach_payload_json(&spec, "warden", "shell-detach:abc", None);
        assert!(
            json.contains("\"label\":\"shell-detach:abc\""),
            "got: {json}"
        );
    }
}
