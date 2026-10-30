//! The window form flow: Window ▸ New Window… and the banner's Edit Window… / Delete Window…
//! menu, through [`open_form`] to the app's config write and the set's own change.
//!
//! A form result plans its [`FlowStep`]s under the set's lock ([`SetState::plan_form`]) and runs
//! them outside it, stopping at the first failure, which reaches the app through
//! [`WindowHost::edit_failed`](super::WindowHost::edit_failed):
//!
//! - Create → [`WindowWrite::Add`]; the app's reload opens the window.
//! - Update → refuse a title another window holds → [`WindowWrite::Set`] → [`WindowSet::rename`]
//!   when the title changed (in place, so the reload echo keeps the window).
//! - Delete → [`WindowWrite::Remove`] → [`WindowSet::delete`].

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::{AppHandle, LogicalPosition, Manager};

use super::form::{open_form, FormMode, FormResult};
use super::set::{RenameError, SetState, WindowSet, WindowWrite};
use crate::menu::ids;

/// One step of a form result, run in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FlowStep {
    /// [`WindowHost::write`](super::WindowHost::write).
    Write(WindowWrite),
    /// [`WindowSet::rename`].
    Rename { label: String, title: String },
    /// [`WindowSet::delete`].
    Delete(String),
}

/// Run `steps` through `exec`, stopping at the first error.
pub(crate) fn run_steps(
    steps: &[FlowStep],
    exec: impl FnMut(&FlowStep) -> Result<(), String>,
) -> Result<(), String> {
    steps.iter().try_for_each(exec)
}

/// A menu item the flow owns.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuRequest {
    /// Window ▸ New Window….
    New,
    /// Banner ▸ Edit Window… for this window label.
    Edit(String),
    /// Banner ▸ Delete Window… for this window label.
    Delete(String),
}

impl MenuRequest {
    pub fn id(&self) -> String {
        match self {
            MenuRequest::New => ids::NEW_WINDOW.to_string(),
            MenuRequest::Edit(l) => format!("{}{l}", ids::EDIT_WINDOW_PREFIX),
            MenuRequest::Delete(l) => format!("{}{l}", ids::DELETE_WINDOW_PREFIX),
        }
    }
}

/// The flow's request behind a menu id, or `None` for any id it does not own.
pub fn menu_request(id: &str) -> Option<MenuRequest> {
    if id == ids::NEW_WINDOW {
        return Some(MenuRequest::New);
    }
    id.strip_prefix(ids::EDIT_WINDOW_PREFIX)
        .map(|l| MenuRequest::Edit(l.to_string()))
        .or_else(|| {
            id.strip_prefix(ids::DELETE_WINDOW_PREFIX)
                .map(|l| MenuRequest::Delete(l.to_string()))
        })
}

impl SetState {
    /// The steps `result` takes for the form opened on `target` (`None` for New Window). A
    /// target gone since the form opened takes none; a refused title is the `Err` message.
    pub(crate) fn plan_form(
        &self,
        target: Option<&str>,
        result: FormResult,
    ) -> Result<Vec<FlowStep>, String> {
        let current = target
            .and_then(|l| self.index_of(l))
            .map(|(_, s)| (s.label, s.decl));
        match (result, current) {
            (FormResult::Create { title, colour }, _) => {
                Ok(vec![FlowStep::Write(WindowWrite::Add {
                    title: title.trim().to_string(),
                    colour,
                })])
            }
            (FormResult::Update { title, colour }, Some((label, decl))) => {
                let (old, new) = (decl.title.trim(), title.trim());
                if new.is_empty() {
                    return Err(RenameError::Empty.to_string());
                }
                let retitle = old != new;
                if retitle && self.label_for_title(new).is_some_and(|l| l != label) {
                    return Err(RenameError::Taken(new.to_string()).to_string());
                }
                let colour = (decl.colour != colour).then_some(colour);
                if !retitle && colour.is_none() {
                    return Ok(Vec::new());
                }
                let mut steps = vec![FlowStep::Write(WindowWrite::Set {
                    title: old.to_string(),
                    new_title: retitle.then(|| new.to_string()),
                    colour,
                })];
                if retitle {
                    steps.push(FlowStep::Rename {
                        label,
                        title: new.to_string(),
                    });
                }
                Ok(steps)
            }
            (FormResult::Delete, Some((label, decl))) => Ok(vec![
                FlowStep::Write(WindowWrite::Remove {
                    title: decl.title.trim().to_string(),
                }),
                FlowStep::Delete(label),
            ]),
            _ => Ok(Vec::new()),
        }
    }
}

