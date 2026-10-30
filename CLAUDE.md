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

Two rules span every module below:

- **No core→core edge.** shell-core never depends on config-core (or chrome-core). The three cores
  are a flat fan-out, so a config-core bump never forces a shell-core rev and each core stays
  independently patchable by the `*-dev`/`*-pin` loop. Where a module needs a config value, the app
  bridges it to a plain type (`cycle_digits: bool`, a raw sRGB tuple, the app's own
  `write_default_config` call).
- **shell-core owns the surface; the app wires the actions.** The window shell, label scheme, menu
  items and pages live here; what an action *does* to the app's content or state (creating a config,
  re-parenting a docked tab, origin bookkeeping, placing items in its own submenus) lives in the
  app's closures and event handlers. shell-core stores no app state.

**In** — each module's docblock is the home for its mechanism and footguns; read it before editing:

- **`scripts/`** — the release, install/launch and core-patch scripts (embedded + materialized,
  below). Each script's header carries its steps and footguns (`release.sh`'s ordering,
  `launch-app.sh`'s `env -i`, `gen-latest-json.sh`'s two platform keys). The `<core>-dev` /
  `<core>-pin` recipe names wrapping `core-patch.sh` are the interface fleet tooling calls.
- **`build_stamp()`** — git sha/date → `BUILD_GIT_SHA`/`BUILD_DATE` for the About box.
- **`tabs::pick_live_neighbour`** (default surface) — the tab-unload policy, every app.
- **`register_plugins()`** (`runtime`) — geometry, updater and process plugins plus the home and
  detach custom protocols, for every app.
  - **Deferred: drop `register_plugins`' `skip_labels` parameter.** No consumer uses it (every app
    passes `&[]`), so it and its plumbing are dead: `geometry::plugin`'s `skip_labels`,
    `GeometryState.skip`, the `state.skip.contains` clause in `geometry::is_excluded`, and the
    skip-list half of its test. Removing it changes `register_plugins`' signature, which forces a
    re-pin of every app. **Unlock:** the next shell-core bump that is already
    API-breaking for every app. Ride along with it; never cut a bump for this alone. **Next
    action:** delete the parameter and the plumbing above, drop the argument at each app's call
    site in that same lockstep re-pin, and remove the `skip_labels` mentions here and in the
    README.
- **`geometry`** (`runtime`) — per-window size/position persistence, every app; the app hands it
  only a resolved config path. `src/geometry.rs`.
- **`compositing`** (`runtime`) — the hole-punch webview layout, curator, lector + flotilla. warden composites a
  native `NSView` through its own `geometry.rs` — genuine divergence, left alone.
- **`watch::watch_config`** (`runtime`) — config hot-reload, curator, lector + flotilla. warden keeps its own
  watcher: it parses inside and drives a main-thread reconcile, a different contract.
- **`mouse_nav::install`** / **`progress_bar::install`** (`runtime`, macOS) — side-button navigation
  and the loading bar for content WKWebViews, curator + lector; flotilla takes `mouse_nav` (warden
  hosts native terminals).
- **`web`** (`runtime`) — the content-webview layer: `escape` (link routing, escape sentinel),
  `session` (`data_store_id` over an app-supplied seed; the default session name stays in the
  app's config) and `insecure` (allowlisted self-signed TLS, macOS). **`data_store_id` names the
  WebKit store holding every login** — a change to it logs everyone out; its parity test pins
  curator's ids. `content` builds every content webview from a spec plus app hooks. **A content
  webview is only built or destroyed through `web::content`** (`create_content_webview`,
  `close_content_webview`, `close_window_pages`): a bare `close()` leaks the running page, and a
  hand-built webview lacks the key-verified sentinel handling, the inspector watcher
  (`web::inspector`: an attached Web Inspector is forced into its own window and the hole
  re-applied, so it never covers the sidebar), `web::pointer` (emits `pointer-away` to the
  window's chrome when the pointer enters content, so the sidebar's `:hover` can't freeze; each
  app's chrome page maps it to chrome-core's `pointerAway()`) and `web::ipc_global` (wry's
  `window.ipc` made replaceable before the first load, which is why every content webview is born
  on `about:blank`). lector, which hand-builds its local-server webviews, calls `web::pointer::watch`
  itself.
- **`menu`** (`runtime`) — the App/Config/Window/Edit spine, the whole Tab submenu and `route`, all
  apps. The Tab submenu's order and the popped-out routing rule live here; each app supplies its own
  item groups, event names, origin lookup and `selectByOffset` cycle predicate (required
  divergence). The close/pop-out/find accelerators are constants here, the family
  standard's one home.
- **`home`** (`runtime`) — the surface an app shows instead of being stranded windowless, every
  app. `src/home.rs` has why it is served over its own scheme.
- **`windows`** (`runtime`) — `plan` (pure: labels pinned to live ones, title-matched reconcile, menu
  entries), `WindowSet` (open/focus, close-keeps-listed, opt-in last-close quit, in-place rename,
  delete, home) over the app's `WindowHost`, `form` + `flow` (the New/Edit/Delete form and its menu
  ids; the app only writes the `WindowWrite` to its config), `open_url` (`<scheme>://open?window=…` parsing; `WindowSet::last_focused` routes a link naming no window). Apps map their config to `WindowDecl`;
  no config-core edge. The `{window}:{within}` child-label scheme is `label` (zero-dep), including
  `rehome` for a child moving to another window label. Origin bookkeeping (which window a
  popped-out tab returns to) stays per app; it finds that window by title (`entry_for_title`).
