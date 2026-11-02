//! The app-wide download list: every content webview's downloads in one newest-first list, the
//! model behind the downloads popup. [`list`] is pure (no Tauri), so the list's rules are unit
//! tested without a webview.

pub mod list;

pub use list::{DownloadList, DownloadRow, DownloadsState, RowStatus};
