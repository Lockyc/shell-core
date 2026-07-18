//! The home surface: what the app shows when it would otherwise have no window.
//!
//! It exists so the app is **never stranded invisible**. Before this, curator had an error window
//! (states an error, offers nothing) and warden had a launcher (offers windows, cannot express an
//! error) — two half-implementations of one idea, and lector had neither, so a fresh install
//! launched to nothing at all.
//!
//! shell-core owns the surface and its state machine; the **app wires the actions**. In particular
//! "Create a starter config" is the app's handler calling `config_core::write_default_config` with
//! its own template — shell-core never touches config-core, and the cores stay mutually
//! independent.

use crate::menu::WindowEntry;

/// The label of the home window. Pass it to `register_plugins`' `skip_labels` so its throwaway
/// bounds are never persisted or restored.
pub const HOME_LABEL: &str = "shell-home";

/// The custom URI scheme [`register_plugins`](crate::register_plugins) registers on the app
/// `Builder` (via [`register_protocol`]) to serve [`HOME_HTML`].
///
/// This is load-bearing, not cosmetic: Tauri's ACL engine classifies a webview as **local** or
/// **remote** by its navigated URL, and only a local origin matches the apps' existing capability
/// entries (`windows: ["*"], webviews: ["*"]`, no `remote` block — see lector's capabilities
/// footgun) without extra wiring. A `data:` URL or any other externally-navigated origin is
/// `Origin::Remote`, and the three `shell_home_*` commands the page invokes would need an explicit
/// `remote.urls` capability match — awkward to the point of impractical for a `data:` URL, which
/// has no stable origin pattern to match against. Registering our own scheme via
/// `register_uri_scheme_protocol` makes `is_local_url` true for it (Tauri treats any
/// Builder-registered custom protocol as local), so the home surface's webview needs no capability
/// changes beyond what the apps already ship.
const HOME_SCHEME: &str = "shell-home";

/// The event [`show_home`] pushes to an already-open home window with a fresh payload, mirroring
/// warden's `warden:launcher-refresh` — the page only fetches its initial state once on load.
const HOME_REFRESH_EVENT: &str = "shell:home-refresh";

/// The embedded page. Served at runtime over [`HOME_SCHEME`] (never via `WebviewUrl::App`, which
/// would require materializing it into each consumer's own `frontendDist` — this needs no per-app
/// build step).
const HOME_HTML: &str = include_str!("home.html");

#[derive(Debug)]
pub enum HomeState {
    /// No config file at all — offer to write one.
    NoConfig { path: String },
    /// The config exists but did not load. Last-good (if any) stays live.
    Broken { path: String, error: String },
    /// A valid config; these are its windows, none currently open.
    Windows { windows: Vec<WindowEntry> },
}

/// Which state the home surface should show, or `None` when a real window exists.
///
/// Precedence is deliberate: a real window beats everything (the surface only exists to prevent
/// invisibility); then no-config; then a load error; then the window list. An error must beat the
/// list, or a user staring at a window list would never learn their edit didn't parse.
///
/// Note a **warning** is not an error — a config with a missing `dir` loads fine and opens its
/// windows. Routing warnings here would resurrect exactly the stranding the warn-don't-error rule
/// exists to prevent.
pub fn home_state(
    has_windows: bool,
    config_exists: bool,
    config_path: &str,
    load_error: Option<&str>,
    windows: &[WindowEntry],
) -> Option<HomeState> {
    if has_windows {
        return None;
    }
    if !config_exists {
        return Some(HomeState::NoConfig {
            path: config_path.to_string(),
        });
    }
    if let Some(e) = load_error {
        return Some(HomeState::Broken {
            path: config_path.to_string(),
            error: e.to_string(),
        });
    }
    Some(HomeState::Windows {
        windows: windows.to_vec(),
    })
}

/// Escape a string for embedding inside a double-quoted JS (and, since the escapes we emit are a
/// subset of JSON's, JSON) string literal. Ported from curator's `js_string_escape` — curator's own
/// copy is deleted once it adopts this surface.
///
/// `pub(crate)` so [`crate::detach`] reuses it for its own payload builder instead of carrying a
/// second copy — one source of truth for the escaping both surfaces need.
pub(crate) fn js_string_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// One `WindowEntry` as a JSON object literal.
fn window_entry_json(w: &WindowEntry) -> String {
    let colour = match &w.colour {
        Some(c) => format!("\"{}\"", js_string_escape(c)),
        None => "null".to_string(),
    };
    format!(
        "{{\"id\":\"{}\",\"title\":\"{}\",\"open\":{},\"colour\":{colour}}}",
        js_string_escape(&w.id),
        js_string_escape(&w.title),
        w.open,
    )
}

