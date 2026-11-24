//! Config-file hot-reload watcher.
//!
//! Watches the config file's **parent directory** (atomic-save editors write a temp file + rename,
//! replacing the inode — a single-file watch would silently break) and matches events by **file
//! name** (macOS FSEvents reports canonical `/private/var/...` paths while the caller holds a
//! `/var/...` symlink path, so exact-path equality misses every event). A symlinked config is also
//! watched at its target's directory and name. On each matching change it waits for the file to
//! settle ([`settled`]), reads it and hands the raw source to `on_change`; shell-core never parses (config-agnostic — no
//! `config-core` edge). It delivers a file's contents only when they differ from what it last
//! delivered — or from what `on_change` last wrote (`Some(bytes)`, a format-on-save rewrite) — so
//! one save's burst of events, and the echo of the app's own write, reload exactly once, and a
//! re-save of identical bytes reloads nothing.

use notify::{RecursiveMode, Watcher as _};
use std::path::{Path, PathBuf};

/// Spawn a background thread that watches `path`'s parent directory and calls `on_change(src)` on
/// each change to the file (matched by name), passing the file's current contents. `on_change`
/// returns `Some(bytes)` if it wrote the file itself (format-on-save) so the watcher swallows the
/// echo; `None` otherwise. Fire-and-forget: the thread lives for the process and the returned
/// unit carries no handle.
///
/// The parent directory is created (before this returns) if it doesn't exist yet, so a watcher
/// armed on a fresh install, before any config is written, still sees the file once it appears —
/// the consumers arm it once at setup and never again. It returns once the watch is registered, so
/// a write straight after it is seen. If the watch can't be established the thread exits and the
/// config won't hot-reload.
pub fn watch_config(
    path: PathBuf,
    mut on_change: impl FnMut(&str) -> Option<String> + Send + 'static,
) {
    // A bare relative filename has parent "" (watching which errors) → fall back to ".".
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let _ = std::fs::create_dir_all(&dir);
    // A symlinked config (`config.toml -> ~/dotfiles/app.toml`) is edited at its target, so the
    // events land in the target's directory under the target's name: watch both the link's and
    // the target's (dir, name). The link's pair catches the link itself being replaced, which
    // reloads through the new link; the target pair is fixed here, so edits to a re-pointed link's
    // new target don't hot-reload until the next launch (unlock: re-canonicalize on a link event
    // and watch the new target's dir).
    let mut watched = vec![(dir, path.file_name().map(|n| n.to_owned()))];
    if path.is_symlink() {
        if let Ok(target) = std::fs::canonicalize(&path) {
            if let Some(target_dir) = target.parent() {
                let pair = (
                    target_dir.to_path_buf(),
                    target.file_name().map(|n| n.to_owned()),
                );
                if !watched.contains(&pair) {
                    watched.push(pair);
                }
            }
        }
    }
    // Dropped (without a send) when the thread exits early, which ends the wait below just the same.
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(tx) else {
            return;
        };
        let dirs: std::collections::BTreeSet<_> = watched.iter().map(|(d, _)| d).collect();
        for dir in dirs {
            if watcher.watch(dir, RecursiveMode::NonRecursive).is_err() {
                return;
            }
        }
        let _ = ready_tx.send(());
        let names: Vec<_> = watched.iter().filter_map(|(_, n)| n.clone()).collect();
        // The bytes the app now holds: the last delivered source, or on_change's own write of it.
        // A read that matches is a repeat event (FSEvents and atomic renames deliver several per
        // save) or that write's echo, and reloading identical bytes is a no-op for every consumer.
        let mut current: Option<String> = None;
        for res in rx {
            let Ok(event) = res else { continue };
            if !event
                .paths
                .iter()
                .any(|p| p.file_name().is_some_and(|n| names.iter().any(|w| w == n)))
            {
                continue;
            }
            settled(&path);
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            if current.as_deref() == Some(src.as_str()) {
                continue;
            }
            current = Some(on_change(&src).unwrap_or(src));
        }
    });
    let _ = ready_rx.recv();
}