- **`detach`** (`runtime`) — a popped-out tab's window, every app. **Invariant spanning
  `detach`, `geometry` and every consumer: the `token` passed to `detached_label` must derive from a
  durable tab identity**, because geometry persists a detached window keyed on that label; each
  consumer pins its derivation with its own test. `src/detach.rs` and `src/detach.html` carry the
  rest.

**Out — and why (do not "consolidate" these; the divergence is real):**
- **IPC fan-out** — curator centralizes `emit_to_*chrome` helpers with plain event names; warden
  inlines `emit_to` at each site with app-namespaced events (`warden:refresh`) + a `forMe()` filter.
  Different structures; a shared helper would fight both. Which window a menu command targets is
  shared (`menu::route`); the emit itself stays each app's.
- **warden's native compositing + tab registry** — its `NSView` hole-punch (`geometry.rs`) and its
  `TabSlot` state machine over the `TerminalSurface` trait are a different beast from curator/lector's
  webview registries; genuinely per-app.

## The command-isolation security model — single-sourced here

This is the shared reasoning for *why a Tauri app command needs (or doesn't need) a per-caller
gate*. It lived only in lector's `commands.rs` header; it belongs here so a future sibling app finds
it instead of copying curator's redundant gate. **Verified against the pinned `tauri = 2.11.5`
vendored source.**

- **A crate's own `#[tauri::command]`s are gated by *origin*, not by a hand-rolled label check** —
  *given the app ships no app-command ACL manifest*. Dispatch (`webview/mod.rs:on_message`) only
  requires a resolved ACL when `has_app_acl_manifest || !is_local`. The sidebar chrome is the
  window's main webview loaded from `frontendDist` (`tauri://…`) → `Origin::Local`, so with no app
  manifest its invokes pass unconditionally; a content webview loading `http://127.0.0.1:{port}/` or
  an `External` page is `Origin::Remote`, so `!is_local` is true and the invoke is **rejected before
  any command body runs**, gate or no gate.
- **So a `require_chrome`/`is_chrome_caller` label gate is redundant against the *remote-content*
  threat** — Tauri's origin dispatch already covers it. The one thing a label gate *uniquely* covers
  is a **second *local* surface** (a home/detach page, or any Builder-registered custom protocol,
  all `Origin::Local`) that hosts *untrusted* content — origin dispatch is local-vs-remote and won't
  screen one local surface from another.
- **The gate is not curator-specific.** lector hosts remote `127.0.0.1` content too (same threat
  shape as curator), and origin dispatch — not any label gate — is what isolates it. shell-core's own
  home + detach surfaces are exactly the second-*local*-surface case, but they serve **fixed,
  shell-core-bundled** HTML (no untrusted content), so they need no gate. A future third local surface
  hosting anything user-supplied would need an explicit gate.
- **curator's `require_chrome` is therefore redundant belt-and-braces** (curator meets the
  precondition exactly: tauri 2.11.5, and its capability grants only `core:*`/`updater`/`process` —
  zero app-command permissions). It can stay as defense-in-depth or be narrowed to a
  local-surface-only screen — a **security-sensitive judgment call left to the maintainer**, not a
  mechanical cleanup.
- **This bypass does NOT extend to core *plugin* commands** (`core:event`, `core:window`, updater,
  process) — those are gated by their own default-denied permission set regardless of the app
  manifest, so each app still ships a `capabilities/*.json` granting the sidebar exactly the plugin
  permissions it uses. (Footgun: a missing grant makes `event.listen`/window-drag silently no-op —
  the rejection comes from the plugin's ACL, not the crate-command dispatch path above.)

## The embed-and-materialize pattern (the tooling seam)

The scripts are the source of truth **here** in `scripts/`. `src/lib.rs` embeds each via
`include_str!`. A consumer's
`build.rs` calls `materialize_scripts(<its scripts dir>)`, which writes them out **git-ignored** — so a plain clone
rebuilds them from the pinned rev and there is no second tracked copy to drift. This mirrors
chrome-core's CSS/JS embed exactly. A fresh clone therefore has none of them until its first
build; the apps' `core-patch.sh` wrappers (gate guard included) refuse with "run `cargo check`
first" rather than skip.

- **Edit scripts HERE, never in a consuming app** — the app's copy is generated and git-ignored;
  an edit there is silently overwritten on the next build.
- **The scripts are generic — no app name may appear in them.** Every app-specific value is read from
  the consumer's tracked `scripts/tooling.env` (keys: `scripts/tooling.env.example`);
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
