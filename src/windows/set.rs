//! The runtime window set: an app's declared windows, which are open, and every lifecycle move.
//!
//! [`WindowSet`] holds the full declared list, open **and** closed, so a closed window stays listed
//! (Window menu, home surface) and reopens with its latest title, colour and size. Each call
//! decides under the lock in a pure `plan_*` on [`SetState`], releases it, then performs the
//! resulting [`Effect`]s; the host is never called with the lock held, so a host hook may call
//! back into the set.
//!
//! **Rename is in place.** A Tauri label can't change, so [`WindowSet::rename`] retitles the live
//! window, rewrites the stored title (the reload echo of the app's own config write then
//! reconciles as a kept window, not close + open) and moves the window's geometry to the label it
//! will hold next launch ([`geometry::persist_as`](crate::geometry::persist_as)). Once closed, it
//! reopens on that label.
//!
//! Manage it directly (`app.manage(WindowSet::new(..))`): every window it builds reports its
//! `Destroyed` to the managed `WindowSet`, so apps need not call [`WindowSet::on_destroyed`].

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use tauri::{AppHandle, Manager, WebviewWindow};

use super::plan::{assign, menu_entries, reconcile, Slot, WindowDecl, WindowOp};
use crate::home::{close_home, home_state, show_home, HomeState};
use crate::menu::WindowEntry;

/// The app's per-window content: what a window holds and how it is torn down.
pub trait WindowHost: Send + Sync + 'static {
    /// Build the window for `slot` under `slot.label` (title, colour and first-run size from
    /// `slot.decl`). Geometry restore is the geometry plugin's, not the host's.
    fn build(&self, app: &AppHandle, slot: &Slot) -> tauri::Result<WebviewWindow>;
    /// `label`'s window is closing or closed: drop its content. Called once per open window,
    /// before the set closes it (delete, a reload removing it) or after it was destroyed.
    fn closed(&self, app: &AppHandle, label: &str);
    /// `label`'s declared title or colour changed in place (rename, or a reload's colour edit).
    fn retitled(&self, app: &AppHandle, label: &str, decl: &WindowDecl);
}

/// Why [`WindowSet::rename`] refused a title.
#[derive(Debug, Clone, PartialEq)]
pub enum RenameError {
    /// The title is blank.
    Empty,
    /// Another window already has this (trimmed) title.
    Taken(String),
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenameError::Empty => f.write_str("A window needs a title"),
            RenameError::Taken(t) => write!(f, "A window named \u{201c}{t}\u{201d} already exists"),
        }
    }
}

impl std::error::Error for RenameError {}

/// One step a planned change performs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Effect {
    /// `host.build` the slot's window.
    Build(Slot),
    /// Unminimize and focus a live window.
    Focus(String),
    /// `host.closed`, then close the live window.
    Close(String),
    /// Retitle a live window.
    SetTitle { label: String, title: String },
    /// `geometry::persist_as`.
    PersistAs { label: String, key: String },
    /// `host.retitled`.
    Retitled { label: String, decl: WindowDecl },
}

/// The set's state and its pure decisions; no Tauri calls.
#[derive(Debug, Default)]
pub(crate) struct SetState {
    /// Every declared window, config order.
    decls: Vec<WindowDecl>,
    /// Trimmed title → label of each open window.
    live: BTreeMap<String, String>,
    /// The app's popped-out windows; they count as open for the home surface.
    pub(crate) detached: usize,
    /// The last config load's error, cleared by every successful materialize/apply.
    pub(crate) load_error: Option<String>,
}

impl SetState {
    fn slots(&self) -> Vec<Slot> {
        assign(&self.decls, &self.live)
    }

    fn index_of(&self, label: &str) -> Option<(usize, Slot)> {
        self.slots()
            .into_iter()
            .enumerate()
            .find(|(_, s)| s.label == label)
    }

    pub(crate) fn entries(&self) -> Vec<WindowEntry> {
        menu_entries(&self.slots())
    }