/// The gap between the two reads that must agree before a changed config is delivered.
pub const SETTLE: std::time::Duration = std::time::Duration::from_millis(100);

/// Blocks until two reads of `path`, [`SETTLE`] apart, return the same bytes (a missing file
/// counts as its own state), giving up after a second so a file rewritten continuously still
/// loads. An editor saving in place truncates, then writes: a read between the two sees an empty
/// or cut-short file that may still parse.
fn settled(path: &Path) {
    let mut last = std::fs::read(path).ok();
    for _ in 0..10 {
        std::thread::sleep(SETTLE);
        let now = std::fs::read(path).ok();
        if now == last {
            return;
        }
        last = now;
    }
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
        // Proves the watcher fires on a change to the watched file — the file-name match an
        // exact-path match misses entirely under a symlinked config dir on macOS.
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write(&path, "first");

        let (tx, rx) = mpsc::channel();
        watch_config(path.clone(), move |src| {
            let _ = tx.send(src.to_string());
            None
        });

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

    #[test]
    fn delivers_each_distinct_content_once() {
        // Several writes of the same bytes (a save's event burst, a re-save) reload once; a real
        // change after them still reloads.
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write(&path, "first");

        let (tx, rx) = mpsc::channel();
        watch_config(path.clone(), move |src| {
            let _ = tx.send(src.to_string());
            None
        });

        for _ in 0..3 {
            write(&path, "same");
            std::thread::sleep(Duration::from_millis(100));
        }
        write(&path, "changed");

        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !got.iter().any(|v: &String| v == "changed") {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(v) => got.push(v),
                Err(_) => panic!("timed out; got {got:?}"),
            }
        }
        assert_eq!(
            got.iter().filter(|v| *v == "same").count(),
            1,
            "got {got:?}"
        );
    }

    #[test]
    fn an_in_place_save_is_delivered_only_once_it_settles() {
        // An editor saving in place truncates, then writes: a read between the two sees an empty
        // file, which an app may parse as "no windows" and close every one.
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write(&path, "first");

        let (tx, rx) = mpsc::channel();
        watch_config(path.clone(), move |src| {
            let _ = tx.send(src.to_string());
            None
        });

        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        f.sync_all().unwrap();
        std::thread::sleep(SETTLE / 3);
        f.write_all(b"second").unwrap();
        f.sync_all().unwrap();
        drop(f);

        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !seen.iter().any(|v: &String| v == "second") {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(v) => seen.push(v),
                Err(_) => panic!("timed out; got {seen:?}"),
            }
        }
        assert!(
            !seen.iter().any(String::is_empty),
            "a delivery saw the half-written file: {seen:?}"
        );
    }

    #[test]
    fn fires_when_a_symlinked_config_is_edited_at_its_target() {
        let link_dir = tempdir().unwrap();
        let target_dir = tempdir().unwrap();
        let target = target_dir.path().join("app.toml");
        write(&target, "first");
        let link = link_dir.path().join("config.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let (tx, rx) = mpsc::channel();
        watch_config(link, move |src| {
            let _ = tx.send(src.to_string());
            None
        });

        write(&target, "second");

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(v) if v == "second" => break,
                Ok(_) => continue,
                Err(_) => panic!("timed out waiting for a reload of the symlink target"),
            }
        }
    }

    #[test]
    fn fires_when_the_config_dir_did_not_exist_yet() {
        // A fresh install: no config dir yet, so there is nothing to watch until watch_config
        // creates it. Writing the file afterwards (the home surface's "Create a starter config")
        // must still reach on_change.
        let root = tempdir().unwrap();
        let path = root.path().join("app").join("config.toml");

        let (tx, rx) = mpsc::channel();
        watch_config(path.clone(), move |src| {
            let _ = tx.send(src.to_string());
            None
        });
        assert!(
            path.parent().unwrap().is_dir(),
            "watch_config must create the parent dir"
        );

        write(&path, "starter");

        let v = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("timed out waiting for the starter config");
        assert_eq!(v, "starter");
    }
}
