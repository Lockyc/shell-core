//! The app-wide download list: every content webview's downloads in one newest-first list, the
//! model behind the downloads popup. [`list`] is pure (no Tauri), so the list's rules are unit
//! tested without a webview.
//!
//! The one list lives here, behind a process-wide lock; [`with_list`] is the only way to change it
//! and reports the resulting [`DownloadsState`] to the app's [`init`] hook after the lock is
//! released, so the hook may call back into [`state`].

use std::sync::{Arc, Mutex, OnceLock};

use tauri::AppHandle;

pub mod list;

pub use list::{DownloadList, DownloadRow, DownloadsState, RowStatus};

type OnChange = Arc<dyn Fn(DownloadsState) + Send + Sync>;

static LIST: Mutex<Option<DownloadList>> = Mutex::new(None);
static ON_CHANGE: OnceLock<Mutex<Option<OnChange>>> = OnceLock::new();

fn on_change_slot() -> &'static Mutex<Option<OnChange>> {
    ON_CHANGE.get_or_init(|| Mutex::new(None))
}

/// Installs the app's state hook: `on_change` runs after every list change with the new state.
/// Without it the list is still kept and nothing is reported.
pub fn init(
    app: &AppHandle,
    on_change: impl Fn(&AppHandle, DownloadsState) + Send + Sync + 'static,
) {
    let app = app.clone();
    *on_change_slot().lock().unwrap() = Some(Arc::new(move |s| on_change(&app, s)));
}

/// What the downloads button shows now.
pub fn state() -> DownloadsState {
    with_list_quiet(|l| l.state())
}

fn with_list_quiet<T>(f: impl FnOnce(&mut DownloadList) -> T) -> T {
    let mut guard = LIST.lock().unwrap();
    f(guard.get_or_insert_with(DownloadList::new))
}

/// Mutates the list, then reports the new state to the [`init`] hook.
pub(crate) fn with_list<T>(f: impl FnOnce(&mut DownloadList) -> T) -> T {
    let (out, state) = with_list_quiet(|l| {
        let out = f(l);
        (out, l.state())
    });
    // The popup's row push goes here, after the lock is released and alongside the state hook.
    let hook = on_change_slot().lock().unwrap().clone();
    if let Some(hook) = hook {
        hook(state);
    }
    out
}

#[cfg(test)]
fn set_on_change_for_test(f: Box<dyn Fn(DownloadsState) + Send + Sync>) {
    *on_change_slot().lock().unwrap() = Some(Arc::from(f));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn with_list_fires_on_change_with_new_state() {
        let seen: Arc<Mutex<Vec<DownloadsState>>> = Arc::default();
        let sink = seen.clone();
        set_on_change_for_test(Box::new(move |s| sink.lock().unwrap().push(s)));

        with_list(|l| l.requested("tab", "https://x/a.pdf", "/tmp/a.pdf".into()));

        let got = seen.lock().unwrap().clone();
        assert_eq!(got.len(), 1);
        assert!(got[0].active && got[0].unseen);
        assert_eq!(state(), got[0]);
    }
}