    pub(crate) fn label_for_title(&self, title: &str) -> Option<String> {
        self.slots()
            .into_iter()
            .find(|s| s.decl.title.trim() == title.trim())
            .map(|s| s.label)
    }

    /// The label the window at `idx` gets at the next launch (nothing live yet).
    fn launch_label_at(&self, idx: usize) -> String {
        assign(&self.decls, &BTreeMap::new()).swap_remove(idx).label
    }

    pub(crate) fn launch_label(&self, label: &str) -> Option<String> {
        self.index_of(label).map(|(i, _)| self.launch_label_at(i))
    }

    pub(crate) fn has_open(&self) -> bool {
        !self.live.is_empty() || self.detached > 0
    }

    pub(crate) fn home(&self, config_exists: bool, config_path: &str) -> Option<HomeState> {
        home_state(
            self.has_open(),
            config_exists,
            config_path,
            self.load_error.as_deref(),
            &self.entries(),
        )
    }

    fn open(&mut self, slot: Slot) -> Effect {
        self.live
            .insert(slot.decl.title.trim().to_string(), slot.label.clone());
        Effect::Build(Slot { open: true, ..slot })
    }

    pub(crate) fn plan_materialize(&mut self, decls: Vec<WindowDecl>) -> Vec<Effect> {
        self.load_error = None;
        self.decls = decls;
        self.slots()
            .into_iter()
            .filter(|s| !s.open && s.decl.open_on_start)
            .map(|s| self.open(s))
            .collect()
    }

    pub(crate) fn plan_apply(&mut self, decls: Vec<WindowDecl>) -> (Vec<WindowOp>, Vec<Effect>) {
        self.load_error = None;
        let ops = reconcile(&self.decls, &decls, &self.live);
        self.decls = decls;
        let effects = ops
            .iter()
            .cloned()
            .map(|op| match op {
                WindowOp::Open(slot) => self.open(slot),
                WindowOp::Close(label) => {
                    self.live.retain(|_, l| *l != label);
                    Effect::Close(label)
                }
                WindowOp::Update { label, decl } => Effect::Retitled { label, decl },
            })
            .collect();
        (ops, effects)
    }

    pub(crate) fn plan_open_or_focus(&mut self, label: &str) -> Option<Effect> {
        let (_, slot) = self.index_of(label)?;
        Some(if slot.open {
            Effect::Focus(slot.label)
        } else {
            self.open(slot)
        })
    }

    /// Mark `label` closed; whether it was open.
    pub(crate) fn plan_destroyed(&mut self, label: &str) -> bool {
        let before = self.live.len();
        self.live.retain(|_, l| l != label);
        self.live.len() != before
    }

    pub(crate) fn plan_rename(
        &mut self,
        label: &str,
        new_title: &str,
    ) -> Result<Vec<Effect>, RenameError> {
        let title = new_title.trim();
        if title.is_empty() {
            return Err(RenameError::Empty);
        }
        let Some((idx, slot)) = self.index_of(label) else {
            return Ok(Vec::new());
        };
        let old = slot.decl.title.trim();
        if old == title {
            return Ok(Vec::new());
        }
        if self.decls.iter().any(|d| d.title.trim() == title) {
            return Err(RenameError::Taken(title.to_string()));
        }
        self.decls[idx].title = title.to_string();
        let mut effects = Vec::new();
        if slot.open {
            self.live.remove(old);
            self.live.insert(title.to_string(), slot.label.clone());
            effects.push(Effect::SetTitle {
                label: slot.label.clone(),
                title: title.to_string(),
            });
        }
        effects.push(Effect::PersistAs {
            label: slot.label.clone(),
            key: self.launch_label_at(idx),
        });
        effects.push(Effect::Retitled {
            label: slot.label,
            decl: self.decls[idx].clone(),
        });
        Ok(effects)
    }

    pub(crate) fn plan_delete(&mut self, label: &str) -> Vec<Effect> {
        let Some((idx, slot)) = self.index_of(label) else {
            return Vec::new();
        };
        self.decls.remove(idx);
        if slot.open {
            self.live.remove(slot.decl.title.trim());
            vec![Effect::Close(slot.label)]
        } else {
            Vec::new()
        }
    }
}