/// Build the `window.__SHELL_HOME__` payload `home.html` reads: a small hand-rolled JSON literal
/// (every string run through [`js_string_escape`]) rather than pulling in `serde_json` for one
/// fixed, three-shape object.
fn payload_json(state: &HomeState, app_name: &str) -> String {
    let app = js_string_escape(app_name);
    match state {
        HomeState::NoConfig { path } => format!(
            "{{\"appName\":\"{app}\",\"state\":\"no_config\",\"path\":\"{}\"}}",
            js_string_escape(path)
        ),
        HomeState::Broken { path, error } => format!(
            "{{\"appName\":\"{app}\",\"state\":\"broken\",\"path\":\"{}\",\"error\":\"{}\"}}",
            js_string_escape(path),
            js_string_escape(error)
        ),
        HomeState::Windows { windows } => {
            let items = windows
                .iter()
                .map(window_entry_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("{{\"appName\":\"{app}\",\"state\":\"windows\",\"windows\":[{items}]}}")
        }
    }
}

/// Open (or refresh) the home surface. Idempotent, mirroring warden's `show_launcher`: if it's
/// already open, push a fresh payload via [`HOME_REFRESH_EVENT`] instead of rebuilding — the page
/// only fetches `window.__SHELL_HOME__` once, on load. Otherwise builds a standalone
/// [`tauri::WebviewWindow`] — a single webview created *as the window's primary content* (label
/// == [`HOME_LABEL`]), serving [`HOME_HTML`] over [`HOME_SCHEME`], with the state injected as
/// `window.__SHELL_HOME__` via an `initialization_script`.
///
/// **Not** a bare `WindowBuilder` + `add_child` (the shape this used to have, mirroring curator's
/// `build_error_window`) — see [`close_home`]'s doc for why. Every *real* content window in every
/// consumer gives its window a primary webview via `WebviewWindowBuilder` (only *additional*
/// webviews — content panes — are layered on with `add_child`); the home surface needs exactly
/// one webview, so it should build the same way instead of being the one bespoke construction.
pub fn show_home<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &HomeState,
    app_name: &str,
) -> tauri::Result<()> {
    use tauri::Manager;

    let payload = payload_json(state, app_name);

    if let Some(window) = app.get_window(HOME_LABEL) {
        use tauri::Emitter;
        let _ = app.emit_to(HOME_LABEL, HOME_REFRESH_EVENT, payload);
        let _ = window.set_focus();
        return Ok(());
    }

    let url: tauri::Url = format!("{HOME_SCHEME}://localhost/")
        .parse()
        .expect("HOME_SCHEME url is a fixed, valid literal");
    // The payload is embedded as an escaped JS STRING (not a raw object literal), exactly like
    // curator's `window.__CURATOR_ERROR__` — the page `JSON.parse`s it. This keeps the init-script
    // path and the refresh-event path (whose payload also arrives JS-side as a string, per Tauri's
    // event serialization of a Rust `String`) identical instead of one being an object and the
    // other a string to parse.
    tauri::WebviewWindowBuilder::new(app, HOME_LABEL, tauri::WebviewUrl::CustomProtocol(url))
        .title(app_name)
        .inner_size(560.0, 480.0)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .initialization_script(format!(
            "window.__SHELL_HOME__ = \"{}\";",
            js_string_escape(&payload)
        ))
        .build()?;
    Ok(())
}

/// Close the home surface if open. Safe no-op otherwise.
///
/// Plain `w.close()`. This surface's *previous* shape (a bare `WindowBuilder` + one
/// `add_child`ed webview and nothing else, mirroring curator's `build_error_window`) is confirmed
/// broken on macOS 26: `w.close()` returns `Ok(())` and Tauri's own bookkeeping (`get_window`)
/// drops the window immediately, but the underlying window stays fully painted on screen —
/// confirmed by screenshotting it *after* `close()` returned. [`show_home`] now builds this
/// window the same way every real content window builds its own (a primary webview via
/// `WebviewWindowBuilder`, not one `add_child`ed on afterward), removing the one construction-level
/// difference this surface had from the rest of the family. Do not reintroduce a `WindowBuilder` +
/// `add_child`-only construction for a window that has (or will only ever have) exactly one
/// webview — give it that webview at construction instead.
pub fn close_home<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager;
    if let Some(w) = app.get_window(HOME_LABEL) {
        let _ = w.close();
    }
}

