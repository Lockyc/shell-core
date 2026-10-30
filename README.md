<h1 align="center">shell-core</h1>

<p align="center">The shared Tauri app-shell + release tooling for <a href="https://github.com/Lockyc/curator">curator</a>, <a href="https://github.com/Lockyc/warden">warden</a>, and <a href="https://github.com/Lockyc/lector">lector</a> — one release pipeline, many apps.</p>

<p align="center">
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
  `core-patch.sh` (switches an app between a local checkout of a shared core and its pinned git
  rev). A consuming app's `build.rs` writes them into its own `scripts/` (git-ignored) via
  `materialize_scripts`. App-specific values are read from a tracked per-app
  `scripts/tooling.env` (template: `scripts/tooling.env.example`). The second build-time helper,
  `build_stamp()`, stamps the git sha and date for the About box. This feature set also exports
  `tabs::pick_live_neighbour`, which picks the tab that becomes active after one unloads.
- **Runtime (`runtime` feature).** The shared app-shell:
  - `register_plugins(builder, config_path, skip_labels)` installs the shared plugins — `geometry`,
    the updater, and the process plugin (for the updater's relaunch) — and
    the custom protocols that serve the home and detach windows (below).
  - `geometry` persists each window's size and position in AppKit points, clamped to the target
    monitor's work area on restore, and never recorded while a window is fullscreen or minimized.
    Each config file gets its own saved geometry (`geometry_filename`). Restored geometry is
    correct across monitors with different scale factors.
  - `menu::build_spine()` builds the **menu spine** — the App submenu (About, Check for Updates…),
    the Config submenu (Edit Config / Reveal in Finder), the Window submenu (a checked per-window
    selector) and the Edit submenu, with constants for the shortcuts every app shares (⌘W closes a
    tab, ⌘⇧W the window). `menu::build_tab_menu()` builds the whole **Tab** submenu — Previous/Next
    Tab, the Cycle Tabs with ⌘1/⌘2 toggle, Close / Pop Out / Find in Sidebar, the app's own item
    groups, Tab 1–9 — and `menu::route()` decides which window a menu command acts on, including
    from a popped-out tab's window. The app places the submenus; nothing here sets the menu.
  - `home::{home_state, show_home, close_home}` is the **home surface**: the window an app shows
    when no other window is open. It shows the window list, a load error, or (with no config) a
    prompt to create one, so a fresh install never launches to nothing. The app wires the actions,
    including the "Create a starter config" button.
  - `windows` runs every app's config-declared windows: `WindowSet` opens, focuses, renames in
    place and closes them from the app's declarations (and can quit when the last one closes), and
    `form` + `flow` are the New/Edit/Delete Window form and its menu items. The app builds each
    window's content and writes the form's result to its own config. `label` names a window's
    child webviews (`{window}:{within}`).
  - `detach` pops a tab out into its own window: shell-core provides the window and its banner;
    the app moves the tab's content into it.
  - `compositing` positions content webviews in the area the sidebar leaves free (`HoleRect`,
    `layout_webviews`). Used by curator and lector.
  - `watch` is the config-file hot-reload watcher (it ignores the app's own format-on-save
    writes); the app re-parses the file. Used by curator and lector.
  - `mouse_nav` drives WKWebView back/forward from the mouse side buttons, and `progress_bar` draws
    the page-load bar on each content webview. Used by curator and lector.
  - `web` is the content-webview layer: link routing and escape sentinels (`web::escape`), the
    WebKit login-store id for a session seed (`web::session`), self-signed TLS for allowlisted
    hosts (`web::insecure`, macOS), and the content-webview builder (`web::content`), which takes
    a spec plus app hooks (`intercept`, `on_app_sentinel`, `on_title`, `on_download`). Every
    content webview keeps an opened Web Inspector in its own window, off the sidebar
    (`web::inspector`), and leaves the page free to define its own global `ipc`
    (`web::ipc_global`), which Google Sheets needs to load.

## Status

In use by warden, curator, lector and flotilla. Kept in each app, not here: sending events to the app's
webviews, warden's config watcher and native compositing, each app's own menu items, and
curator's check that only its sidebar can call certain commands.

## How it's consumed

Each app pins shell-core by git rev twice: with no features (no Tauri) as a build-dependency for
`build.rs`, and with the `runtime` feature for the app itself:

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
