//! Shared Tauri app-shell + release-tooling layer, consumed by git-rev pin. Two concerns, split by
//! Cargo feature so a build-dependency stays light:
//!
//! - **Build/release tooling (default, zero-dep).** [`materialize_scripts`] writes the embedded
//!   release/install/core-patch scripts into a consumer's own `scripts/` from its `build.rs`;
//!   [`build_stamp`] stamps the build's git sha and date for the About box.
//! - **Runtime (`runtime` feature).** [`register_plugins`] installs what every app registers
//!   identically; each module's own docblock carries its contract.
//!
//! What belongs here and what deliberately does not is the crate's CLAUDE.md.

/// Embedded source of `scripts/release.sh` — the one-command release: build + notarize, then tag,
/// fast-forward main, push, and publish the GitHub release with its assets.
/// A consumer's `build.rs` writes this into its own `scripts/release.sh` (git-ignored).
pub const RELEASE_SH: &str = include_str!("../scripts/release.sh");
/// Embedded source of `scripts/gen-latest-json.sh` — the tauri-updater manifest generator.
pub const GEN_LATEST_SH: &str = include_str!("../scripts/gen-latest-json.sh");
/// Embedded source of `scripts/install-app.sh` — the /Applications installer for local builds.
pub const INSTALL_APP_SH: &str = include_str!("../scripts/install-app.sh");
/// Embedded source of `scripts/launch-app.sh` — the clean-environment launcher for an installed
/// build. `install-app.sh` deliberately never launches; this is its counterpart, and it exists
/// because a bare `open` forwards the installing terminal's whole environment to the app (the
/// script's own header carries the full footgun).
pub const LAUNCH_APP_SH: &str = include_str!("../scripts/launch-app.sh");
/// Embedded source of `scripts/core-patch.sh` — the shared-core `[patch]` dev loop (`dev`/`pin`/
/// `guard`) behind every app's `<core>-dev` / `<core>-pin` recipes and its gate's patch guard.
pub const CORE_PATCH_SH: &str = include_str!("../scripts/core-patch.sh");

/// Every embedded script, file name → body: the one roster `materialize_scripts` writes and the
/// tests guard, so a script added here is both materialized and checked.
pub const SCRIPTS: [(&str, &str); 5] = [
    ("release.sh", RELEASE_SH),
    ("gen-latest-json.sh", GEN_LATEST_SH),
    ("install-app.sh", INSTALL_APP_SH),
    ("launch-app.sh", LAUNCH_APP_SH),
    ("core-patch.sh", CORE_PATCH_SH),
];

