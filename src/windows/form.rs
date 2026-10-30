//! The window form: one small shell-core-owned window for New Window (empty), Edit Window
//! (prefilled title + colour) and Delete Window (confirm). It reports a [`FormResult`]; the app
//! does the config write itself (shell-core never depends on config-core).
//!
//! The page is served over its own scheme (same reason as [`crate::home`]'s) and reports back by
//! requesting `/result?p=<json>` on that same scheme, which the protocol handler parses
//! ([`parse_result_request`]) and delivers: no app command and no capability grant (an event emit
//! would need `core:event:allow-emit`, which apps don't give). The page validates the title
//! (non-empty, not in `taken_titles`, trimmed) and the colour (`#rgb`/`#rrggbb`, config-core's
//! `Colour::parse` rule, reimplemented in the page) before enabling Save.

use serde::Deserialize;

/// The form window's label. Excluded from geometry persistence (`geometry::is_excluded`) and
/// reserved against window labels (`windows::plan::reserved_labels`).
pub const FORM_LABEL: &str = "shell-window-form";

const FORM_SCHEME: &str = "shell-window-form";
const FORM_HTML: &str = include_str!("form.html");

/// Rust -> page: a fresh payload for an already-open form.
const REFRESH_EVENT: &str = "shell:window-form-refresh";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormMode {
    New,
    Edit {
        title: String,
        colour: Option<String>,
    },
    ConfirmDelete {
        title: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FormResult {
    Create {
        title: String,
        colour: Option<String>,
    },
    Update {
        title: String,
        colour: Option<String>,
    },
    Delete,
    Cancel,
}

/// The `window.__SHELL_WINDOW_FORM__` payload the page reads.
fn payload_json(mode: &FormMode, taken_titles: &[String]) -> String {
    let (mode, title, colour) = match mode {
        FormMode::New => ("new", "", None),
        FormMode::Edit { title, colour } => ("edit", title.as_str(), colour.as_deref()),
        FormMode::ConfirmDelete { title } => ("delete", title.as_str(), None),
    };
    serde_json::json!({ "mode": mode, "title": title, "colour": colour, "taken": taken_titles })
        .to_string()
}

/// The path the page requests to report its [`FormResult`] (JSON in the `p` query parameter).
const RESULT_PATH: &str = "/result";

/// The [`FormResult`] a page request carries, or `None` for any other request.
fn parse_result_request(uri: &tauri::http::Uri) -> Option<FormResult> {
    if uri.path() != RESULT_PATH {
        return None;
    }
    let (_, json) = tauri::Url::parse(&uri.to_string())
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == "p")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))?;
    serde_json::from_str(&json).ok()
}

/// Serve [`FORM_HTML`], and deliver the page's result on [`RESULT_PATH`]: the callback runs first,
/// then the form closes itself (its `Destroyed` handler finds the callback already taken, so it
/// cannot answer twice).
pub(crate) fn register_protocol<R: tauri::Runtime>(
    builder: tauri::Builder<R>,
) -> tauri::Builder<R> {
    use tauri::Manager;
    builder.register_uri_scheme_protocol(FORM_SCHEME, |ctx, req| {
        let respond = |status: u16, ct: &str, body: Vec<u8>| {
            tauri::http::Response::builder()
                .status(status)
                .header(tauri::http::header::CONTENT_TYPE, ct)
                .body(body)
                .expect("static response")
        };
        if req.uri().path() == RESULT_PATH {
            return match parse_result_request(req.uri()) {
                Some(result) => {
                    finish(result);
                    if let Some(w) = ctx.app_handle().get_window(FORM_LABEL) {
                        let _ = w.close();
                    }
                    respond(200, "text/plain", b"ok".to_vec())
                }
                None => respond(400, "text/plain", b"bad result".to_vec()),
            };
        }
        respond(
            200,
            "text/html; charset=utf-8",
            FORM_HTML.as_bytes().to_vec(),
        )
    })
}

type Callback = Box<dyn FnOnce(FormResult) + Send>;

/// The pending callback of the one open form. Taken by whichever comes first: the page's result
/// or the window being destroyed (which reports `Cancel`).
static PENDING: std::sync::Mutex<Option<Callback>> = std::sync::Mutex::new(None);

fn finish(result: FormResult) {
    let cb = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(cb) = cb {
        cb(result);
    }
}

