//! The home surface: what the app shows when it would otherwise have no window.
//!
//! It exists so the app is **never stranded invisible**: one surface covers no config, a load
//! error, and a valid config's window list, so a fresh install never launches to nothing.
//!
//! shell-core owns the surface and its state machine; the **app wires the actions**. In particular
//! "Create a starter config" is the app's handler calling `config_core::write_default_config` with
//! its own template — shell-core never touches config-core, and the cores stay mutually
//! independent.

use crate::menu::WindowEntry;

/// The label of the home window. Excluded from geometry persistence structurally
/// (`geometry::is_excluded`); never list it in `register_plugins`' `skip_labels`.
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

/// The init script that hands a page its payload: the JSON text embedded as a JS string literal
/// (`serde_json` quoting a string is a valid JS string), which the page `JSON.parse`s. Shared by
/// the home and detach surfaces so their embedding can't diverge.
pub(crate) fn payload_init_script(global: &str, payload: &str) -> String {
    let literal = serde_json::to_string(payload).expect("a string always serialises");
    format!("window.{global} = {literal};")
}

/// Build the `window.__SHELL_HOME__` payload `home.html` reads.
fn payload_json(state: &HomeState, app_name: &str) -> String {
    let v = match state {
        HomeState::NoConfig { path } => {
            serde_json::json!({ "appName": app_name, "state": "no_config", "path": path })
        }
        HomeState::Broken { path, error } => serde_json::json!({
            "appName": app_name, "state": "broken", "path": path, "error": error,
        }),
        HomeState::Windows { windows } => {
            let windows: Vec<_> = windows
                .iter()
                .map(|w| {
                    serde_json::json!({
                        "id": w.id, "title": w.title, "open": w.open, "colour": w.colour,
                    })
                })
                .collect();
            serde_json::json!({ "appName": app_name, "state": "windows", "windows": windows })
        }
    };
    v.to_string()
}

/// Register a custom protocol serving the fixed `html` for any request — the home and detach
/// surfaces each register one through this.
pub(crate) fn serve_static<R: tauri::Runtime>(
    builder: tauri::Builder<R>,
    scheme: &'static str,
    html: &'static str,
) -> tauri::Builder<R> {
    builder.register_uri_scheme_protocol(scheme, move |_ctx, _req| {
        tauri::http::Response::builder()
            .header(
                tauri::http::header::CONTENT_TYPE,
                "text/html; charset=utf-8",
            )
            .body(html.as_bytes().to_vec())
            .expect("static response body")
    })
}

/// Open (or refresh) the home surface. Idempotent, mirroring warden's `show_launcher`: if it's
/// already open, push a fresh payload via [`HOME_REFRESH_EVENT`] instead of rebuilding — the page
/// only fetches `window.__SHELL_HOME__` once, on load. Otherwise builds a standalone
/// [`tauri::WebviewWindow`] — a single webview created *as the window's primary content* (label
/// == [`HOME_LABEL`]), serving [`HOME_HTML`] over [`HOME_SCHEME`], with the state injected as
/// `window.__SHELL_HOME__` via an `initialization_script`.
///
/// **Not** a bare `WindowBuilder` + `add_child` — see [`close_home`]'s doc for why. Every *real* content window in every
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
    // The payload is embedded as a JS STRING (not a raw object literal) the page `JSON.parse`s,
    // so the init-script path and the refresh-event path (whose payload also arrives JS-side as a
    // string, per Tauri's event serialization of a Rust `String`) are parsed identically.
    tauri::WebviewWindowBuilder::new(app, HOME_LABEL, tauri::WebviewUrl::CustomProtocol(url))
        .title(app_name)
        .inner_size(560.0, 480.0)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .initialization_script(payload_init_script("__SHELL_HOME__", &payload))
        .build()?;
    Ok(())
}

/// Close the home surface if open. Safe no-op otherwise.
///
/// Plain `w.close()`. That is sufficient only because [`show_home`] gives the window its webview
/// at construction (`WebviewWindowBuilder`), the way every real content window is built. A bare
/// `WindowBuilder` + one `add_child`ed webview is broken on macOS 26: `w.close()` returns `Ok(())`
/// and Tauri's own bookkeeping (`get_window`) drops the window immediately, but the underlying
/// window stays fully painted on screen. Do not reintroduce a `WindowBuilder` + `add_child`-only
/// construction for a window that has (or will only ever have) exactly one webview — give it that
/// webview at construction instead.
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
    serve_static(builder, HOME_SCHEME, HOME_HTML)
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
    fn init_script_embeds_the_payload_as_one_js_string() {
        let s = payload_init_script("__X__", "{\"a\":\"q\\\"\nz\"}");
        let literal = s
            .strip_prefix("window.__X__ = ")
            .and_then(|r| r.strip_suffix(';'))
            .expect("the assignment shape");
        let back: String = serde_json::from_str(literal).expect("a valid string literal");
        assert_eq!(back, "{\"a\":\"q\\\"\nz\"}");
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

    /// The page subscribes by a string literal it can't import, so a renamed constant would
    /// compile and silently stop refreshing an open home window.
    #[test]
    fn home_page_listens_for_the_refresh_event() {
        assert!(HOME_HTML.contains(&format!("'{HOME_REFRESH_EVENT}'")));
    }
}
