//! [`DownloadList`]: at most [`CAP`] rows, newest first. A row is *unseen* from the moment it is
//! requested until [`DownloadList::mark_seen`]; finishing after that makes it unseen again.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// The most rows the list keeps.
const CAP: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Downloading,
    Done,
    Failed,
}

#[derive(Debug)]
struct Entry {
    id: u64,
    webview: String,
    url: String,
    path: PathBuf,
    status: Status,
    unseen: bool,
}

/// What the downloads button shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadsState {
    /// A download is in progress.
    pub active: bool,
    /// Some row has not been seen.
    pub unseen: bool,
    /// Some unseen row failed.
    pub unseen_failed: bool,
}

/// A row's status as the popup shows it. `Missing` is a finished download whose file is gone.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RowStatus {
    Downloading,
    Done,
    Failed,
    Missing,
}

/// One row of the popup.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
pub struct DownloadRow {
    pub id: u64,
    /// The saved file's name.
    pub name: String,
    pub path: String,
    pub status: RowStatus,
    /// The file's size, when it exists.
    pub size: Option<u64>,
}

/// The app-wide download list.
#[derive(Debug, Default)]
pub struct DownloadList {
    /// Newest first.
    entries: VecDeque<Entry>,
    next_id: u64,
}

impl DownloadList {
    pub fn new() -> Self {
        Self::default()
    }

