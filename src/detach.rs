//! The detached-tab window: label scheme + the banner-shell surface a popped-out tab's window
//! shows. Mirrors [`crate::home`] throughout — same custom-protocol-serves-a-static-page shape,
//! same hand-rolled escaped-JSON payload, same reasons — so read that module's docs for the "why"
//! behind each piece here.
//!
//! - The **label scheme** (`DETACH_LABEL_PREFIX`/`detached_label`/`is_detached_label`/
//!   `detach_token`) marks a window as an ephemeral "popped out" tab so hot-reload reconcile and
//!   window-state persistence both know to skip it, the same exclusion [`crate::home::HOME_LABEL`]
//!   gets, generalized to an unbounded set of ephemeral windows (one per detached tab).
//! - The **banner-shell page** (`DETACH_SCHEME`/`DetachSpec`/`register_detach_protocol`) is the
//!   slim identity banner (title + accent stripe) a detached window shows above its transparent
//!   content hole, reporting that hole's rect to the app via `set_hole_rect` — every app already
//!   exposes that command identically.
//!
//! Touches no config-core symbol — the cores stay mutually independent.

/// Prefix marking a window label as a detached-tab window. A label under this prefix is never a
/// real (config-defined) window label, so reconcile and window-state persistence can both use
/// [`is_detached_label`] to skip it — the same exclusion `home::HOME_LABEL` gets, generalized to
/// an unbounded set of ephemeral windows (one per detached tab) rather than a single fixed label.
pub const DETACH_LABEL_PREFIX: &str = "shell-detach:";

/// Build the Tauri window label for a detached tab identified by `token` (an opaque,
/// caller-chosen identifier — e.g. the tab's own key).
pub fn detached_label(token: &str) -> String {
    format!("{DETACH_LABEL_PREFIX}{token}")
}

/// Whether `label` names a detached-tab window (as opposed to a real config-defined window or the
/// home surface). Reconcile and window-state persistence use this to skip these windows.
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
/// detached window's commands (`set_hole_rect`, and whatever return-to-window command later work
/// adds) need no extra capability wiring beyond what each app already ships. See
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
pub struct DetachSpec {
    pub title: String,
    pub colour: Option<String>,
    pub width: f64,
    pub height: f64,
}

/// Build the `window.__SHELL_DETACH__` payload `detach.html` reads: a small hand-rolled JSON
/// literal (every string run through [`crate::home::js_string_escape`] — the same function
/// `home::payload_json` uses, not a second copy) rather than pulling in `serde_json` for one fixed
/// object. Only the page-relevant fields are embedded: `width`/`height` size the *window* at
/// creation time, the page itself never reads them.
fn detach_payload_json(spec: &DetachSpec, app_name: &str) -> String {
    let colour = match &spec.colour {
        Some(c) => format!("\"{}\"", crate::home::js_string_escape(c)),
        None => "null".to_string(),
    };
    format!(
        "{{\"appName\":\"{}\",\"title\":\"{}\",\"colour\":{colour}}}",
        crate::home::js_string_escape(app_name),
        crate::home::js_string_escape(&spec.title),
    )
}

/// Register [`DETACH_SCHEME`], serving [`DETACH_HTML`] for any request. Mirrors
/// [`crate::home::register_protocol`] exactly. Chained into
/// [`register_plugins`](crate::register_plugins) alongside the home surface's protocol.
pub(crate) fn register_detach_protocol<R: tauri::Runtime>(
    builder: tauri::Builder<R>,
) -> tauri::Builder<R> {
    builder.register_uri_scheme_protocol(DETACH_SCHEME, |_ctx, _req| {
        tauri::http::Response::builder()
            .header(
                tauri::http::header::CONTENT_TYPE,
                "text/html; charset=utf-8",
            )
            .body(DETACH_HTML.as_bytes().to_vec())
            .expect("static response body")
    })
}

/// Open a detached tab's window: the banner-shell surface (this window's *primary* webview,
/// serving [`DETACH_HTML`] over [`DETACH_SCHEME`]) plus whatever content the caller docks into it.
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
pub fn open_detached<R, F>(
    app: &tauri::AppHandle<R>,
    token: &str,
    spec: &DetachSpec,
    app_name: &str,
    birth_content: F,
) -> tauri::Result<String>
where
    R: tauri::Runtime,
    F: FnOnce(&tauri::WebviewWindow<R>) -> tauri::Result<()>,
{
    let label = detached_label(token);
    let payload = detach_payload_json(spec, app_name);

    let url: tauri::Url = format!("{DETACH_SCHEME}://localhost/")
        .parse()
        .expect("DETACH_SCHEME url is a fixed, valid literal");

    // The payload is embedded as an escaped JS STRING (not a raw object literal), exactly like
    // `home::show_home`'s `window.__SHELL_HOME__` — the page `JSON.parse`s it.
    let window =
        tauri::WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::CustomProtocol(url))
            .title(&spec.title)
            .inner_size(spec.width, spec.height)
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .transparent(true)
            .initialization_script(format!(
                "window.__SHELL_DETACH__ = \"{}\";",
                crate::home::js_string_escape(&payload)
            ))
            .build()?;

    if let Err(e) = birth_content(&window) {
        let _ = window.close();
        return Err(e);
    }

    Ok(label)
}

/// Install the detached window's return orchestration: when the window closes, run `on_close`.
///
/// shell-core owns only the *when* — the window's `Destroyed` event — never the *what*. The app's
/// `on_close` closure owns all origin bookkeeping: reopening the origin window if the user closed
/// it while the tab was detached, redocking the content back into that origin, clearing the app's
/// own "this tab is detached" placeholder, and rebuilding the menu to drop the now-gone window's
/// entry. shell-core does not track which origin window/tab a detached window came from anywhere
/// in its own state — that association lives entirely in the app's manager, captured by the
/// closure the app passes in here. Mirrors how `home.rs` leaves "Create a starter config" to the
/// app: shell-core wires the moment, the app supplies the behaviour.
pub fn wire_return<R, F>(window: &tauri::WebviewWindow<R>, on_close: F)
where
    R: tauri::Runtime,
    F: Fn() + Send + 'static,
{
    window.on_window_event(move |e| {
        if let tauri::WindowEvent::Destroyed = e {
            on_close();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
            },
            "warden",
        );
        assert!(s.contains("a\\\"b"));
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
            },
            "lector",
        );
        assert!(s.contains("\"colour\":null"));
    }
}
