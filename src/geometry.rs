//! Per-window geometry persistence — size and position, in AppKit points.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{
    plugin::TauriPlugin, AppHandle, LogicalPosition, LogicalSize, Manager, Monitor, RunEvent,
    Runtime, Window, WindowEvent,
};

/// On-disk schema version. Bump when the shape of a persisted entry changes; an unrecognised
/// version is discarded rather than migrated, so a format change costs one reset, not a
/// migration path carried forever.
const SCHEMA_VERSION: u32 = 1;

/// Floor for a restored dimension, so a corrupt or degenerate stored size can't produce an
/// effectively invisible window.
const MIN_DIM: f64 = 200.0;

/// A window rectangle in **AppKit points**, top-left origin.
///
/// Points — not physical pixels — are the only coherent cross-monitor unit on macOS: physical is
/// points × the scale factor of whichever screen the window occupies, so a value saved on a 2x
/// display and applied on a 1x one is out by the ratio. Points make that failure unrepresentable.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn right(&self) -> f64 {
        self.x + self.width
    }

    fn bottom(&self) -> f64 {
        self.y + self.height
    }

    /// Area of the intersection with `other`, or 0.0 when they don't overlap.
    fn overlap_area(&self, other: &Rect) -> f64 {
        let w = (self.right().min(other.right()) - self.x.max(other.x)).max(0.0);
        let h = (self.bottom().min(other.bottom()) - self.y.max(other.y)).max(0.0);
        w * h
    }
}

#[derive(Deserialize)]
struct Store {
    version: u32,
    #[serde(default)]
    windows: HashMap<String, Rect>,
}

#[derive(Serialize)]
struct StoreRef<'a> {
    version: u32,
    windows: &'a HashMap<String, Rect>,
}

/// Parse a store file. Anything unreadable — bad JSON, a version this build doesn't know — is an
/// empty store, never an error and never a panic: a corrupt file must cost the user their saved
/// bounds, not their app launch.
fn decode(bytes: &[u8]) -> HashMap<String, Rect> {
    match serde_json::from_slice::<Store>(bytes) {
        Ok(store) if store.version == SCHEMA_VERSION => store.windows,
        _ => HashMap::new(),
    }
}

fn encode(windows: &HashMap<String, Rect>) -> Vec<u8> {
    serde_json::to_vec_pretty(&StoreRef {
        version: SCHEMA_VERSION,
        windows,
    })
    .unwrap_or_else(|_| b"{}".to_vec())
}

