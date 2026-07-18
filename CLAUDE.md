---
type: architecture
links:
  - rel: see-also
    to: README.md
---

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
- **The shared tab-unload policy** (`tabs::pick_live_neighbour`, default surface — no `runtime`
  feature). One decision every app makes identically: which sibling tab becomes active after a
  tab is unloaded. Pure index logic, zero-dependency, window-agnostic — the loaded/unloaded state
  it's handed is each app's own concern. It lives here rather than in chrome-core's JS because
  warden auto-unloads a tab straight from Rust (`handle_child_exited`, on child-process exit), with
  no chrome round-trip to hook the decision into.
- `register_plugins()` (`runtime` feature) — window-state + updater + process, the three plugins
  every app registers the same way.
- **The menu spine** (`menu::build_spine`, `runtime` feature) — the App submenu (About +
  Check for Updates…), the Config submenu (Edit Config / Reveal in Finder), and the Window submenu
  (minimize/maximize/fullscreen, Close Window, and a checked per-window selector). Returns the
  submenus for the app to place among its own; it does not set the menu.
  - **This revises an earlier decision, and half of it stands.** The old line ruled menus out
    entirely: *"warden's terminal tab semantics vs curator's webview Edit/clipboard menu are
    genuinely different menus, not one menu with parameters."* That is **still true of the items** —
    curator's Reload Tab / Reset All / DevTools and warden's tab semantics stay per-app and are not
    parameterisable. It was drawn too coarsely: it swept the *spine* out with them, and the spine is
    identical for any app regardless of what it hosts. Two consumers could not expose the
    distinction; a third did — lector had no menu at all, and the Config submenu turned out to be
    byte-identical in both apps modulo the config path. **Spine in, items out.**
  - Parameterised only where the apps genuinely differ: the app name, the config path, and the
    window list. The Window items take warden's checked/`(closed)` shape — it shows state; curator's
    plain items didn't.
  - **The close accelerators are constants here, not parameters: ⌘W closes a tab, ⌘⇧W the window.**
    That is the family standard, and this is the one place it lives. An earlier draft made the
    close-window accelerator a parameter to accommodate curator's ⌘W-closes-the-window — which would
    have encoded a bug as a requirement. Every app has an `unload_tab` meaning the same thing
    (unload the active tab to cold; it respawns on next select), so nothing app-specific remains.
    curator's ⌘W had drifted precisely because each app kept its own copy of the convention; one
    place is what stops it drifting again. `Spine::close_tab` is returned as a bare item because
    every app's tab submenu differs (warden's Jump/Cycle digit modes, curator's Reload/Reset/DevTools,
    lector's empty one) — the item is shared, its placement is the app's.
  - **Check for Updates… is a menu item here, not update logic.** chrome-core owns self-update (its
    dividing-line exemplar); the app forwards the event to `checkForUpdateNow()`. `handle_spine_event`
    handles only the two config ids, which act on a file and need no window.
- **The home surface** (`home::{home_state, show_home, close_home}`, `runtime` feature) — what an
  app shows when it would otherwise have no window, so it is **never stranded invisible**. Three
  states: no config (offer to write one), a load error (show it), or a valid config's window list
  (warden's launcher, generalised). `home_state` is a pure function of those inputs and is tested
  as one.
  - It replaces **curator's error window** and **warden's launcher** — two half-implementations of
    one idea. curator's stated an error and offered no action; warden's offered windows and could
    not express an error. Neither could do the other's job, and lector had neither, so a fresh
    install launched to nothing at all.
  - **shell-core owns the surface; the app wires the actions.** "Create a starter config" is the
    app's command calling `config_core::write_default_config` with its own template. shell-core
    must **never** depend on config-core: the three cores are a flat fan-out, and a core→core edge
    would let a config-core bump force a shell-core rev and break the `*-dev`/`*-pin` loop's
    assumption that each core is independently patchable.
  - Pass `HOME_LABEL` in `register_plugins`' `skip_labels`, or its throwaway bounds get persisted.
  - **The page is served over its own custom URI scheme (`home::HOME_SCHEME`, registered by
    `register_plugins` via `home::register_protocol`), never `WebviewUrl::App`.** This is
    deliberate, not a shortcut: Tauri's ACL engine classifies a webview as `Origin::Local` or
    `Origin::Remote` from its navigated URL, and only `Local` matches a capability entry that
    carries no `remote` block — which is what every consumer's existing capabilities file is
    shaped like (see lector's capabilities footgun). `WebviewUrl::App` would need `home.html`
    materialized into each consumer's own `frontendDist` (an extra per-app build step this task
    doesn't otherwise need); a `data:` URL is `Origin::Remote` and has no stable `remote.urls`
    pattern to grant against, which would silently break the page's `invoke()` calls. A
    Builder-registered custom protocol is the one option that is both self-contained (no consumer
    build.rs change) and `Origin::Local` (works with each app's capabilities file unchanged).
  - **Needs the `unstable` Cargo feature on `tauri`** — for `Manager::get_window` (`show_home` and
    `close_home` look up an already-open home window). **Not** for the window's construction: that
    is `WebviewWindowBuilder`, which is stable — the `WindowBuilder` + `add_child` shape that once
    needed `unstable` is gone (see `close_home`'s doc). `unstable` is already on by every consumer's
    own `tauri` dependency, since curator's and lector's content webviews `add_child` a webview per
    open tab, so this costs nothing new downstream.

**Out — and why (do not "consolidate" these; the divergence is real):**
- **IPC fan-out** — curator centralizes `emit_to_*chrome` helpers with plain event names; warden
  inlines `emit_to` at each site with app-namespaced events (`warden:refresh`) + a `forMe()` filter.
  Different structures; a shared helper would fight both.
- **The config watcher** — diverged in shape between the apps.
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