impl WindowSet {
    /// Run a menu item the flow owns ([`menu_request`]): New Window…, or a banner menu's Edit
    /// Window… / Delete Window…. Returns whether `id` was one.
    pub fn handle_menu(&self, app: &AppHandle, id: &str) -> bool {
        let Some(request) = menu_request(id) else {
            return false;
        };
        match request {
            MenuRequest::New => self.new_window(app),
            MenuRequest::Edit(label) => {
                if let Some(e) = self.entries().into_iter().find(|e| e.id == label) {
                    let mode = FormMode::Edit {
                        title: e.title,
                        colour: e.colour,
                    };
                    self.form(app, Some(label), mode);
                }
            }
            MenuRequest::Delete(label) => {
                if let Some(e) = self.entries().into_iter().find(|e| e.id == label) {
                    self.form(app, Some(label), FormMode::ConfirmDelete { title: e.title });
                }
            }
        }
        true
    }

    /// Window ▸ New Window…: the empty form; on Create the app writes the window and its reload
    /// opens it.
    pub fn new_window(&self, app: &AppHandle) {
        self.form(app, None, FormMode::New);
    }

    /// Pop the banner's Edit Window… / Delete Window… menu over `label`'s window at the chrome's
    /// `(x, y)` (chrome-core's `onBannerMenu`). Its items come back as menu events for
    /// [`handle_menu`](Self::handle_menu).
    pub fn banner_menu(&self, app: &AppHandle, label: &str, x: f64, y: f64) -> tauri::Result<()> {
        let window = app.get_window(label).ok_or(tauri::Error::WindowNotFound)?;
        let edit = MenuItemBuilder::with_id(MenuRequest::Edit(label.into()).id(), "Edit Window…")
            .build(app)?;
        let delete =
            MenuItemBuilder::with_id(MenuRequest::Delete(label.into()).id(), "Delete Window…")
                .build(app)?;
        let menu = MenuBuilder::new(app).items(&[&edit, &delete]).build()?;
        window.popup_menu_at(&menu, LogicalPosition::new(x, y))
    }

    /// Open the form on `target`; the titles it refuses are every other window's.
    fn form(&self, app: &AppHandle, target: Option<String>, mode: FormMode) {
        let taken = self
            .entries()
            .into_iter()
            .filter(|e| Some(&e.id) != target.as_ref())
            .map(|e| e.title)
            .collect();
        let app2 = app.clone();
        open_form(app, mode, taken, move |result| {
            if let Some(set) = app2.try_state::<WindowSet>() {
                set.finish_form(&app2, target.as_deref(), result);
            }
        });
    }