/// An app's windows at runtime (managed state). Every mutating call ends by showing or closing the
/// home surface to match.
pub struct WindowSet {
    app_name: String,
    config_path: PathBuf,
    host: Box<dyn WindowHost>,
    state: Mutex<SetState>,
}

impl WindowSet {
    /// `app_name` titles the home surface; `config_path` is the file it offers to create when
    /// missing.
    pub fn new(app_name: impl Into<String>, config_path: PathBuf, host: impl WindowHost) -> Self {
        Self {
            app_name: app_name.into(),
            config_path,
            host: Box::new(host),
            state: Mutex::new(SetState::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, SetState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Record `decls` from a successful launch load and open the `open_on_start` ones.
    pub fn materialize(&self, app: &AppHandle, decls: Vec<WindowDecl>) {
        let effects = self.lock().plan_materialize(decls);
        self.perform(app, effects);
    }

    /// Bring the running set in line with a successfully reloaded config; returns the ops
    /// performed (the app applies its own per-window content changes to kept windows).
    pub fn apply(&self, app: &AppHandle, decls: Vec<WindowDecl>) -> Vec<WindowOp> {
        let (ops, effects) = self.lock().plan_apply(decls);
        self.perform(app, effects);
        ops
    }

    /// Focus `label`'s window, or reopen it if it is listed but closed. Unknown labels are ignored.
    pub fn open_or_focus(&self, app: &AppHandle, label: &str) {
        let effects = self.lock().plan_open_or_focus(label);
        self.perform(app, effects.into_iter().collect());
    }

    /// `label`'s window was destroyed: it stays listed, closed. Idempotent.
    pub fn on_destroyed(&self, app: &AppHandle, label: &str) {
        if self.lock().plan_destroyed(label) {
            self.host.closed(app, label);
        }
        self.sync_home(app);
    }

    /// Retitle `label`'s window in place. The app writes the new title to its config itself; the
    /// resulting reload keeps the window.
    pub fn rename(&self, app: &AppHandle, label: &str, new_title: &str) -> Result<(), RenameError> {
        let effects = self.lock().plan_rename(label, new_title)?;
        self.perform(app, effects);
        Ok(())
    }

    /// Close `label`'s window (if open) and drop it from the set. The app removes it from its
    /// config itself.
    pub fn delete(&self, app: &AppHandle, label: &str) {
        let effects = self.lock().plan_delete(label);
        self.perform(app, effects);
    }

    /// Window-menu / home-surface rows, config order.
    pub fn entries(&self) -> Vec<WindowEntry> {
        self.lock().entries()
    }

    /// The label of the declared window titled `title` (trimmed), open or closed.
    pub fn label_for_title(&self, title: &str) -> Option<String> {
        self.lock().label_for_title(title)
    }

    /// The label `label`'s window will hold at the next launch: differs from `label` after an
    /// in-place rename. Key per-window state that must survive a relaunch by this.
    pub fn launch_label(&self, label: &str) -> Option<String> {
        self.lock().launch_label(label)
    }

    /// Whether any window is open, popped-out windows included.
    pub fn has_open(&self) -> bool {
        self.lock().has_open()
    }

    /// How many popped-out windows the app has; they keep the home surface away.
    pub fn set_detached_count(&self, app: &AppHandle, count: usize) {
        self.lock().detached = count;
        self.sync_home(app);
    }

    /// Record a failed config load (`None` clears it) for the home surface's broken state.
    pub fn set_load_error(&self, app: &AppHandle, error: Option<String>) {
        self.lock().load_error = error;
        self.sync_home(app);
    }

    fn perform(&self, app: &AppHandle, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Build(slot) => self.build(app, &slot),
                Effect::Focus(label) => {
                    if let Some(w) = app.get_window(&label) {
                        let _ = w.unminimize();
                        let _ = w.set_focus();
                    }
                }
                Effect::Close(label) => {
                    self.host.closed(app, &label);
                    if let Some(w) = app.get_window(&label) {
                        let _ = w.close();
                    }
                }
                Effect::SetTitle { label, title } => {
                    if let Some(w) = app.get_window(&label) {
                        let _ = w.set_title(&title);
                    }
                }
                Effect::PersistAs { label, key } => crate::geometry::persist_as(app, &label, &key),
                Effect::Retitled { label, decl } => self.host.retitled(app, &label, &decl),
            }
        }
        self.sync_home(app);
    }

    fn build(&self, app: &AppHandle, slot: &Slot) {
        match self.host.build(app, slot) {
            Ok(window) => {
                let app = app.clone();
                let label = slot.label.clone();
                window.on_window_event(move |event| {
                    if matches!(event, tauri::WindowEvent::Destroyed) {
                        if let Some(set) = app.try_state::<WindowSet>() {
                            set.on_destroyed(&app, &label);
                        }
                    }
                });
            }
            Err(e) => {
                eprintln!(
                    "{}: couldn't open window {:?}: {e}",
                    self.app_name, slot.decl.title
                );
                self.lock().plan_destroyed(&slot.label);
            }
        }
    }

    fn sync_home(&self, app: &AppHandle) {
        let path = self.config_path.display().to_string();
        let state = self.lock().home(self.config_path.exists(), &path);
        match state {
            Some(s) => {
                let _ = show_home(app, &s, &self.app_name);
            }
            None => close_home(app),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(title: &str) -> WindowDecl {
        WindowDecl {
            title: title.into(),
            colour: None,
            width: 800.0,
            height: 600.0,
            open_on_start: true,
        }
    }

    fn built(effects: &[Effect]) -> Vec<&str> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Build(s) => Some(s.label.as_str()),
                _ => None,
            })
            .collect()
    }

