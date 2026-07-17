//! Shared Tauri app-shell + release-tooling layer for the curator and warden apps (and future
//! siblings). Consumed by git-rev pin, like `chrome-core` (the sidebar view) and `config-core`
//! (config primitives). Two concerns, split by Cargo feature so a build-dependency stays light:
//!
//! - **Build/release tooling (default, zero-dep).** The three release scripts are the source of
//!   truth here in `scripts/`, embedded as [`RELEASE_SH`]/[`GEN_LATEST_SH`]/[`INSTALL_APP_SH`]. A
//!   consumer's `build.rs` writes them into its own `scripts/` (git-ignored) — the same
//!   embed-and-materialize pattern `chrome-core` uses for its CSS/JS. The scripts are generic;
//!   every app-specific value is read from a tracked per-app `scripts/tooling.env` (`APP_NAME`,
//!   `TAURI_CRATE_DIR`, `UPDATER_REPO`). [`build_stamp`] is the other build-time helper: a git
//!   sha/date stamp for the About box.
//! - **Runtime (`runtime` feature).** [`register_plugins`] installs the plugins every app registers
//!   identically (window-state + updater + process) and the home surface's custom protocol. [`menu`]
//!   builds the shared menu spine — the App/Config/Window submenus, identical across apps; each
//!   app's own items (curator's Reload Tab, warden's tab semantics) interleave with it. [`home`] is
//!   the surface an app shows when it would otherwise have no window (no config / a load error / a
//!   valid config's window list), so it is never stranded invisible. Deliberately NOT shared: IPC
//!   fan-out and the config watcher (diverged in structure per app), and the chrome-caller command
//!   gate (curator-only — warden hosts no untrusted webviews).

/// Embedded source of `scripts/release.sh` — the generic build+notarize+upload release script.
/// A consumer's `build.rs` writes this into its own `scripts/release.sh` (git-ignored).
pub const RELEASE_SH: &str = include_str!("../scripts/release.sh");
/// Embedded source of `scripts/gen-latest-json.sh` — the tauri-updater manifest generator.
pub const GEN_LATEST_SH: &str = include_str!("../scripts/gen-latest-json.sh");
/// Embedded source of `scripts/install-app.sh` — the /Applications installer for local builds.
pub const INSTALL_APP_SH: &str = include_str!("../scripts/install-app.sh");

/// Materialize the three embedded release scripts into `<dir>` (each git-ignored in the consumer),
/// preserving the executable bit. Call from `build.rs` with the app's `scripts/` dir so a plain
/// clone can build + release from the pinned shell-core rev without a tracked copy to drift.
///
/// The per-app `scripts/tooling.env` is NOT written here — it is tracked, committed once per app.
pub fn materialize_scripts(dir: &std::path::Path) -> std::io::Result<()> {
    for (name, body) in [
        ("release.sh", RELEASE_SH),
        ("gen-latest-json.sh", GEN_LATEST_SH),
        ("install-app.sh", INSTALL_APP_SH),
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

#[cfg(feature = "runtime")]
pub mod menu;

#[cfg(feature = "runtime")]
pub mod home;

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
    use tauri::{Builder, Runtime};

    /// Register the plugins every consuming app installs identically: window-state (persist each
    /// window's size/position/maximized, keyed per-config-file via `state_filename`), the updater,
    /// and the process plugin (for the updater's relaunch). Returns the builder for continued
    /// chaining (`.setup(..).invoke_handler(..)` etc.).
    ///
    /// `state_filename` is the per-app window-state filename (each app derives its own, scoped by a
    /// hash of its config path). `skip_labels` are transient windows excluded from state restore
    /// (warden: its diagnostic + launcher windows; curator: its error window).
    pub fn register_plugins<R: Runtime>(
        builder: Builder<R>,
        state_filename: String,
        skip_labels: &[&str],
    ) -> Builder<R> {
        use tauri_plugin_window_state::StateFlags;
        let mut ws = tauri_plugin_window_state::Builder::default()
            .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED);
        for label in skip_labels {
            ws = ws.skip_initial_state(label);
        }
        let builder = builder
            .plugin(ws.with_filename(state_filename).build())
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init());
        // The home surface's page is served over its own custom protocol rather than materialized
        // into each consumer's frontendDist — see `home::HOME_SCHEME`'s doc for why that's what
        // keeps its webview's origin classified `local` (so its commands need no extra capability
        // wiring). Registered here alongside the rest of the identical runtime setup.
        crate::home::register_protocol(builder)
    }
}

#[cfg(feature = "runtime")]
pub use runtime::register_plugins;
