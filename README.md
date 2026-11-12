# shell-core

![Platform](https://img.shields.io/badge/platform-macOS-555)
![Rust](https://img.shields.io/badge/rust-CE412B?logo=rust&logoColor=white)
[![License](https://img.shields.io/github/license/lockyc/shell-core)](LICENSE)

The app shell and release pipeline four macOS Tauri apps share —
[warden](https://github.com/lockyc/warden) (terminals), [curator](https://github.com/lockyc/curator)
(browser tabs), [lector](https://github.com/lockyc/lector) (local doc sites) and
[flotilla](https://github.com/lockyc/flotilla) (document workspaces). At runtime it is the windows,
menus, web views, notifications and updater every app needs; at build time it is a one-command
signed release. Each app's own content, menu items and commands stay in the app.

## Use

Pin shell-core twice — with no features (no Tauri) as a build-dependency for `build.rs`, and with
the `runtime` feature for the app itself:

```toml
[build-dependencies]
shell-core = { git = "https://github.com/lockyc/shell-core", rev = "<commit>", default-features = false }

[dependencies]
shell-core = { git = "https://github.com/lockyc/shell-core", rev = "<commit>", features = ["runtime"] }
```

`build.rs` writes the release scripts into the app and stamps the build for the About box:

```rust
shell_core::materialize_scripts(std::path::Path::new("../scripts"))?; // path from the crate to repo scripts/
shell_core::build_stamp();
```

and the app commits its `scripts/tooling.env` (template: `scripts/tooling.env.example`). At
runtime, the shared plugins — window geometry, the updater and its relaunch, and the Edit-menu
handling — go on the Tauri builder:

```rust
let builder = shell_core::register_plugins(tauri::Builder::default(), Some(&config_path), &[]);
```

An app that needs only banners and window geometry takes those two features alone:
`default-features = false, features = ["banner", "geometry"]`.

## What it provides

**Build and release (default features, zero dependencies)** — `release.sh`, the one-command
release: build, notarize, tag, fast-forward `main`, push and publish the GitHub release with its
assets; the updater-manifest, install and launch helpers; `core-patch.sh`, which switches an app
between a local checkout of a shared core and its pinned rev; and the zero-dependency helpers
`tabs::pick_live_neighbour` (the tab that becomes active after one unloads) and `label` (child
web-view names, `{window}:{within}`).

**Runtime (`runtime` feature):**

- **Windows** — `geometry` (each window's size and position, restored correctly across monitors),
  `windows` (opening, renaming and closing the config's windows, and the New/Edit/Delete Window
  form), `home` (the window shown when no other is open), and `detach` (a tab popped out into its
  own window).
- **Menus** — the shared App, Config, Window, Edit and Tab menus, with the shortcuts every app
  shares, and `menu::route` for which window a command acts on.
- **Web views** — the content-webview builder (`web::content`) with link routing, login stores,
  downloads, the page-load bar and mouse back/forward, plus `compositing` to place content beside
  the sidebar.
- **Platform glue** — `watch` (the config hot-reload watcher), `banner` (native notifications that
  hand back the window and tab), and `edit_keys` (undo and select-all reach the page first).

Every module and its contract: the crate's rustdoc (`cargo doc --open --features runtime`). Kept
in each app, not here: sending events to its web views, warden's own config watcher and native
compositing, each app's own menu items, and curator's check that only its sidebar can call certain
commands.

## Status

In use by all four apps, each pinned to a commit rev; there are no tags or releases.

## Related

[chrome-core](https://github.com/lockyc/chrome-core) (the sidebar) and
[config-core](https://github.com/lockyc/config-core) (config primitives) are the other two cores
the apps share.

## Development

```sh
git clone https://github.com/lockyc/shell-core && cd shell-core
just gate       # zero-dep + fmt check + clippy + tests
just test       # the test suite
just zero-dep   # prove the default feature set still pulls no dependencies
```

Conventions: [CLAUDE.md](CLAUDE.md).

## License

MIT — see [LICENSE](LICENSE).
