//! The window form: one small shell-core-owned window for New Window (empty), Edit Window
//! (prefilled title + colour) and Delete Window (confirm). It reports a [`FormResult`]; the app
//! does the config write itself (shell-core never depends on config-core).
//!
//! The page is served over its own scheme (same reason as [`crate::home`]'s) and talks back with
//! a Tauri event, [`RESULT_EVENT`], so it needs no app command. The page validates the title
//! (non-empty, not in `taken_titles`, trimmed) and the colour (`#rgb`/`#rrggbb`, config-core's
//! `Colour::parse` rule, reimplemented in the page) before enabling Save.

use serde::Deserialize;

/// The form window's label. Excluded from geometry persistence (`geometry::is_excluded`) and
/// reserved against window labels (`windows::plan::reserved_labels`).
pub const FORM_LABEL: &str = "shell-window-form";

const FORM_SCHEME: &str = "shell-window-form";
const FORM_HTML: &str = include_str!("form.html");

/// Page -> Rust: the user's choice, a [`FormResult`] JSON object.
const RESULT_EVENT: &str = "shell:window-form-result";
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

pub(crate) fn register_protocol<R: tauri::Runtime>(
    builder: tauri::Builder<R>,
) -> tauri::Builder<R> {
    crate::home::serve_static(builder, FORM_SCHEME, FORM_HTML)
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
    use tauri::{Emitter, Listener, Manager};

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

    let id = app.listen(RESULT_EVENT, |ev| {
        if let Ok(result) = serde_json::from_str::<FormResult>(ev.payload()) {
            finish(result);
            // The form is done once it has reported; closing here cannot re-enter `finish`
            // (the callback is already taken).
        }
    });
    let handle = app.clone();
    window.on_window_event(move |e| {
        if matches!(e, tauri::WindowEvent::Destroyed) {
            handle.unlisten(id);
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
        assert!(FORM_HTML.contains(RESULT_EVENT));
        assert!(FORM_HTML.contains(REFRESH_EVENT));
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
