<h1 align="center">shell-core</h1>

<p align="center">The shared Tauri app-shell + release tooling for <a href="https://github.com/Lockyc/curator">curator</a>, <a href="https://github.com/Lockyc/warden">warden</a>, and <a href="https://github.com/Lockyc/lector">lector</a> — one release pipeline, many apps.</p>

<p align="center">
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS-555">
  <img alt="Rust" src="https://img.shields.io/badge/built%20with-Rust-CE412B?logo=rust&logoColor=white">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/Lockyc/shell-core"></a>
</p>

curator (browser keeper-tabs), warden (terminals), and lector (local doc sites) are sibling macOS
Tauri apps. They already share a **view** ([chrome-core](https://github.com/Lockyc/chrome-core))
and **config primitives** ([config-core](https://github.com/Lockyc/config-core)). `shell-core` is
the third shared layer: the
**build/release tooling** and the sliver of **Tauri runtime setup** that is identical for any such
app — extracted once so a new sibling app inherits it instead of copy-pasting (and drifting).

It carries two concerns, split by Cargo feature so a build-dependency stays light:

- **Build/release tooling (default, zero-dependency).** The three release scripts live here in
  `scripts/` as the source of truth, embedded as string constants (`RELEASE_SH`, `GEN_LATEST_SH`,
  `INSTALL_APP_SH`). A consuming app's `build.rs` writes them into its own `scripts/` (git-ignored) —
  the same embed-and-materialize pattern chrome-core uses for its CSS/JS. The scripts are **generic**;
  every app-specific value is read from a tracked per-app `scripts/tooling.env` (three keys:
  `APP_NAME`, `TAURI_CRATE_DIR`, `UPDATER_REPO`). `build_stamp()` is the other build-time helper — a
  git sha/date stamp for the About box.
- **Runtime (`runtime` feature).** Three pieces of app-shell every app would otherwise hand-roll:
  - `register_plugins()` installs the plugins every app registers identically: window-state,
    updater, process.
  - `menu::build_spine()` builds the **menu spine** — the App submenu (About, Check for Updates…),
    the Config submenu (Edit Config / Reveal in Finder), and the Window submenu (a checked
    per-window selector), plus the family-standard close accelerators (⌘W a tab, ⌘⇧W the window) as
    constants so they can't drift per app again. It returns the submenus for the app to interleave
    with its own genuinely-per-app items; it does not set the menu.
  - `home::{home_state, show_home, close_home}` is the **home surface** — what an app shows when it
    would otherwise have no window (no config / a load error / a valid config's window list), so a
    fresh install never launches to nothing. shell-core owns the surface; the app wires the actions
    (the "Create a starter config" button is the app's own command, which is how shell-core stays
    free of any dependency on config-core).

## Status

In use. Extracted from warden and curator's copy-pasted tooling; all three apps (warden, curator,
and lector) now consume it, pinned to a `0.1.x` rev. **Deliberately NOT shared** (each diverges per
app): IPC fan-out, the config watcher, the per-app window-state filename hash, the apps' own menu
*items* (only the spine above is shared), and the chrome-caller command gate (curator-only — warden
hosts no content webviews at all, and lector's remote-origin content webviews are denied by Tauri's
own IPC dispatch without needing an explicit gate).

## How it's consumed

Each app pins shell-core by git rev, using the light default features for `build.rs` and the
`runtime` feature for the app. Under Cargo's resolver 2 the two resolve features independently, so
the build-dependency never pulls in tauri:

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

and the app commits a three-line `scripts/tooling.env` (see `scripts/tooling.env.example`).

## Development

```
just gate       # pre-merge: fmt check + clippy + tests
just test       # the test suite
just zero-dep   # prove the default feature set still pulls no dependencies
```

`menu` and `home` are gated behind the `runtime` feature, so a bare `cargo test` compiles neither
and passes having checked almost nothing — the recipes pass `--features runtime` for you. The
`rust-toolchain.toml` pin makes those checks reproducible; it governs standalone development only,
since each consuming app compiles shell-core with its own pin.

## License

MIT © Lachlan Collins
