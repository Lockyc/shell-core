//! Per-window geometry persistence — size and position, in AppKit points.

// This module lands the pure primitives (rect clamp, monitor pick, JSON store) ahead of their
// caller: `register_plugins` only calls `geometry_filename` so far, and the real load/save/apply
// wiring — the thing that actually calls `clamp_to_work_area`/`pick_monitor`/`decode`/`encode` —
// is a follow-up change. Until then those are reachable only from this module's own tests, which
// don't exist in a non-test build, so `-D warnings` would otherwise fail the gate on dead code
// that has a real, imminent caller. Remove this once that caller lands.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

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
    let width = saved.width.min(work.width).max(MIN_DIM);
    let height = saved.height.min(work.height).max(MIN_DIM);
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
}