    fn launched(titles: &[&str]) -> SetState {
        let mut s = SetState::default();
        let fx = s.plan_materialize(titles.iter().map(|t| decl(t)).collect());
        assert_eq!(built(&fx).len(), titles.len());
        s
    }

    #[test]
    fn materialize_opens_only_open_on_start_windows_and_lists_all() {
        let mut later = decl("later");
        later.open_on_start = false;
        let mut s = SetState::default();
        let fx = s.plan_materialize(vec![decl("work"), later]);
        assert_eq!(built(&fx), ["work"]);
        let e = s.entries();
        assert_eq!(e.len(), 2);
        assert!(e[0].open && !e[1].open);
    }

    #[test]
    fn closing_keeps_the_window_listed_as_closed() {
        let mut s = launched(&["work", "side"]);
        assert!(s.plan_destroyed("side"));
        let e = s.entries();
        assert_eq!(e.len(), 2);
        assert_eq!((e[1].id.as_str(), e[1].open), ("side", false));
        assert!(!s.plan_destroyed("side"), "a second Destroyed is a no-op");
    }

    #[test]
    fn open_or_focus_focuses_a_live_window() {
        let mut s = launched(&["work"]);
        assert_eq!(
            s.plan_open_or_focus("work"),
            Some(Effect::Focus("work".into()))
        );
    }

    #[test]
    fn open_or_focus_reopens_a_closed_window_on_a_fresh_label() {
        // Renamed while open: the live label stays `work`. Once closed, the window reopens on the
        // label its current title gives it, the one it will also hold next launch.
        let mut s = launched(&["work"]);
        s.plan_rename("work", "job").unwrap();
        s.plan_destroyed("work");
        assert_eq!(s.entries()[0].id, "job");
        match s.plan_open_or_focus("job") {
            Some(Effect::Build(slot)) => {
                assert_eq!(slot.label, "job");
                assert_eq!(slot.decl.title, "job");
                assert!(slot.open);
            }
            other => panic!("expected Build, got {other:?}"),
        }
        assert!(s.has_open());
        assert_eq!(s.plan_open_or_focus("nope"), None);
    }

