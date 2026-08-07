//! Shared Tauri app-shell + release-tooling layer for the curator, warden, and lector apps (and
//! future siblings). Consumed by git-rev pin, like `chrome-core` (the sidebar view) and
//! `config-core` (config primitives). Two concerns, split by Cargo feature so a build-dependency
//! stays light:
//!
//! - **Build/release tooling (default, zero-dep).** The release/deploy scripts are the source of
//!   truth here in `scripts/`, embedded as
//!   [`RELEASE_SH`]/[`GEN_LATEST_SH`]/[`INSTALL_APP_SH`]/[`LAUNCH_APP_SH`]. A
//!   consumer's `build.rs` writes them into its own `scripts/` (git-ignored) — the same
//!   embed-and-materialize pattern `chrome-core` uses for its CSS/JS. The scripts are generic;
//!   every app-specific value is read from a tracked per-app `scripts/tooling.env` (`APP_NAME`,
//!   `TAURI_CRATE_DIR`, `UPDATER_REPO`). [`build_stamp`] is the other build-time helper: a git
//!   sha/date stamp for the About box.
//! - **Runtime (`runtime` feature).** [`register_plugins`] installs the plugins every app registers
//!   identically (the updater + process plugins) and the home + detach surfaces' custom protocols.
//!   [`geometry`] persists each window's size/position — in AppKit points, clamped to the target
//!   monitor's work area on restore, and never recorded while a window is fullscreen or minimized.
//!   It replaces `tauri-plugin-window-state`, whose physical-pixel model is wrong across monitors
//!   of differing scale factor: physical is points × the occupied screen's scale factor, so a rect
//!   saved on a 2x display and applied on a 1x one is out by the ratio. Given an app's resolved
//!   config path it derives `.window-geometry-{fnv1a_64(canonicalize(path)):016x}.json`
//!   ([`geometry_filename`]), the canonicalize→hash→format step that was copied per app (only the
//!   *path* is app-specific).
//!   [`menu`] builds the shared menu spine — the App/Config/Window submenus, identical across apps,
//!   plus the Close Tab and Pop Out Tab items; each app's own items (curator's Reload Tab, warden's
//!   tab semantics) interleave with it. [`home`] is the surface an app shows when it would otherwise
//!   have no window (no config / a load error / a valid config's window list), so it is never
//!   stranded invisible. [`detach`] is the "pop a tab out into its own temporary window" lifecycle —
//!   the label scheme + banner-shell window a detached tab gets; the app owns moving the tab's actual
//!   content and all origin bookkeeping. [`compositing`] is the hole-punch content-webview placement
//!   shared by curator + lector (the [`compositing::HoleRect`] rect + [`compositing::layout_webviews`]);
//!   warden composites a native `NSView` through its own geometry, so it is not a consumer.
//!   [`watch`] is the config-file hot-reload watcher for curator + lector (parent-dir watch, file-name
//!   match, echo-swallow via a config-agnostic seam — the app parses); warden's own watcher parses
//!   inside + drives a deeper reconcile, so it keeps its own. Deliberately NOT shared: IPC fan-out
//!   (per-app event shapes) and warden's native compositing/registry. The per-caller
//!   command-isolation model — why a Tauri command needs a label gate, or doesn't (origin dispatch's
//!   job, given no app ACL manifest) — is documented once in this crate's CLAUDE.md, not per app.

/// Embedded source of `scripts/release.sh` — the generic build+notarize+upload release script.
/// A consumer's `build.rs` writes this into its own `scripts/release.sh` (git-ignored).
pub const RELEASE_SH: &str = include_str!("../scripts/release.sh");
/// Embedded source of `scripts/gen-latest-json.sh` — the tauri-updater manifest generator.
pub const GEN_LATEST_SH: &str = include_str!("../scripts/gen-latest-json.sh");
/// Embedded source of `scripts/install-app.sh` — the /Applications installer for local builds.
pub const INSTALL_APP_SH: &str = include_str!("../scripts/install-app.sh");
/// Embedded source of `scripts/launch-app.sh` — the clean-environment launcher for a deployed
/// build. `install-app.sh` deliberately never launches; this is its counterpart, and it exists
/// because a bare `open` forwards the deploying terminal's whole environment to the app (the
/// script's own header carries the full footgun).
pub const LAUNCH_APP_SH: &str = include_str!("../scripts/launch-app.sh");

