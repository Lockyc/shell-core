<h1 align="center">shell-core</h1>

<p align="center">The shared Tauri app-shell + release tooling for <a href="https://github.com/Lockyc/curator">curator</a>, <a href="https://github.com/Lockyc/warden">warden</a>, and <a href="https://github.com/Lockyc/lector">lector</a> — one release pipeline, many apps.</p>

<p align="center">
  <a href="https://github.com/lockyc/shell-core/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/lockyc/shell-core/actions/workflows/ci.yml/badge.svg"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS-555">
  <img alt="Rust" src="https://img.shields.io/badge/built%20with-Rust-CE412B?logo=rust&logoColor=white">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/Lockyc/shell-core"></a>
</p>

curator (browser keeper-tabs), warden (terminals), and lector (local doc sites) are sibling macOS
Tauri apps. `shell-core` holds their shared **build/release tooling** and **Tauri runtime setup**,
alongside [chrome-core](https://github.com/Lockyc/chrome-core) (the view) and
[config-core](https://github.com/Lockyc/config-core) (config primitives).

It has two parts, split by Cargo feature:

- **Build/release tooling (default, zero-dependency).** The scripts in `scripts/`: `release.sh`
  (the one-command release — build + notarize, then tag, fast-forward `main`, push and publish the
  GitHub release with its assets), the updater-manifest, install and launch helpers, and
  `core-patch.sh` (the shared-core `[patch]` dev/pin loop). A consuming app's `build.rs` writes them
  into its own `scripts/` (git-ignored) via `materialize_scripts`. App-specific values are read
  from a tracked per-app `scripts/tooling.env` (`scripts/tooling.env.example`). `build_stamp()`
  is the other build-time helper — a git sha/date stamp for the About box. `tabs::pick_live_neighbour`
  (which sibling tab becomes active after one unloads) also sits on this zero-dependency surface.
- **Runtime (`runtime` feature).** The shared app-shell:
  - `register_plugins(builder, config_path, skip_labels)` installs the shared plugins — `geometry`,
    the updater, and the process plugin (for the updater's relaunch) — and
    the custom protocols that serve the home and detach surfaces.
  - `geometry` persists each window's size and position in AppKit points, clamped to the target
    monitor's work area on restore, and never recorded while a window is fullscreen or minimized.
    The file is scoped to the app's config path (`geometry_filename`). Restored geometry is
    correct across monitors with different scale factors.
  - `menu::build_spine()` builds the **menu spine** — the App submenu (About, Check for Updates…),
    the Config submenu (Edit Config / Reveal in Finder), and the Window submenu (a checked
    per-window selector), plus the Close Tab / Pop Out Tab / Find in Sidebar items and the
    family-standard close accelerators (⌘W a tab, ⌘⇧W the window) as constants. It returns the
    submenus and items for the app to interleave with its own per-app items; it does not set the
    menu. `menu::build_tab_nav()` builds the tab-navigation block (Previous/Next Tab, ⌘1–⌘9).
  - `home::{home_state, show_home, close_home}` is the **home surface** — what an app shows when it
    would otherwise have no window (no config / a load error / a valid config's window list), so a
    fresh install never launches to nothing. shell-core owns the surface; the app wires the
    actions, including the "Create a starter config" button.
  - `detach` pops a tab out into its own window — the label scheme and the banner-shell page; the
    app moves the tab's content.
  - `compositing` places content webviews into the chrome's reported hole (`HoleRect`,
    `layout_webviews`) — curator and lector.
  - `watch` is the config-file hot-reload watcher (it ignores the app's own format-on-save
    writes); the app parses — curator and lector.
  - `mouse_nav` drives WKWebView back/forward from the mouse side buttons, and `progress_bar` draws
    the page-load bar on each content webview — curator and lector.

## Status

In use by warden, curator, and lector. Kept per app, not here: IPC fan-out, warden's config
watcher and native compositing, each app's own menu items, and curator's chrome-caller command
gate.

## How it's consumed

Each app pins shell-core by git rev: default features for `build.rs` (no tauri), the `runtime`
feature for the app:

```toml
[build-dependencies]
shell-core = { git = "https://github.com/Lockyc/shell-core", rev = "<commit>", default-features = false }

[dependencies]
shell-core = { git = "https://github.com/Lockyc/shell-core", rev = "<commit>", features = ["runtime"] }
```

`build.rs` materializes the scripts + stamps the build:

```rust
shell_core::materialize_scripts(std::path::Path::new("../scripts"))?; // path from the crate to repo scripts/
shell_core::build_stamp();
```

and the app commits its `scripts/tooling.env` (see `scripts/tooling.env.example`).

## Development

```
just gate       # pre-merge: zero-dep + fmt check + clippy + tests
just test       # the test suite
just zero-dep   # prove the default feature set still pulls no dependencies
```

## License

MIT © Lachlan Collins