    #[test]
    fn rename_to_existing_title_is_refused() {
        let mut s = launched(&["work", "side"]);
        assert_eq!(
            s.plan_rename("work", " side "),
            Err(RenameError::Taken("side".into()))
        );
        assert_eq!(s.plan_rename("work", "  "), Err(RenameError::Empty));
        assert_eq!(s.label_for_title("work").as_deref(), Some("work"));
    }

    #[test]
    fn rename_rekeys_title_lookup() {
        let mut s = launched(&["work"]);
        let fx = s.plan_rename("work", "job").unwrap();
        assert_eq!(s.label_for_title("job").as_deref(), Some("work"));
        assert_eq!(s.label_for_title("work"), None);
        assert!(fx.contains(&Effect::SetTitle {
            label: "work".into(),
            title: "job".into()
        }));
        assert!(fx.contains(&Effect::PersistAs {
            label: "work".into(),
            key: "job".into()
        }));
        // The reload echo of the app's own config write keeps the window.
        let mut job = decl("job");
        job.colour = None;
        let (ops, fx) = s.plan_apply(vec![job]);
        assert!(ops.is_empty() && fx.is_empty(), "{ops:?} {fx:?}");
        assert!(s.entries()[0].open);
    }

    #[test]
    fn rename_persists_under_the_next_launch_label() {
        // "shell home" can never take the home surface's label, so its next-launch label is -2.
        let mut s = launched(&["work"]);
        let fx = s.plan_rename("work", "shell home").unwrap();
        assert!(fx.contains(&Effect::PersistAs {
            label: "work".into(),
            key: "shell-home-2".into()
        }));
    }

    #[test]
    fn rename_of_a_closed_window_moves_its_geometry_without_a_title_call() {
        let mut s = launched(&["work", "side"]);
        s.plan_destroyed("side");
        let fx = s.plan_rename("side", "aside").unwrap();
        assert!(!fx.iter().any(|e| matches!(e, Effect::SetTitle { .. })));
        assert!(fx.contains(&Effect::PersistAs {
            label: "side".into(),
            key: "aside".into()
        }));
        assert_eq!(s.entries()[1].id, "aside");
    }

    #[test]
    fn deleting_last_open_window_shows_home() {
        let mut s = launched(&["work"]);
        let fx = s.plan_delete("work");
        assert_eq!(fx, [Effect::Close("work".into())]);
        assert!(s.entries().is_empty());
        assert!(!s.has_open());
        assert!(matches!(
            s.home(true, "/c.toml"),
            Some(HomeState::Windows { windows }) if windows.is_empty()
        ));
    }

    #[test]
    fn a_detached_window_keeps_home_away() {
        let mut s = launched(&["work"]);
        s.detached = 1;
        s.plan_destroyed("work");
        assert!(s.has_open());
        assert!(s.home(true, "/c.toml").is_none());
    }

    #[test]
    fn a_load_error_shows_broken_until_a_successful_apply() {
        let mut s = SetState {
            load_error: Some("bad".into()),
            ..Default::default()
        };
        assert!(matches!(
            s.home(true, "/c.toml"),
            Some(HomeState::Broken { .. })
        ));
        s.plan_apply(vec![decl("work")]);
        assert!(s.load_error.is_none());
        assert!(s.home(true, "/c.toml").is_none());
    }

    #[test]
    fn apply_performs_reconcile_ops_as_effects() {
        let mut s = launched(&["work", "side"]);
        let mut work = decl("work");
        work.colour = Some("#ff0000".into());
        let (ops, fx) = s.plan_apply(vec![work.clone(), decl("new")]);
        assert_eq!(ops.len(), 3);
        assert!(fx.contains(&Effect::Close("side".into())));
        assert_eq!(built(&fx), ["new"]);
        assert!(fx.contains(&Effect::Retitled {
            label: "work".into(),
            decl: work
        }));
        let e = s.entries();
        assert_eq!(e.len(), 2);
        assert!(e.iter().all(|e| e.open));
    }
}
