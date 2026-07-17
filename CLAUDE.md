# shell-core — agent guide

The third shared layer behind **curator**, **warden**, and **lector**, alongside
`chrome-core` (the sidebar view) and `config-core` (config primitives). shell-core owns the
**build/release tooling** and the sliver of **Tauri runtime setup** that is byte-identical across
those apps. Consumed by **git-rev pin** (never a path dep in git); shell-core is **main-only** — a
library gated by nothing but its consumers' pins, so there's no release cadence to run here.

## The dividing line — what lives here, and what deliberately does NOT

shell-core is the home for app-agnostic **shell + tooling**: anything identical for any such app
regardless of what it hosts. It is NOT a place to abstract things that merely *look* similar.

**In:**
- The three release scripts (`scripts/release.sh`, `gen-latest-json.sh`, `install-app.sh`) — generic,
  param-driven, embedded + materialized (below).
- `build_stamp()` — git sha/date → `BUILD_GIT_SHA`/`BUILD_DATE` for the About box.
- `register_plugins()` (`runtime` feature) — window-state + updater + process, the three plugins
  every app registers the same way.

**Out — and why (do not "consolidate" these; the divergence is real):**
- **IPC fan-out** — curator centralizes `emit_to_*chrome` helpers with plain event names; warden
  inlines `emit_to` at each site with app-namespaced events (`warden:refresh`) + a `forMe()` filter.
  Different structures; a shared helper would fight both.
- **The config watcher** — diverged in shape between the apps.
- **Menu construction** — warden's terminal tab semantics vs curator's webview Edit/clipboard menu
  are genuinely different menus, not one menu with parameters.
- **The chrome-caller command gate (`is_chrome_caller`) — curator-only.** curator hosts arbitrary web
  content in sibling webviews, so it must reject commands from non-chrome callers. warden's surfaces
  are native NSViews — it has no untrusted webview to spoof a call — so it has no such gate and never
  needs one. Sharing it would push dead, misleading security code into warden.
- **`window_state_filename()` stays per-app** — each app hashes its own config path. It is passed
  *into* `register_plugins`, not owned here. **Do not move the hash here** to deduplicate it — the
  ~8-line `fnv1a_64` each app carries is the deliberate cost of that boundary.
  - **Footgun every consumer must keep honouring:** the hash drives a *persistent on-disk filename*,
    so it must use a **fixed** algorithm — `fnv1a_64`, pinned by a known-vectors test in each app.
    Never `std::hash::DefaultHasher`: its output is **not** guaranteed stable across Rust releases, so
    a toolchain bump silently changes the filename and resets every window's saved bounds. It reads
    as "the app forgot my layout", never as a toolchain problem. (curator shipped this bug — fixed
    2026-07-16; warden was always correct. A new sibling app copying curator's shape must copy the
    *fixed* version — lector does: its single `fnv1a_64` lives in its **config crate**
    (`lector-config/src/hash.rs`), not duplicated into `src-tauri` the way curator's is. The
    dividing line above rules the hash out of shell-core; it does not mandate a per-app duplicate,
    and lector's placement is a valid alternative to curator's.)

## The embed-and-materialize pattern (the tooling seam)

The scripts are the source of truth **here** in `scripts/`. `src/lib.rs` embeds each via
`include_str!` (`RELEASE_SH`/`GEN_LATEST_SH`/`INSTALL_APP_SH`). A consumer's `build.rs` calls
`materialize_scripts(<its scripts dir>)`, which writes them out **git-ignored** — so a plain clone
rebuilds them from the pinned rev and there is no second tracked copy to drift. This mirrors
chrome-core's CSS/JS embed exactly.

- **Edit scripts HERE, never in a consuming app** — the app's copy is generated and git-ignored;
  an edit there is silently overwritten on the next build.
- **The scripts are generic — no app name may appear in them.** Every app-specific value is read from
  the consumer's tracked `scripts/tooling.env` (`APP_NAME`, `TAURI_CRATE_DIR`, `UPDATER_REPO`);
  everything else derives (`VERSION_FILE=${TAURI_CRATE_DIR}/Cargo.toml`, bundle `${APP_NAME}.app`,
  zip/tarball names, URLs from `${UPDATER_REPO}`). The `tests/scripts.rs` guard fails the build if
  `warden`/`curator`/`lector` leaks into a script or a script stops sourcing `tooling.env`.

## The zero-dep/runtime feature split (why it exists)

`build.rs` needs only `build_stamp()` + the script consts — all zero-dependency. `register_plugins`
needs tauri. If they shared one always-on dependency set, every consumer's `[build-dependencies]`
would drag in the whole tauri tree. So the default feature set is zero-dep and the tauri helper sits
behind `runtime`. **Load-bearing:** consumers must set `default-features = false` on the
`[build-dependencies]` entry and `features = ["runtime"]` on the `[dependencies]` entry. Resolver 2
resolves the two independently, so the build-dep compiles without tauri (verify: a default
`cargo build` compiles only `shell-core`, no tauri crates).

## Versioning / releases

`version` in `Cargo.toml` is the single source of truth. Bump it when the shared surface changes,
then bump each consumer's pinned `rev` (in lockstep with the plugin/toolchain versions). No GitHub
release cadence — consumers pin by rev, not by tag.