/// Materialize the embedded release/deploy scripts into `<dir>` (each git-ignored in the consumer),
/// preserving the executable bit. Call from `build.rs` with the app's `scripts/` dir so a plain
/// clone can build + release from the pinned shell-core rev without a tracked copy to drift.
///
/// The per-app `scripts/tooling.env` is NOT written here — it is tracked, committed once per app.
pub fn materialize_scripts(dir: &std::path::Path) -> std::io::Result<()> {
    for (name, body) in [
        ("release.sh", RELEASE_SH),
        ("gen-latest-json.sh", GEN_LATEST_SH),
        ("install-app.sh", INSTALL_APP_SH),
        ("launch-app.sh", LAUNCH_APP_SH),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

/// Shared tab-selection policy (`pick_live_neighbour`). Zero-dependency, so it stays on the
/// default (non-`runtime`) surface — every consumer, build-dep or runtime dep, can call it.
pub mod tabs;
pub use tabs::pick_live_neighbour;

#[cfg(feature = "runtime")]
pub mod menu;

#[cfg(feature = "runtime")]
pub mod home;

#[cfg(feature = "runtime")]
pub mod detach;

#[cfg(feature = "runtime")]
pub mod compositing;

#[cfg(feature = "runtime")]
pub mod watch;

#[cfg(feature = "runtime")]
pub mod mouse_nav;

#[cfg(feature = "runtime")]
pub mod progress_bar;

/// Per-window geometry persistence — size/position in AppKit points, clamped to the target
/// monitor on restore. Wired by [`register_plugins`].
#[cfg(feature = "runtime")]
pub mod geometry;

/// Emit a build stamp so the About box can confirm the installed app matches a given commit. Prints
/// `cargo:rustc-env=BUILD_GIT_SHA=<short>[-dirty]` and `cargo:rustc-env=BUILD_DATE=<YYYY-MM-DD>`,
/// plus a `rerun-if-changed` on the git ref log so it re-stamps on every commit/checkout. Call from
/// a consumer's `build.rs`; read the values with `env!("BUILD_GIT_SHA")` / `env!("BUILD_DATE")`.
///
/// Zero-dependency (shells `git`/`date`) so it is safe to call from a light `[build-dependencies]`.
pub fn build_stamp() {
    fn git(args: &[&str]) -> Option<String> {
        let out = std::process::Command::new("git").args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    let sha = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let sha = if dirty { format!("{sha}-dirty") } else { sha };
    let date = std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    println!("cargo:rustc-env=BUILD_GIT_SHA={sha}");
    println!("cargo:rustc-env=BUILD_DATE={date}");
    // Re-stamp after any git ref update (commit/checkout). `--git-path` resolves logs/HEAD from any
    // crate depth (warden's crate is two levels down, curator's one), so no per-app relative path.
    if let Some(logs) = git(&["rev-parse", "--git-path", "logs/HEAD"]) {
        println!("cargo:rerun-if-changed={logs}");
    }
}

#[cfg(feature = "runtime")]
mod runtime {
    use std::path::Path;
    use tauri::{Builder, Runtime};

    /// Register the plugins every consuming app installs identically: window geometry (persist
    /// each window's size/position in points, keyed per-config-file via
    /// [`crate::geometry::geometry_filename`]), the updater, and the process plugin (for the
    /// updater's relaunch). Returns the builder for continued chaining.
    ///
    /// `config_path` is the app's resolved config path — `Some(path)` scopes the geometry file to
    /// it; `None` uses an unscoped default name.
    ///
    /// `skip_labels` are for an app's own transient windows, excluded from both save and restore.
    /// No caller passes any today — all three apps pass `&[]` — so the parameter stays reserved
    /// for a future app-specific transient window. The home surface and every detached-tab window
    /// are excluded structurally inside [`crate::geometry`] and must not be listed here.
    pub fn register_plugins<R: Runtime>(
        builder: Builder<R>,
        config_path: Option<&Path>,
        skip_labels: &[&str],
    ) -> Builder<R> {
        let filename = config_path
            .map(crate::geometry::geometry_filename)
            .unwrap_or_else(|| ".window-geometry.json".to_string());
        let builder = builder
            .plugin(crate::geometry::plugin(filename, skip_labels))
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init());
        // The home surface's and the detach surface's pages are each served over their own custom
        // protocol rather than materialized into each consumer's frontendDist — see
        // `home::HOME_SCHEME`'s doc for why that's what keeps their webviews' origin classified
        // `local` (so their commands need no extra capability wiring). Registered here alongside
        // the rest of the identical runtime setup.
        let builder = crate::home::register_protocol(builder);
        crate::detach::register_detach_protocol(builder)
    }
}

#[cfg(feature = "runtime")]
pub use geometry::geometry_filename;
#[cfg(feature = "runtime")]
pub use runtime::register_plugins;