/// Register [`HOME_SCHEME`], serving [`HOME_HTML`] for any request. Called from
/// [`register_plugins`](crate::register_plugins), which chains it alongside the other Tauri
/// runtime wiring every consumer needs identically.
pub(crate) fn register_protocol<R: tauri::Runtime>(
    builder: tauri::Builder<R>,
) -> tauri::Builder<R> {
    builder.register_uri_scheme_protocol(HOME_SCHEME, |_ctx, _req| {
        tauri::http::Response::builder()
            .header(
                tauri::http::header::CONTENT_TYPE,
                "text/html; charset=utf-8",
            )
            .body(HOME_HTML.as_bytes().to_vec())
            .expect("static response body")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> crate::menu::WindowEntry {
        crate::menu::WindowEntry {
            id: id.into(),
            title: id.into(),
            open: false,
            colour: None,
        }
    }

    #[test]
    fn real_windows_mean_no_home_surface() {
        // The surface exists only to stop the app being stranded invisible. A window is showing —
        // nothing to stop.
        assert!(home_state(true, true, "/c.toml", None, &[entry("w1")]).is_none());
        // Even mid-error: last-good config keeps its windows, and an error belongs in the chrome's
        // error bar, not a takeover surface.
        assert!(home_state(true, true, "/c.toml", Some("bad toml"), &[]).is_none());
    }

    #[test]
    fn no_config_offers_to_create_one() {
        let s = home_state(false, false, "/c.toml", None, &[]).unwrap();
        assert!(matches!(s, HomeState::NoConfig { path } if path == "/c.toml"));
    }

    #[test]
    fn a_load_error_beats_the_window_list() {
        // An error must win: showing a window list built from last-good config, while the file on
        // disk is broken, tells the user nothing is wrong.
        let s = home_state(false, true, "/c.toml", Some("expected `=`"), &[entry("w1")]).unwrap();
        assert!(matches!(s, HomeState::Broken { error, .. } if error == "expected `=`"));
    }

    #[test]
    fn a_valid_config_with_no_open_windows_lists_them() {
        let s = home_state(false, true, "/c.toml", None, &[entry("w1"), entry("w2")]).unwrap();
        match s {
            HomeState::Windows { windows } => assert_eq!(windows.len(), 2),
            other => panic!("expected Windows, got {other:?}"),
        }
    }

    #[test]
    fn a_valid_config_defining_no_windows_is_not_an_error() {
        // Distinct from Broken: the file parsed fine, it just has no [[window]] blocks. The list
        // is empty and the surface says so — it does not claim the config failed.
        let s = home_state(false, true, "/c.toml", None, &[]).unwrap();
        assert!(matches!(s, HomeState::Windows { windows } if windows.is_empty()));
    }

    #[test]
    fn escape_handles_quotes_backslashes_and_control_chars() {
        assert_eq!(js_string_escape("a\"b"), "a\\\"b");
        assert_eq!(js_string_escape("a\\b"), "a\\\\b");
        assert_eq!(js_string_escape("a\nb"), "a\\nb");
        assert_eq!(js_string_escape("a\u{1}b"), "a\\u0001b");
    }

    #[test]
    fn payload_json_escapes_every_string_field_it_embeds() {
        // A config path or a parser error message is attacker-shaped input (it can contain
        // whatever the user's filesystem/toml text does) landing inside an inline <script> — every
        // string field must go through escaping, or a `"` in a path/error breaks out of the literal.
        let s = payload_json(
            &HomeState::Broken {
                path: "/tmp/\"evil\".toml".to_string(),
                error: "line 1: unexpected \"".to_string(),
            },
            "lector",
        );
        assert!(s.contains("\\\"evil\\\""));
        assert!(!s.contains("unexpected \""), "unescaped quote in {s}");
    }

    #[test]
    fn payload_json_windows_state_serialises_each_entry() {
        let s = payload_json(
            &HomeState::Windows {
                windows: vec![
                    WindowEntry {
                        id: "w1".into(),
                        title: "Docs".into(),
                        open: true,
                        colour: Some("#ff0000".into()),
                    },
                    WindowEntry {
                        id: "w2".into(),
                        title: "Notes".into(),
                        open: false,
                        colour: None,
                    },
                ],
            },
            "lector",
        );
        assert!(s.contains("\"id\":\"w1\""));
        assert!(s.contains("\"colour\":\"#ff0000\""));
        assert!(s.contains("\"id\":\"w2\""));
        assert!(s.contains("\"colour\":null"));
    }
}