/// Open the form, or retarget it if one is already open (the earlier request is dropped without a
/// result). `on_result` runs once: with the page's choice, or `Cancel` if the window is closed.
pub fn open_form<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    mode: FormMode,
    taken_titles: Vec<String>,
    on_result: impl FnOnce(FormResult) + Send + 'static,
) {
    use tauri::{Emitter, Manager};

    let payload = payload_json(&mode, &taken_titles);
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(on_result));

    if let Some(window) = app.get_window(FORM_LABEL) {
        let _ = app.emit_to(FORM_LABEL, REFRESH_EVENT, payload);
        let _ = window.set_focus();
        return;
    }

    let url: tauri::Url = format!("{FORM_SCHEME}://localhost/")
        .parse()
        .expect("FORM_SCHEME url is a fixed, valid literal");
    let title = match mode {
        FormMode::New => "New Window",
        FormMode::Edit { .. } => "Edit Window",
        FormMode::ConfirmDelete { .. } => "Delete Window",
    };
    let built =
        tauri::WebviewWindowBuilder::new(app, FORM_LABEL, tauri::WebviewUrl::CustomProtocol(url))
            .title(title)
            .inner_size(380.0, 260.0)
            .resizable(false)
            .minimizable(false)
            .maximizable(false)
            .initialization_script(crate::home::payload_init_script(
                "__SHELL_WINDOW_FORM__",
                &payload,
            ))
            .build();
    let Ok(window) = built else {
        finish(FormResult::Cancel);
        return;
    };

    window.on_window_event(|e| {
        if matches!(e, tauri::WindowEvent::Destroyed) {
            finish(FormResult::Cancel);
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_result_round_trips_from_the_page_payload() {
        let r: FormResult =
            serde_json::from_str(r##"{"kind":"create","title":"Work","colour":"#abc"}"##).unwrap();
        assert_eq!(
            r,
            FormResult::Create {
                title: "Work".into(),
                colour: Some("#abc".into())
            }
        );
        let r: FormResult =
            serde_json::from_str(r#"{"kind":"update","title":"W","colour":null}"#).unwrap();
        assert_eq!(
            r,
            FormResult::Update {
                title: "W".into(),
                colour: None
            }
        );
        assert_eq!(
            serde_json::from_str::<FormResult>(r#"{"kind":"delete"}"#).unwrap(),
            FormResult::Delete
        );
        assert_eq!(
            serde_json::from_str::<FormResult>(r#"{"kind":"cancel"}"#).unwrap(),
            FormResult::Cancel
        );
        assert!(serde_json::from_str::<FormResult>(r#"{"kind":"bogus"}"#).is_err());
    }

    #[test]
    fn payload_carries_mode_prefill_and_taken_titles() {
        let v: serde_json::Value = serde_json::from_str(&payload_json(
            &FormMode::Edit {
                title: "A".into(),
                colour: Some("#fff".into()),
            },
            &["B".to_string()],
        ))
        .unwrap();
        assert_eq!(v["mode"], "edit");
        assert_eq!(v["title"], "A");
        assert_eq!(v["colour"], "#fff");
        assert_eq!(v["taken"][0], "B");
        let v: serde_json::Value =
            serde_json::from_str(&payload_json(&FormMode::New, &[])).unwrap();
        assert_eq!(v["mode"], "new");
        let v: serde_json::Value = serde_json::from_str(&payload_json(
            &FormMode::ConfirmDelete { title: "A".into() },
            &[],
        ))
        .unwrap();
        assert_eq!(v["mode"], "delete");
    }

    #[test]
    fn the_form_page_owns_the_events_and_the_colour_rule() {
        assert!(FORM_HTML.contains(RESULT_PATH));
        assert!(FORM_HTML.contains(REFRESH_EVENT));
    }

    #[test]
    fn a_result_request_parses_and_other_requests_do_not() {
        let u = |s: &str| s.parse::<tauri::http::Uri>().unwrap();
        assert_eq!(
            parse_result_request(&u(
                "shell-window-form://localhost/result?p=%7B%22kind%22%3A%22delete%22%7D"
            )),
            Some(FormResult::Delete)
        );
        assert_eq!(
            parse_result_request(&u("shell-window-form://localhost/")),
            None
        );
        assert_eq!(
            parse_result_request(&u("shell-window-form://localhost/result?p=nope")),
            None
        );
    }

    #[test]
    fn a_result_answers_once_and_a_later_cancel_is_silent() {
        let hits = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let h = hits.clone();
        *PENDING.lock().unwrap() = Some(Box::new(move |r| h.lock().unwrap().push(r)));
        finish(FormResult::Delete);
        finish(FormResult::Cancel); // the window's Destroyed handler
        assert_eq!(*hits.lock().unwrap(), vec![FormResult::Delete]);
    }

    #[test]
    fn the_form_page_has_the_colour_rule_and_the_inline_reasons() {
        assert!(FORM_HTML.contains("[0-9a-fA-F]{3}|[0-9a-fA-F]{6}"));
        assert!(FORM_HTML.contains("A title is required."));
    }

    #[test]
    fn the_form_label_is_reserved_and_not_geometry_persisted() {
        assert!(crate::windows::plan::reserved_labels().contains(FORM_LABEL));
    }
}