    fn finish_form(&self, app: &AppHandle, target: Option<&str>, result: FormResult) {
        let planned = self.lock().plan_form(target, result);
        let steps = match planned {
            Ok(steps) if steps.is_empty() => return,
            Ok(steps) => steps,
            Err(e) => return self.host.edit_failed(app, &e),
        };
        let done = run_steps(&steps, |step| match step {
            FlowStep::Write(w) => self.host.write(app, w),
            FlowStep::Rename { label, title } => {
                self.rename(app, label, title).map_err(|e| e.to_string())
            }
            FlowStep::Delete(label) => {
                self.delete(app, label);
                Ok(())
            }
        });
        match done {
            Ok(()) => self.host.changed(app),
            Err(e) => self.host.edit_failed(app, &e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::form::FormResult;
    use crate::windows::plan::WindowDecl;
    use crate::windows::set::SetState;

    fn decl(title: &str, colour: Option<&str>) -> WindowDecl {
        WindowDecl {
            title: title.into(),
            colour: colour.map(str::to_string),
            width: 800.0,
            height: 600.0,
            open_on_start: true,
        }
    }

    fn launched() -> SetState {
        let mut s = SetState::default();
        s.plan_materialize(vec![decl("Work", Some("#abc")), decl("Side", None)]);
        s
    }

    fn update(title: &str, colour: Option<&str>) -> FormResult {
        FormResult::Update {
            title: title.into(),
            colour: colour.map(str::to_string),
        }
    }

    #[test]
    fn form_results_map_to_writes_in_order() {
        let s = launched();
        let create = FormResult::Create {
            title: " New ".into(),
            colour: Some("#f00".into()),
        };
        assert_eq!(
            s.plan_form(None, create).unwrap(),
            [FlowStep::Write(WindowWrite::Add {
                title: "New".into(),
                colour: Some("#f00".into())
            })]
        );
        // New title: Set (addressed by the current title) then rename.
        assert_eq!(
            s.plan_form(Some("Work"), update("Job", Some("#abc")))
                .unwrap(),
            [
                FlowStep::Write(WindowWrite::Set {
                    title: "Work".into(),
                    new_title: Some("Job".into()),
                    colour: None,
                }),
                FlowStep::Rename {
                    label: "Work".into(),
                    title: "Job".into()
                },
            ]
        );
        // Colour only: Set, no rename. Clearing a colour is Some(None).
        assert_eq!(
            s.plan_form(Some("Work"), update(" Work ", None)).unwrap(),
            [FlowStep::Write(WindowWrite::Set {
                title: "Work".into(),
                new_title: None,
                colour: Some(None),
            })]
        );
        // Nothing changed: nothing to write.
        assert!(s
            .plan_form(Some("Work"), update("Work", Some("#abc")))
            .unwrap()
            .is_empty());
        assert_eq!(
            s.plan_form(Some("Side"), FormResult::Delete).unwrap(),
            [
                FlowStep::Write(WindowWrite::Remove {
                    title: "Side".into()
                }),
                FlowStep::Delete("Side".into()),
            ]
        );
        assert!(s
            .plan_form(Some("Work"), FormResult::Cancel)
            .unwrap()
            .is_empty());
        // A window gone since the form opened: nothing to do.
        assert!(s
            .plan_form(Some("Gone"), FormResult::Delete)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_taken_title_is_refused_before_any_write() {
        let s = launched();
        assert_eq!(
            s.plan_form(Some("Work"), update(" Side ", None)),
            Err("A window named \u{201c}Side\u{201d} already exists".to_string())
        );
    }

    #[test]
    fn a_failed_write_stops_the_sequence() {
        let steps = launched()
            .plan_form(Some("Work"), update("Job", None))
            .unwrap();
        let mut ran = Vec::new();
        let r = run_steps(&steps, |step| {
            ran.push(step.clone());
            match step {
                FlowStep::Write(_) => Err("disk full".to_string()),
                _ => Ok(()),
            }
        });
        assert_eq!(r, Err("disk full".to_string()));
        assert_eq!(
            ran,
            steps[..1],
            "the rename after a failed write never runs"
        );
        let mut n = 0;
        assert_eq!(
            run_steps(&steps, |_| {
                n += 1;
                Ok(())
            }),
            Ok(())
        );
        assert_eq!(n, 2);
    }

    #[test]
    fn menu_ids_round_trip_and_unknown_ids_are_not_consumed() {
        for req in [
            MenuRequest::Edit("a:b".into()),
            MenuRequest::Delete("Work-2".into()),
        ] {
            assert_eq!(menu_request(&req.id()), Some(req));
        }
        assert_eq!(
            menu_request(crate::menu::ids::NEW_WINDOW),
            Some(MenuRequest::New)
        );
        for id in [
            crate::menu::ids::CLOSE_WINDOW,
            "shell:open_window:Work",
            "curator:edit_window:Work",
            "reload_active",
        ] {
            assert_eq!(menu_request(id), None, "{id}");
        }
    }
}
