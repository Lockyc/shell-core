<h1 align="center">shell-core</h1>

<p align="center">The shared Tauri app-shell + release tooling for <a href="https://github.com/Lockyc/curator">curator</a> and <a href="https://github.com/Lockyc/warden">warden</a> — one release pipeline, many apps.</p>

curator (browser keeper-tabs) and warden (terminals) are sibling macOS Tauri apps. They already
share a **view** ([chrome-core](https://github.com/Lockyc/chrome-core)) and **config primitives**
([config-core](https://github.com/Lockyc/config-core)). `shell-core` is the third shared layer: the
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
- **Runtime (`runtime` feature).** `register_plugins()` installs the plugins every app registers
  identically: window-state, updater, process.

## Status

New. Extracted from warden and curator's copy-pasted tooling; both consume it pinned to a `0.1.x`
rev. **Deliberately NOT shared** (each diverges per app): IPC fan-out, the config watcher, menu
construction, and the chrome-caller command gate (curator-only — warden hosts no untrusted webviews).

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

## License

MIT © Lachlan Collins
