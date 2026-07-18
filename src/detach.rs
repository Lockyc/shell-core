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
//! Not yet built: the window-opening function and the return-to-home orchestration (YAGNI until
//! there's a caller — later work).
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
///
/// `#[allow(dead_code)]`: only [`register_detach_protocol`] reads this, and that function isn't
/// chained into [`register_plugins`](crate::register_plugins) yet — no caller until the
/// window-opening orchestration lands. Remove the allow once that wiring exists.
#[allow(dead_code)]
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
/// creation time (later work), the page itself never reads them.
///
/// `#[allow(dead_code)]`: exercised by the escaping tests below; no production caller until the
/// window-opening orchestration (later work) builds a [`DetachSpec`] and calls this to construct
/// its `initialization_script`, mirroring [`crate::home::show_home`]. Remove the allow then.
#[allow(dead_code)]
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
/// [`crate::home::register_protocol`] exactly. Not yet chained into
/// [`register_plugins`](crate::register_plugins) — that lands with the window-opening
/// orchestration, once there's a caller.
///
/// `#[allow(dead_code)]` until that wiring exists — this task defines the protocol handler, the
/// next task chains it in (the same split [`register_protocol`](crate::home::register_protocol)
/// went through before `register_plugins` picked it up).
#[allow(dead_code)]
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