/// Materialize the embedded scripts into `<dir>` (each git-ignored in the consumer),
/// preserving the executable bit. Call from `build.rs` with the app's `scripts/` dir so a plain
/// clone can build + release from the pinned shell-core rev without a tracked copy to drift.
///
/// The per-app `scripts/tooling.env` is NOT written here — it is tracked, committed once per app.
pub fn materialize_scripts(dir: &std::path::Path) -> std::io::Result<()> {
    for (name, body) in SCRIPTS {
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

/// The `{window}:{within}` child-label scheme. Zero-dependency, like [`tabs`].
pub mod label;
pub use label::{child_label, window_of};

#[cfg(feature = "runtime")]
pub mod menu;

#[cfg(feature = "runtime")]
pub mod home;

/// Pure window plan (labels, reconcile, menu entries) for the windows an app declares.
#[cfg(feature = "runtime")]
pub mod windows;

#[cfg(feature = "runtime")]
pub mod detach;

#[cfg(feature = "runtime")]
pub mod compositing;

#[cfg(feature = "runtime")]
pub mod watch;

/// Launching an app without taking the operator's focus (`SHELL_CORE_UNFOCUSED=1`).
#[cfg(any(feature = "banner", feature = "geometry"))]
pub mod focus;

/// Native macOS banners that surface the tab that raised them when clicked.
#[cfg(feature = "banner")]
pub mod banner;

/// The app-wide download list behind the downloads popup (pure model).
#[cfg(feature = "runtime")]
pub mod downloads;

/// App-agnostic content-webview layer: link routing, login-store identity, allowlisted
/// self-signed TLS, the content-webview builder.
#[cfg(feature = "runtime")]
pub mod web;

#[cfg(feature = "runtime")]
pub mod mouse_nav;

#[cfg(feature = "runtime")]
pub mod edit_keys;

/// Spawn `cmd` without blocking the caller, and wait on it from a throwaway thread — the crate's
/// one spawn path for a fire-and-forget `open`. Dropping a `Child` never reaps it, so an unwaited
/// `open` would linger as a zombie for the app's life.
#[cfg(feature = "runtime")]
pub(crate) fn spawn_reaped(cmd: &mut std::process::Command) {
    if let Ok(mut child) = cmd.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Run `f` on the main thread once the scheme request the caller is serving has returned: the
/// crate's one path for a shell page closing its own webview or window from inside its own request
/// (the form's result, the downloads popup's close). A close run synchronously there tears the
/// webview down while its request is still running; posting it from another thread queues it
/// behind the response.
#[cfg(feature = "runtime")]
pub(crate) fn after_request<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    f: impl FnOnce() + Send + 'static,
) {
    let app = app.clone();
    std::thread::spawn(move || {
        let _ = app.run_on_main_thread(f);
    });
}

#[cfg(feature = "runtime")]
pub mod progress_bar;

/// Per-window geometry persistence — size/position in AppKit points, clamped to the target
/// monitor on restore. Wired by [`register_plugins`], or alone through the `geometry` feature.
#[cfg(feature = "geometry")]
pub mod geometry;

/// Every window builds hidden and opens on a desktop, never over the app's own fullscreen space.
#[cfg(feature = "runtime")]
pub mod spaces;

/// Emit a build stamp so the About box can confirm the installed app matches a given commit. Prints
/// `cargo:rustc-env=BUILD_GIT_SHA=<short>[-dirty]` and `cargo:rustc-env=BUILD_DATE=<YYYY-MM-DD>`.
/// Call from a consumer's `build.rs`; read the values with `env!("BUILD_GIT_SHA")` /
/// `env!("BUILD_DATE")`.
///
/// The stamp is re-evaluated on **every** build: `-dirty` depends on the whole working tree (an
/// unstaged edit, a stage, a stash pop), which no single `rerun-if-changed` path captures, so the
/// script reruns on a path that never exists. A stamp claiming a clean tree for a dirty build is
/// worse than none; the cost is re-running the consumer's `build.rs` each build.
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
    // A missing path is always "changed", so cargo reruns the script every build (see above).
    println!("cargo:rerun-if-changed=shell-core-build-stamp.always-rerun");
}

#[cfg(feature = "runtime")]
mod runtime {
    use std::path::Path;
    use tauri::{Builder, Runtime};

    /// Register what every consuming app installs identically, and return the builder for
    /// continued chaining:
    ///
    /// - plugins: window geometry (keyed per config file via
    ///   [`crate::geometry::geometry_filename`]), window placement ([`crate::spaces`], after
    ///   geometry so bounds restore while the window is hidden), the updater, the process plugin (the updater's
    ///   relaunch) and [`crate::edit_keys`];
    /// - custom protocols: the home surface ([`crate::home`]), the window form
    ///   ([`crate::windows::form`]), the downloads popup ([`crate::downloads::popup`]) and the
    ///   detached-tab window ([`crate::detach`]).
    ///
    /// It reads [`crate::focus::UNFOCUSED_ENV`], so an app launched with it never activates.
    ///
    /// `config_path` is the app's resolved config path — `Some(path)` scopes the geometry file to
    /// it; `None` uses an unscoped default name.
    ///
    /// `skip_labels` are for an app's own transient windows, excluded from both save and restore.
    /// No caller passes any — every app passes `&[]` — and the parameter is slated for removal
    /// at the next API-breaking bump (the deferral is in CLAUDE.md). The home surface and the
    /// window form are excluded structurally inside [`crate::geometry`] and must not be listed
    /// here; a detached-tab window is deliberately kept (`geometry::is_excluded` says why).
    pub fn register_plugins<R: Runtime>(
        builder: Builder<R>,
        config_path: Option<&Path>,
        skip_labels: &[&str],
    ) -> Builder<R> {
        let filename = config_path
            .map(crate::geometry::geometry_filename)
            .unwrap_or_else(|| ".window-geometry.json".to_string());
        // A plugin's setup runs on the main thread, as the `edit_keys` monitor requires.
        let edit_keys = tauri::plugin::Builder::<R>::new("shell-edit-keys")
            .setup(|_, _| {
                crate::edit_keys::install();
                Ok(())
            })
            .build();
        // Launched unfocused, the app activates only if no other app is active: never, in practice.
        #[cfg(target_os = "macos")]
        let builder = builder.activate_ignoring_other_apps(crate::focus::focused());
        let builder = builder
            .plugin(edit_keys)
            .plugin(crate::geometry::plugin(filename, skip_labels))
            .plugin(crate::spaces::plugin())
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_process::init());
        // The home surface's and the detach surface's pages are each served over their own custom
        // protocol rather than materialized into each consumer's frontendDist — see
        // `home::HOME_SCHEME`'s doc for why that's what keeps their webviews' origin classified
        // `local` (so their commands need no extra capability wiring). Registered here alongside
        // the rest of the identical runtime setup.
        let builder = crate::home::register_protocol(builder);
        let builder = crate::windows::form::register_protocol(builder);
        let builder = crate::downloads::popup::register_protocol(builder);
        crate::detach::register_detach_protocol(builder)
    }
}

#[cfg(feature = "runtime")]
pub use geometry::{config_scoped_filename, geometry_filename};
#[cfg(feature = "runtime")]
pub use runtime::register_plugins;