    /// A download began; returns its row id. Past [`CAP`] rows the oldest finished row goes, and
    /// only when none is finished the oldest in-progress one.
    pub fn requested(&mut self, webview: &str, url: &str, path: PathBuf) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push_front(Entry {
            id,
            webview: webview.to_string(),
            url: url.to_string(),
            path,
            status: Status::Downloading,
            unseen: true,
        });
        if self.entries.len() > CAP {
            let drop = self
                .entries
                .iter()
                .rposition(|e| e.status != Status::Downloading)
                .unwrap_or(self.entries.len() - 1);
            self.entries.remove(drop);
        }
        id
    }

    /// The oldest in-progress download of `webview` for `url` finished; no such row is a no-op.
    pub fn finished(&mut self, webview: &str, url: &str, success: bool) {
        if let Some(e) = self
            .entries
            .iter_mut()
            .rev()
            .find(|e| e.status == Status::Downloading && e.webview == webview && e.url == url)
        {
            e.status = if success {
                Status::Done
            } else {
                Status::Failed
            };
            e.unseen = true;
        }
    }

    /// `webview` closed: its in-progress downloads can no longer finish.
    pub fn webview_gone(&mut self, webview: &str) {
        for e in self
            .entries
            .iter_mut()
            .filter(|e| e.status == Status::Downloading && e.webview == webview)
        {
            e.status = Status::Failed;
            e.unseen = true;
        }
    }

    /// Remove every finished and failed row, keeping in-progress ones.
    pub fn clear(&mut self) {
        self.entries.retain(|e| e.status == Status::Downloading);
    }

    pub fn mark_seen(&mut self) {
        for e in &mut self.entries {
            e.unseen = false;
        }
    }

    pub fn state(&self) -> DownloadsState {
        DownloadsState {
            active: self.entries.iter().any(|e| e.status == Status::Downloading),
            unseen: self.entries.iter().any(|e| e.unseen),
            unseen_failed: self
                .entries
                .iter()
                .any(|e| e.unseen && e.status == Status::Failed),
        }
    }

    /// The rows, newest first. `exists` returns a file's size when it is on disk.
    pub fn rows(&self, exists: impl Fn(&Path) -> Option<u64>) -> Vec<DownloadRow> {
        self.entries
            .iter()
            .map(|e| {
                let found = exists(&e.path);
                let (status, size) = match e.status {
                    Status::Downloading => (RowStatus::Downloading, None),
                    Status::Failed => (RowStatus::Failed, None),
                    Status::Done => match found {
                        Some(size) => (RowStatus::Done, Some(size)),
                        None => (RowStatus::Missing, None),
                    },
                };
                DownloadRow {
                    id: e.id,
                    name: e
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path: e.path.to_string_lossy().into_owned(),
                    status,
                    size,
                }
            })
            .collect()
    }

    pub fn path_of(&self, id: u64) -> Option<PathBuf> {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(format!("/dl/{name}"))
    }

    fn statuses(l: &DownloadList) -> Vec<RowStatus> {
        l.rows(|_| Some(1)).iter().map(|r| r.status).collect()
    }

    #[test]
    fn requested_row_is_downloading_and_unseen() {
        let mut l = DownloadList::new();
        l.requested("w", "u", p("a.zip"));
        assert_eq!(statuses(&l), [RowStatus::Downloading]);
        let s = l.state();
        assert!(s.active && s.unseen && !s.unseen_failed);
    }

    #[test]
    fn finished_completes_oldest_matching_row() {
        let mut l = DownloadList::new();
        l.requested("w", "u", p("a"));
        l.requested("w", "u", p("b"));
        l.finished("w", "u", true);
        // Newest first: b still downloading, a done.
        assert_eq!(statuses(&l), [RowStatus::Downloading, RowStatus::Done]);
        l.finished("other", "u", true);
        assert_eq!(statuses(&l), [RowStatus::Downloading, RowStatus::Done]);
    }

    #[test]
    fn finished_failure_sets_unseen_failed() {
        let mut l = DownloadList::new();
        l.requested("w", "u", p("a"));
        l.finished("w", "u", false);
        assert_eq!(statuses(&l), [RowStatus::Failed]);
        let s = l.state();
        assert!(!s.active && s.unseen && s.unseen_failed);
    }

    #[test]
    fn webview_gone_fails_its_in_progress_rows_only() {
        let mut l = DownloadList::new();
        l.requested("w1", "u", p("a"));
        l.requested("w2", "u", p("b"));
        l.requested("w1", "v", p("c"));
        l.finished("w1", "v", true);
        l.webview_gone("w1");
        // Newest first: c done, b (w2) downloading, a failed.
        assert_eq!(
            statuses(&l),
            [RowStatus::Done, RowStatus::Downloading, RowStatus::Failed]
        );
    }

    #[test]
    fn clear_keeps_in_progress() {
        let mut l = DownloadList::new();
        l.requested("w", "u", p("a"));
        l.requested("w", "v", p("b"));
        l.requested("w", "x", p("c"));
        l.finished("w", "u", true);
        l.finished("w", "v", false);
        l.clear();
        let rows = l.rows(|_| Some(1));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "c");
    }

    #[test]
    fn cap_is_20_and_drops_oldest_finished_first() {
        let mut l = DownloadList::new();
        // Oldest row stays in progress; the next-oldest finishes.
        l.requested("w", "keep", p("keep"));
        l.requested("w", "fin", p("fin"));
        l.finished("w", "fin", true);
        for i in 0..18 {
            l.requested("w", "x", p(&format!("n{i}")));
        }
        assert_eq!(l.rows(|_| Some(1)).len(), 20);
        l.requested("w", "x", p("last"));
        let names: Vec<_> = l.rows(|_| Some(1)).into_iter().map(|r| r.name).collect();
        assert_eq!(names.len(), 20);
        assert!(names.contains(&"keep".to_string()));
        assert!(!names.contains(&"fin".to_string()));
        assert_eq!(names[0], "last");
    }

    #[test]
    fn cap_drops_oldest_in_progress_when_none_finished() {
        let mut l = DownloadList::new();
        for i in 0..21 {
            l.requested("w", "x", p(&format!("n{i}")));
        }
        let names: Vec<_> = l.rows(|_| Some(1)).into_iter().map(|r| r.name).collect();
        assert_eq!(names.len(), 20);
        assert!(!names.contains(&"n0".to_string()));
    }

    #[test]
    fn mark_seen_clears_unseen_until_next_finish() {
        let mut l = DownloadList::new();
        l.requested("w", "u", p("a"));
        l.mark_seen();
        assert!(!l.state().unseen);
        l.finished("w", "u", false);
        let s = l.state();
        assert!(s.unseen && s.unseen_failed);
    }

    #[test]
    fn rows_report_missing_when_exists_is_none() {
        let mut l = DownloadList::new();
        l.requested("w", "u", p("a.zip"));
        l.finished("w", "u", true);
        let rows = l.rows(|_| None);
        assert_eq!(rows[0].status, RowStatus::Missing);
        assert_eq!(rows[0].size, None);
        let rows = l.rows(|_| Some(42));
        assert_eq!(rows[0].status, RowStatus::Done);
        assert_eq!(rows[0].size, Some(42));
        assert_eq!(rows[0].name, "a.zip");
    }

    #[test]
    fn rows_are_newest_first() {
        let mut l = DownloadList::new();
        let a = l.requested("w", "u", p("a"));
        let b = l.requested("w", "u", p("b"));
        let ids: Vec<_> = l.rows(|_| None).iter().map(|r| r.id).collect();
        assert_eq!(ids, [b, a]);
        assert_eq!(l.path_of(a), Some(p("a")));
        assert_eq!(l.path_of(99), None);
    }
}