/// Fit `saved` inside `work`: shrink an over-large size, then slide the origin so the whole
/// window sits within the work area.
///
/// This is the safety net that makes the reported failure — a window restored bigger than its
/// screen — impossible by construction, independent of how the stored value was produced.
fn clamp_to_work_area(saved: Rect, work: Rect) -> Rect {
    // `.max(MIN_DIM)` before `.min(work.width)` — not the reverse — so the work-area cap always
    // wins: a work area narrower than MIN_DIM (a pathologically small screen) must still bound the
    // result, even though that means falling below the floor. "Never larger than the work area" is
    // the module's headline guarantee; MIN_DIM is a best-effort floor subordinate to it.
    let width = saved.width.max(MIN_DIM).min(work.width);
    let height = saved.height.max(MIN_DIM).min(work.height);
    // `.max(work.x)` keeps the clamp range non-empty when MIN_DIM exceeds the work area (a
    // pathologically small screen); `f64::clamp` panics if min > max.
    let x = saved.x.clamp(work.x, (work.right() - width).max(work.x));
    let y = saved.y.clamp(work.y, (work.bottom() - height).max(work.y));
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Index of the work area `saved` overlaps most, or `None` when it overlaps none of them (the
/// display it was saved on is gone).
fn pick_monitor(saved: Rect, work_areas: &[Rect]) -> Option<usize> {
    work_areas
        .iter()
        .enumerate()
        .map(|(i, work)| (i, saved.overlap_area(work)))
        .filter(|(_, area)| *area > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Filename for the geometry store, scoped per config file:
/// `.window-geometry-{fnv1a_64(canonicalize(config_path)):016x}.json`.
///
/// Geometry is keyed by Tauri label *within one file*, and two different configs can reuse a
/// window title (`just run`'s `examples/config.toml` vs a real `~/.config/<app>/config.toml`), so
/// the filename is scoped by a stable hash of the canonicalized config path to keep their bounds
/// separate. Moving or renaming the config orphans its saved bounds — acceptable; the path is
/// otherwise stable.
///
/// **The policy is shared; only the *path* is app-specific.** Each app resolves its own config
/// path and hands it here, so the canonicalize → hash → format step lives here once.
pub fn geometry_filename(config_path: &Path) -> String {
    let canonical =
        std::fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
    format!(
        ".window-geometry-{:016x}.json",
        fnv1a_64(canonical.as_os_str().as_encoded_bytes())
    )
}

/// FNV-1a 64-bit hash. Small, deterministic, and — crucially — **stable across Rust toolchains**
/// (unlike `std::hash::DefaultHasher`, whose output isn't guaranteed stable across releases), so
/// the value drives a persistent on-disk filename without risk of a `rust-toolchain.toml` bump
/// silently changing it and resetting every window to default bounds. Pinned by the canonical test
/// vectors below. Non-cryptographic; collision resistance is irrelevant (the input is a single
/// trusted path).
///
/// This is **not** a drift-capable shadow of the config crates' own `fnv1a_64`
/// (`curator-config`/`warden-config`/`lector-config`'s `hash.rs`) — that copy hashes window
/// *titles* for session/label identity, a separate domain from this module's config-*path*
/// hashing. Same algorithm by coincidence of both wanting a toolchain-stable hash, not a shared
/// fact to consolidate.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Plugin-managed state: the loaded cache plus the exclusions and filename it was built with.
struct GeometryState {
    filename: String,
    skip: HashSet<String>,
    cache: Mutex<HashMap<String, Rect>>,
}

/// Windows shell-core owns that are transient by construction and must never persist bounds: the
/// home surface, and any popped-out tab window. Excluded structurally — for **save** as well as
/// restore — rather than by a caller-supplied list, because a detached window is created long
/// after startup and so could never have appeared in one.
fn is_excluded(state: &GeometryState, label: &str) -> bool {
    label == crate::home::HOME_LABEL
        || crate::detach::is_detached_label(label)
        || state.skip.contains(label)
}

/// A monitor's work area (screen minus menu bar and Dock) in points, converted with **that
/// monitor's own** scale factor. Per-monitor conversion is unambiguous; converting one monitor's
/// rect with another's factor is the bug this module exists to remove.
fn work_area_points(monitor: &Monitor) -> Rect {
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    Rect {
        x: area.position.x as f64 / scale,
        y: area.position.y as f64 / scale,
        width: area.size.width as f64 / scale,
        height: area.size.height as f64 / scale,
    }
}

/// The window's current geometry in points, or `None` when it must not be recorded.
///
/// `outer_position` + `inner_size` mirror what restore applies (`set_position` is outer,
/// `set_size` is inner), so the pair round-trips exactly.
///
/// Fullscreen is the load-bearing guard: macOS reports a fullscreen or split-view window's frame
/// as the *tile*, and tao sets its fullscreen state in `windowWillEnterFullScreen` — before the
/// resize events land — so checking here catches the tile geometry at every write path, not just
/// at exit. Persisting it would reopen the window as an ordinary window the size of the tile.
fn snapshot<R: Runtime>(window: &Window<R>) -> Option<Rect> {
    if window.is_fullscreen().unwrap_or(false) || window.is_minimized().unwrap_or(false) {
        return None;
    }
    let scale = window.scale_factor().ok()?;
    let position = window.outer_position().ok()?.to_logical::<f64>(scale);
    let size = window.inner_size().ok()?.to_logical::<f64>(scale);
    Some(Rect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    })
}

/// Choose the size to restore a saved rect at, given the work area (if any) it will land on.
/// Both `restore` branches route through here — the branch that found an overlapping monitor and
/// the branch that fell back to the primary one — so the floor/clamp precedence can't diverge
/// between them the way it did before this was factored out.
///
/// - With a work area, clamp to it: `clamp_to_work_area` already applies the `MIN_DIM` floor
///   *before* the work-area cap, so the headline guarantee (never larger than the work area) holds
///   even on a screen narrower than `MIN_DIM`. Applying `.max(MIN_DIM)` again afterwards would
///   undo that ordering and could grow the result back past the work area.
/// - With no work area at all (no monitor could be resolved), there's nothing to clamp against, so
///   only the `MIN_DIM` floor applies and the stored size is otherwise trusted as-is.
fn fit_restored_size(saved: Rect, work: Option<Rect>) -> Rect {
    match work {
        Some(work) => clamp_to_work_area(saved, work),
        None => Rect {
            width: saved.width.max(MIN_DIM),
            height: saved.height.max(MIN_DIM),
            ..saved
        },
    }
}

/// Apply a saved rect, clamped to whichever monitor it most overlaps.
///
/// `LogicalSize`/`LogicalPosition` pass through tao unscaled (`dpi`'s
/// `Position::Logical(p) => p.cast()`), so no scale factor is consulted anywhere on this path.
fn restore<R: Runtime>(window: &Window<R>, saved: Rect) {
    let work_areas: Vec<Rect> = window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(work_area_points)
        .collect();

    match pick_monitor(saved, &work_areas) {
        Some(i) => {
            let fitted = fit_restored_size(saved, Some(work_areas[i]));
            let _ = window.set_size(LogicalSize::new(fitted.width, fitted.height));
            let _ = window.set_position(LogicalPosition::new(fitted.x, fitted.y));
        }
        None => {
            // The display this window was saved on is gone. Keep the size — still clamped, against
            // the primary monitor, so a stale rect can't outgrow the screen — and drop the
            // position so macOS places the window somewhere reachable. If even the primary monitor
            // can't be resolved, there is nothing to clamp against and the stored size is applied
            // verbatim (floored at MIN_DIM) rather than left unbounded.
            let primary = window
                .primary_monitor()
                .ok()
                .flatten()
                .map(|m| work_area_points(&m));
            let fitted = fit_restored_size(saved, primary);
            let _ = window.set_size(LogicalSize::new(fitted.width, fitted.height));
        }
    }
}

/// Re-snapshot every live window into the cache and write it out.
fn flush<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<GeometryState>();

    // Collect snapshots *before* taking the cache lock, matching the Moved/Resized handler's
    // shape below: `snapshot` calls Tauri geometry getters that marshal to the main loop under
    // some runtimes/backends. `RunEvent::Exit` is delivered on the main loop, so those getters
    // run inline today and holding the lock across them wouldn't deadlock — but it's the same
    // shape the consuming apps document as a live footgun elsewhere, and it's one refactor (a
    // periodic save, a save-on-window-close, an async wrapper) from being reachable off-main. Stay
    // out of that trap by construction rather than by remembering not to hit it.
    let fresh: Vec<(String, Rect)> = app
        .windows()
        .into_iter()
        .filter(|(label, _)| !is_excluded(&state, label))
        .filter_map(|(label, window)| snapshot(&window).map(|rect| (label, rect)))
        .collect();

    let payload = {
        let mut cache = state
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.extend(fresh);
        encode(&cache)
    };

    let Ok(dir) = app.path().app_config_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = std::fs::write(dir.join(&state.filename), payload);
}

/// Build the geometry plugin. `filename` comes from [`geometry_filename`]; `skip_labels` are an
/// app's own transient windows (the home and detached surfaces are excluded structurally).
pub fn plugin<R: Runtime>(filename: String, skip_labels: &[&str]) -> TauriPlugin<R> {
    let skip: HashSet<String> = skip_labels.iter().map(|s| s.to_string()).collect();

    tauri::plugin::Builder::new("shell-geometry")
        .setup(move |app, _api| {
            let cache = app
                .path()
                .app_config_dir()
                .ok()
                .and_then(|dir| std::fs::read(dir.join(&filename)).ok())
                .map(|bytes| decode(&bytes))
                .unwrap_or_default();
            app.manage(GeometryState {
                filename: filename.clone(),
                skip: skip.clone(),
                cache: Mutex::new(cache),
            });
            Ok(())
        })
        .on_window_ready(|window| {
            let label = window.label().to_string();
            let state = window.state::<GeometryState>();
            if is_excluded(&state, &label) {
                return;
            }

            let saved = state
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&label)
                .copied();
            if let Some(rect) = saved {
                restore(&window, rect);
            }

            // Snapshot on move/resize so a window's bounds survive even when the window itself is
            // closed mid-session: by the time `flush` runs on `RunEvent::Exit`, a closed window is
            // gone from `app.windows()`, so without this event-driven cache its bounds would be
            // lost entirely rather than merely stale. (The cache is memory-only and the exit-time
            // flush is the only write to disk, so this does *not* protect against an abnormal exit
            // or crash — only against a window closing before a normal one.) No suppression around
            // `restore` is needed: an echoed event records the geometry the window genuinely has,
            // and every value on this path is already point-correct and clamped — unlike the
            // physical-pixel model, there is no value here that could be wrong to write back.
            let tracked = window.clone();
            window.on_window_event(move |event| {
                if !matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
                    return;
                }
                if let Some(rect) = snapshot(&tracked) {
                    let state = tracked.state::<GeometryState>();
                    let mut cache = state
                        .cache
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    cache.insert(tracked.label().to_string(), rect);
                }
            });
        })
        .on_event(|app, event| {
            if matches!(event, RunEvent::Exit) {
                flush(app);
            }
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// The reference failure, from the real machine: a window whose stored rect was
    /// 3380 x 2578 must not come back larger than the 3840 x 2129 work area it lands on.
    #[test]
    fn clamp_never_exceeds_the_work_area() {
        let work = rect(3840.0, 31.0, 3840.0, 2129.0);
        let out = clamp_to_work_area(rect(4975.0, 375.0, 3380.0, 2578.0), work);
        assert!(
            out.width <= work.width,
            "width {} > {}",
            out.width,
            work.width
        );
        assert!(
            out.height <= work.height,
            "height {} > {}",
            out.height,
            work.height
        );
        assert!(out.x >= work.x);
        assert!(out.y >= work.y);
        assert!(out.right() <= work.right() + f64::EPSILON);
        assert!(out.bottom() <= work.bottom() + f64::EPSILON);
    }

    #[test]
    fn clamp_leaves_a_fitting_rect_untouched() {
        let work = rect(-2056.0, 0.0, 2056.0, 1291.0);
        let saved = rect(-1325.0, 38.0, 690.0, 900.0);
        assert_eq!(clamp_to_work_area(saved, work), saved);
    }

    #[test]
    fn clamp_floors_a_degenerate_size() {
        let work = rect(0.0, 0.0, 3840.0, 2129.0);
        let out = clamp_to_work_area(rect(0.0, 0.0, 0.0, 0.0), work);
        assert_eq!(out.width, MIN_DIM);
        assert_eq!(out.height, MIN_DIM);
    }

    /// The work-area cap wins over the MIN_DIM floor: a work area narrower than MIN_DIM (200pt)
    /// in one axis must still bound the result, never the reverse. Unreachable on any real
    /// display, but the headline guarantee — never larger than the work area — must hold for all
    /// inputs, not just plausible ones.
    #[test]
    fn clamp_never_exceeds_a_work_area_smaller_than_min_dim() {
        let work = rect(0.0, 0.0, 100.0, 100.0);
        let out = clamp_to_work_area(rect(0.0, 0.0, 500.0, 500.0), work);
        assert!(
            out.width <= work.width,
            "width {} > {}",
            out.width,
            work.width
        );
        assert!(
            out.height <= work.height,
            "height {} > {}",
            out.height,
            work.height
        );
    }

    #[test]
    fn pick_monitor_takes_the_greatest_overlap() {
        let screens = [
            rect(0.0, 0.0, 3840.0, 2129.0),
            rect(-2056.0, 0.0, 2056.0, 1291.0),
        ];
        assert_eq!(
            pick_monitor(rect(-1800.0, 100.0, 1000.0, 800.0), &screens),
            Some(1)
        );
        assert_eq!(
            pick_monitor(rect(200.0, 100.0, 1000.0, 800.0), &screens),
            Some(0)
        );
    }

    #[test]
    fn pick_monitor_is_none_when_nothing_overlaps() {
        let screens = [rect(0.0, 0.0, 3840.0, 2129.0)];
        assert_eq!(
            pick_monitor(rect(-9000.0, -9000.0, 800.0, 600.0), &screens),
            None
        );
    }

    /// With a work area, `fit_restored_size` clamps into it exactly like `clamp_to_work_area` —
    /// the branch that found an overlapping (or primary) monitor.
    #[test]
    fn fit_restored_size_clamps_when_a_work_area_is_given() {
        let work = rect(0.0, 0.0, 3840.0, 2129.0);
        let saved = rect(4975.0, 375.0, 3380.0, 2578.0);
        assert_eq!(
            fit_restored_size(saved, Some(work)),
            clamp_to_work_area(saved, work)
        );
    }

    /// A work area narrower than `MIN_DIM` must still bound the result — the cap always wins over
    /// the floor, even through the extra layer of indirection this helper adds.
    #[test]
    fn fit_restored_size_never_exceeds_a_work_area_smaller_than_min_dim() {
        let work = rect(0.0, 0.0, 100.0, 100.0);
        let out = fit_restored_size(rect(0.0, 0.0, 500.0, 500.0), Some(work));
        assert!(out.width <= work.width);
        assert!(out.height <= work.height);
    }

    /// With no work area at all (primary monitor unresolvable), there is nothing to clamp
    /// against: only the `MIN_DIM` floor applies, and a size already above it passes through
    /// unclamped — this is the no-primary-monitor arm the review flagged as applying the stored
    /// size with no bound whatsoever.
    #[test]
    fn fit_restored_size_only_floors_when_no_work_area() {
        let saved = rect(10.0, 20.0, 6000.0, 5000.0);
        let out = fit_restored_size(saved, None);
        assert_eq!(out.width, 6000.0);
        assert_eq!(out.height, 5000.0);

        let degenerate = rect(0.0, 0.0, 0.0, 0.0);
        let out = fit_restored_size(degenerate, None);
        assert_eq!(out.width, MIN_DIM);
        assert_eq!(out.height, MIN_DIM);
    }

    #[test]
    fn store_round_trips() {
        let mut windows = HashMap::new();
        windows.insert("wabc".to_string(), rect(-1325.0, 870.0, 1690.0, 1289.0));
        assert_eq!(decode(&encode(&windows)), windows);
    }

    #[test]
    fn store_discards_an_unrecognised_version() {
        let json = br#"{"version":999,"windows":{"w":{"x":1.0,"y":2.0,"width":3.0,"height":4.0}}}"#;
        assert!(decode(json).is_empty());
    }

    #[test]
    fn store_discards_garbage() {
        assert!(decode(b"not json").is_empty());
        assert!(decode(b"").is_empty());
    }

    #[test]
    fn geometry_filename_is_stable_and_uses_the_new_stem() {
        let p = Path::new("/no/such/config.toml");
        let name = geometry_filename(p);
        assert_eq!(name, geometry_filename(p));
        assert!(name.starts_with(".window-geometry-"));
        assert!(name.ends_with(".json"));
    }

    #[test]
    fn fnv1a_64_matches_known_vectors() {
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    /// The one behavioural rule `is_excluded` enforces — home, detached, and caller-skip windows
    /// never persist bounds — is exactly the kind of thing a future refactor could silently
    /// invert with no error, just wrong windows restored next launch.
    #[test]
    fn is_excluded_covers_home_detached_and_the_skip_list_but_not_an_ordinary_window() {
        let state = GeometryState {
            filename: "test.json".to_string(),
            skip: ["sidebar".to_string()].into_iter().collect(),
            cache: Mutex::new(HashMap::new()),
        };
        assert!(is_excluded(&state, crate::home::HOME_LABEL));
        assert!(is_excluded(&state, "shell-detach:abc123"));
        assert!(is_excluded(&state, "sidebar"));
        assert!(!is_excluded(&state, "main"));
    }
}
