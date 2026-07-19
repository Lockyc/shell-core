//! Config-file hot-reload watcher shared by curator and lector.
//!
//! Watches the config file's **parent directory** (atomic-save editors write a temp file + rename,
//! replacing the inode — a single-file watch would silently break) and matches events by **file
//! name** (macOS FSEvents reports canonical `/private/var/...` paths while the caller holds a
//! `/var/...` symlink path, so exact-path equality misses every event — the latent bug curator and
//! lector both shipped, fixed here for both by construction). On each matching change it reads the
//! file and hands the raw source to `on_change`; shell-core never parses (config-agnostic — no
//! `config-core` edge). `on_change` returns `Some(bytes)` when it wrote the file itself (a
//! format-on-save rewrite), and the watcher swallows the echo event those bytes trigger so a user
//! save reloads exactly once.
//!
//! warden is **not** a consumer: its own `Watcher` parses inside, drives a slow root-scan +
//! main-thread reconcile, and relies on `format_file`'s diff-guard rather than an echo-swallow
//! marker — a genuinely different contract, already correct (it is the file-name-match reference
//! this shared watcher was modelled on), so it keeps its own implementation.

use notify::{RecursiveMode, Watcher as _};
use std::path::{Path, PathBuf};

/// Spawn a background thread that watches `path`'s parent directory and calls `on_change(src)` on
/// each change to the file (matched by name), passing the file's current contents. `on_change`
/// returns `Some(bytes)` if it wrote the file itself (format-on-save) so the watcher swallows the
/// echo; `None` otherwise. Fire-and-forget: the thread lives for the process (the returned unit
/// carries no handle, matching the apps' prior inline watchers). If the watch can't be established
/// the thread simply exits and the config won't hot-reload — the same graceful degradation the
/// apps had before.
pub fn watch_config(
    path: PathBuf,
    mut on_change: impl FnMut(&str) -> Option<String> + Send + 'static,
) {
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(tx) else {
            return;
        };
        // A bare relative filename has parent "" (watching which errors) → fall back to ".".
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if watcher.watch(dir, RecursiveMode::NonRecursive).is_err() {
            return;
        }
        let want_name = path.file_name().map(|n| n.to_owned());
        // The exact bytes of on_change's most recent self-write, so its echo is swallowed and a
        // user save reloads exactly once. `take()` clears the marker either way, so a missed echo
        // costs at worst one redundant no-op reload.
        let mut self_write: Option<String> = None;
        for res in rx {
            let Ok(event) = res else { continue };
            if !event
                .paths
                .iter()
                .any(|p| p.file_name() == want_name.as_deref())
            {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            if self_write.take().as_deref() == Some(src.as_str()) {
                continue;
            }
            self_write = on_change(&src);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    fn write(path: &Path, body: &str) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        f.sync_all().unwrap();
    }

    #[test]
    fn fires_on_save_matching_by_file_name() {
        // Proves the shared watcher fires on a change to the watched file — the file-name match
        // that fixes curator's + lector's exact-path bug (they'd miss events entirely under a
        // symlinked config dir on macOS).
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write(&path, "first");

        let (tx, rx) = mpsc::channel();
        watch_config(path.clone(), move |src| {
            let _ = tx.send(src.to_string());
            None
        });

        std::thread::sleep(Duration::from_millis(200));
        write(&path, "second");

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(v) if v == "second" => break,
                Ok(_) => continue, // stale early event (e.g. the initial create) — keep draining
                Err(_) => panic!("timed out waiting for the 'second' change"),
            }
        }
    }
}
