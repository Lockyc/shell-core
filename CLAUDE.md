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
- The release/deploy scripts (`scripts/release.sh`, `gen-latest-json.sh`, `install-app.sh`,
  `launch-app.sh`) — generic, param-driven, embedded + materialized (below). `install-app.sh` and
  `launch-app.sh` are a deliberate pair: installing never launches ("the caller decides"), and
  launching is its own script because **`open` forwards the caller's whole environment to the app**,
  so a bare `open` in a deploy recipe runs the app in a terminal's environment that no real launch
  ever reproduces. `launch-app.sh` wraps it in `env -i` to get the launchd GUI-session environment
  instead. Its header carries the full footgun — don't re-derive it in a consuming app's justfile.
  `release.sh` builds `--target universal-apple-darwin`, so every consuming app's release artifact
  runs on both Apple Silicon and Intel regardless of which Mac cut it, and `gen-latest-json.sh`
  emits **both** `darwin-aarch64` and `darwin-x86_64` pointing at that one tarball.
  - **Footgun: keep both platform keys.** The updater matches on the running install's *own* arch
    and finds nothing when its key is absent — which surfaces as auto-update going **silently
    quiet** (no error, no update bar, no log) for every user on the missing architecture, so the
    breakage is invisible from the machine that cut the release. Both entries carry the same
    signature because they name the same universal tarball; that is not redundancy to collapse.
  - Release-only: `just build`/`just deploy`/`install.sh` stay host-arch, which is all a local
    install needs and is half the compile.
- `build_stamp()` — git sha/date → `BUILD_GIT_SHA`/`BUILD_DATE` for the About box.
- **The shared tab-unload policy** (`tabs::pick_live_neighbour`, default surface — no `runtime`
  feature). One decision every app makes identically: which sibling tab becomes active after a
  tab is unloaded. Pure index logic, zero-dependency, window-agnostic — the loaded/unloaded state
  it's handed is each app's own concern. It lives here rather than in chrome-core's JS because
  warden auto-unloads a tab straight from Rust (`handle_child_exited`, on child-process exit), with
  no chrome round-trip to hook the decision into.
- `register_plugins()` (`runtime` feature) — installs the three plugins every app registers the
  same way: [`geometry`](below), the updater, and the process plugin (for the updater's relaunch) —
  plus the home and detach surfaces' custom protocols. Given an app's resolved config path
  (`Option<&Path>`) it hands `geometry` the derived per-config filename; `None` is a deliberate,
  documented fallback to an unscoped `.window-geometry.json` for an app with no per-config state to
  scope, not a gap to close.
- **`geometry`** (`runtime` feature) — per-window size/position persistence. It owns all of it:
  point-based storage, the fullscreen/minimized recording guard, the target-monitor clamp on
  restore, and the structural exclusion of the home surface (for save as well as restore).
  **A detached-tab window is persisted, not excluded** — its label is deterministic per tab, so a
  popped-out tab reopens at the size and position it was last left at; see `detach`'s label scheme
  below. Nothing stays per-app beyond handing it a
  resolved config path: the canonicalize→hash→format filename step
  (`.window-geometry-{fnv1a_64(canonicalize(path)):016x}.json`, `geometry_filename`) lives here
  once, since it was byte-identical across all three apps' old per-app copies. Uses its own
  `fnv1a_64` — a spec-defined, test-vector-pinned primitive, **not** a shadow of the config crates'
  own `fnv1a_64` (that hashes window titles/tab dirs for *label identity*, a separate domain that
  stays in each config crate).
  - **Footgun that survives: don't reintroduce physical-pixel persistence.** Persisted geometry is
    always AppKit points, and every restore is clamped to the target monitor's work area. Physical
    pixels are points × the occupied screen's scale factor, so a value saved on one display and
    applied against another of a different scale factor is wrong by that ratio — this is the
    reason `geometry` reads and writes only points and never a native pixel size.
  - **Footgun that survives:** the hash drives a *persistent on-disk filename*, so it must use a
    **fixed** algorithm (`fnv1a_64`), never `std::hash::DefaultHasher` — its output isn't guaranteed
    stable across Rust releases, so a toolchain bump silently changes the filename and resets every
    window's saved bounds ("the app forgot my layout"). The pinned test vectors here are what keep
    it fixed. Don't swap the algorithm.
  - **Footgun that survives: `restore` moves the window, sizes it, then moves it again — never
    just size-then-move.** A restored window still sits at its tao creation-time position (none of
    the three apps pass a starting position) when the fitted size would otherwise be applied, and
    `NSWindow.setContentSize:` runs `constrainFrameRect:toScreen:` against whatever screen the
    window currently occupies — not the one it's headed to. Sizing before moving lets AppKit
    silently constrain the rect against the wrong monitor (e.g. shrinking a large-external-display
    rect to fit the built-in display), and the resulting `Resized` event then caches that shrunken
    size, making the corruption stick through the next quit. Don't collapse this back to two calls.
  - **Footgun that survives: the fullscreen/minimized guard in `snapshot` must fail *closed*.**
    `is_fullscreen()`/`is_minimized()` default to `true` on a query error, not `false` — skipping a
    snapshot on error costs a slightly stale rect, while recording one while genuinely fullscreen
    persists the tile geometry this guard exists to prevent. Sequoia's drag-to-edge window
    *tiling* is a separate thing from this guard and is **not** covered by it: tiling isn't a
    fullscreen space (classic Split View is), so a tiled window's ordinary bounds are correctly
    still recorded; only a genuine fullscreen/split-view space is skipped.
- **`compositing`** (`runtime` feature) — the hole-punch content-webview placement (`HoleRect`,
  `CHROME_W`, `initial_hole`, `layout_webviews`) shared byte-identically by curator + lector: the
  sidebar chrome is the window's main webview, and `add_child` content webviews are positioned to
  fill the reported hole. warden is **not** a consumer — it composites a native `NSView` through its
  own `geometry.rs` (Y-flip + HiDPI), genuine divergence left alone.
- **`watch`** (`runtime` feature) — the config-file hot-reload watcher for curator + lector
  (`watch_config`): parent-dir watch (atomic-save-robust), **file-name** event match (macOS
  FSEvents-robust — the fix for the exact-path bug curator + lector both shipped), and an echo-swallow
  seam that stays config-agnostic (the app's `on_change` returns `Some(bytes)` on a format-write, and
  shell-core swallows the echo — it never parses, so no `config-core` edge). warden keeps its own
  watcher: it parses inside + drives a slow root-scan/main-thread reconcile and relies on
  `format_file`'s diff-guard, a genuinely different contract (and the file-name-match reference this
  was modelled on).
- **`mouse_nav::install`** (`runtime` feature, macOS) — native mouse side-button (back/forward)
  navigation for content webviews, shared by curator + lector (warden hosts native terminals with no
  page history, doesn't use it). A local `NSEvent` monitor decodes both delivery paths — a plain
  mouse's `otherMouseDown` buttons 3/4, **and** a driver's `systemDefined` subtype-7
  `NX_SUBTYPE_AUX_MOUSE_BUTTONS` events (`data1` = changed-button mask, `data2` = currently-down
  mask; bit 3 = back, bit 4 = forward) — and drives the focused tab's WKWebView history natively
  (`goBack`/`goForward`). The core owns the monitor + decode + native call; the **app** supplies a
  `Fn() -> Option<Webview>` resolver returning its focused window's active content tab (the one piece
  that differs per app). **Footgun:** WKWebView never forwards the side buttons to the DOM, so a
  page-level JS `mouseup` handler can't see them — native monitoring is the only path that works, so
  don't reintroduce an injected-JS approach. Two more silent traps the monitor encodes: keep
  the value `addLocalMonitorForEventsMatchingMask:handler:` returns alive (dropping the `Retained`
  tears the monitor down instantly), and act only on the *press* transition so each press navigates
  once. Deps (`objc2`/`objc2-app-kit`/`block2`) are optional + macOS-target, pulled only by
  `runtime`, so the zero-tauri build-dep never compiles them.
- **`progress_bar::install`** (`runtime` feature, macOS) — the content-webview loading bar, shared by
  curator + lector (warden: native terminals, no WKWebView). A thin layer-backed `NSView` pinned to
  the top of each content WKWebView, driven by `estimatedProgress` on view-owned block `NSTimer`s
  (fills left→right, alpha-decay fade at 100%). The core owns the view + timers; the **app**
  passes the bar colour as raw sRGB `(r,g,b,a)` (**not** a `config_core::Colour` — keeps config-core
  out of shell-core, the three-cores rule) and calls `install` once per content webview at creation.
  **Footgun — poll, not KVO:** observing `estimatedProgress` by KVO crashes if the webview deallocs
  while still observed (tab unload/recreate) and wry exposes no webview-close hook to remove the
  observer first; the timers self-clean instead, invalidating when `superview()` goes nil (the blocks
  are the bar's only other strong owners). Shares mouse_nav's optional + macOS-target objc2 deps (plus
  `objc2-foundation` for `NSTimer`/geometry).
  **Footgun — two timers, and never one always-on fast one.** A permanent *watcher* at
  `IDLE_TICK_SECS` (tolerance 50%, so macOS can coalesce the wakeup) does nothing but spot
  `estimatedProgress` dropping below 1.0; only then does it schedule the *animator* at
  `ACTIVE_TICK_SECS`, which invalidates itself once the fade completes. Collapsing them back into one
  always-on ~30 Hz timer costs more in `mk_timer_arm` (the kernel *re-arming* it) than in the
  callback, and pays it per content webview for the app's whole life rather than per load — measured
  on curator with 5 live tabs it was 85% of all main-thread work while the app sat idle in the
  background, and it defeats timer coalescing and deep idle, which is what Energy Impact scores
  hardest.
- **The menu spine** (`menu::build_spine`, `runtime` feature) — the App submenu (About +
  Check for Updates…), the Config submenu (Edit Config / Reveal in Finder), and the Window submenu
  (minimize/maximize/fullscreen, Close Window, and a checked per-window selector). Returns the
  submenus for the app to place among its own; it does not set the menu.
  - **Spine in, items out.** The spine — App/Config/Window submenus — is identical for any app
    regardless of what it hosts, so it lives here once. The *items* stay per-app: curator's Reload
    Tab / Reset All / DevTools and warden's Reopen Last Closed are genuinely different menus, not
    one menu with parameters, and are not parameterisable. (The tab-nav block below is the
    exception — it *is* parameterised, by `cycle_digits`.) The Config submenu is byte-identical
    across apps modulo the config path, so it's part of the shared spine.
  - Parameterised only where the apps genuinely differ: the app name, the config path, and the
    window list. The Window items take the checked/`(closed)` shape that shows per-window state.
  - **The close accelerators are constants here, not parameters: ⌘W closes a tab, ⌘⇧W the window.**
    That is the family standard, and this is the one place it lives — one home is what keeps it from
    drifting. Every app has an `unload_tab` meaning the same thing (unload the active tab to cold; it
    respawns on next select), so nothing app-specific remains. `Spine::close_tab` is returned as a bare item because
    every app's tab submenu differs (curator's Reload/Reset/DevTools, warden's Reopen Last Closed
    spliced into the Window submenu) — the item is shared, its placement is the app's.
  - **Check for Updates… is a menu item here, not update logic.** chrome-core owns self-update (its
    dividing-line exemplar); the app forwards the event to `checkForUpdateNow()`. `handle_spine_event`
    handles only the two config ids, which act on a file and need no window.
  - **The tab-nav block** (`menu::build_tab_nav`, `TabNav`, `TabNavAction`, `tab_nav_action`) —
    Previous/Next Tab (`⌘⇧[`/`⌘⇧]`), the `⌘1`–`⌘9` jump-to-position items, and the `⌘1`/`⌘2` cycle
    aliases — is shared too, not per-app: one implementation of the digit mode, with every id and
    accelerator pinned by test (`nav_spec`/`jump_spec` are plain-data functions, unit-tested without
    a Tauri app; `build_tab_nav` only maps that data through `MenuItemBuilder`). What stays per app
    is the **submenu composition** these two blocks are placed into — curator's also carries
    Reload Tab / Reset All / DevTools, lector's is bare, warden splices Reopen Last Closed into the
    Window submenu — that is required divergence, not drift, so it is not pulled in here. The
    `selectByOffset` cycle *predicate* each chrome uses stays out of this block too — the *items*
    are shared, the *predicate* is the app's call. warden and curator both skip cold tabs
    (`liveOnly: true`): cycling is for moving between the tabs you already have open, so it must
    never be what loads one — holding the chord through a mostly-cold window would otherwise spawn
    a PTY (warden) or a webview (curator) per step. lector still cycles every tab
    (`liveOnly: false`). Jumps are unfiltered everywhere: naming a position is an explicit request
    to load.
    `build_tab_nav` takes a plain `cycle_digits: bool`, **never** config-core's `TabDigitKeys`:
    shell-core must not depend on config-core (the cores stay mutually independent so each is
    independently patchable), so the consuming app bridges with
    `cfg.tab_digit_keys.is_cycle()` before calling in.
- **The home surface** (`home::{home_state, show_home, close_home}`, `runtime` feature) — what an
  app shows when it would otherwise have no window, so it is **never stranded invisible**. Three
  states: no config (offer to write one), a load error (show it), or a valid config's window list
  (warden's launcher, generalised). `home_state` is a pure function of those inputs and is tested
  as one.
  - **shell-core owns the surface; the app wires the actions.** "Create a starter config" is the
    app's command calling `config_core::write_default_config` with its own template. shell-core
    must **never** depend on config-core: the three cores are a flat fan-out, and a core→core edge
    would let a config-core bump force a shell-core rev and break the `*-dev`/`*-pin` loop's
    assumption that each core is independently patchable.
  - **`HOME_LABEL` is excluded from geometry persistence structurally, inside `geometry::is_excluded`
    — never via `register_plugins`' `skip_labels`.** All three apps pass `&[]` there; a caller
    listing it would be redundant at best.
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
- **The detach surface** (`detach::{DETACH_LABEL_PREFIX, detached_label, is_detached_label,
  detach_token, DetachSpec, open_detached, wire_return}`, `runtime` feature) — the "pop a tab out
  into its own temporary window" lifecycle, consumed by warden/curator/lector alike. It owns two
  things:
  - **A reserved label scheme.** `DETACH_LABEL_PREFIX = "shell-detach:"` + `detached_label(token)`
    build a window label for a popped-out tab; `is_detached_label`/`detach_token` are the inverse.
    A consumer's hot-reload reconcile calls `is_detached_label` itself to skip these windows.
    - **The `token` a consumer passes is load-bearing beyond uniqueness — it must derive from a
      durable tab identity.** `geometry` persists a detached window's size and position keyed on
      the resulting label; that is *why* a popped-out tab reopens at the shape you left it. A token
      built from a per-session counter or allocation order would silently forget the geometry every
      launch and orphan an entry each time. Every consumer already derives it from a stable
      identity (warden hashes `origin_label:tab_key`; curator and lector hash the tab's own label),
      and each pins that determinism with its own test.
    - **`DetachSpec`'s `width`/`height` are the first-pop-out default only** — restore overwrites
      them inside `open_detached`'s `build()` for any tab popped out before. So `birth_content` is
      handed the window's **real** logical inner size as its second argument, and a consumer sizes
      its docked content from that. All three consumers computed a birth rect from the constants
      before this, and all three were wrong the moment geometry started being remembered; handing
      the fact over beats documenting the trap, since the stale value is now the one you'd have to
      reach past. (The cost of getting it wrong is bounded — one frame, until `detach.html`'s
      `set_hole_rect` reports the true hole — which is also why a failed geometry query falls back
      to `spec`'s size rather than failing the pop-out.)
  - **A banner-only shell page** (`detach.html` — title + origin accent, no sidebar), served over
    its own custom protocol `DETACH_SCHEME`, registered on the `Builder` by
    `register_detach_protocol` (chained into `register_plugins` alongside `home`'s). Same reasoning
    as `HOME_SCHEME` above: a Builder-registered protocol is `Origin::Local`, so the detached
    window's commands need no capability wiring beyond what a consumer already ships. The page
    reports its content-hole rect via each app's existing `set_hole_rect` command — nothing new
    there either. `open_detached(app, token, spec: &DetachSpec, app_name, birth_content)` builds
    the window (a `WebviewWindowBuilder` primary webview, mirroring `show_home`'s construction
    exactly — never `WindowBuilder` + `add_child`, per the macOS-26 `close_home` bug above), injects
    `window.__SHELL_DETACH__`, then hands the built window to `birth_content` to dock the app's
    real content into (a failed dock closes the window and propagates the error, so no
    banner-only orphan is left behind). `wire_return(app, label, on_close)` installs the `Destroyed`
    handler that fires the app's return orchestration — it resolves the window by label via
    `get_window` **itself** (never `get_webview_window`), so a caller can't pick the wrong lookup.
    That matters: an app that docks its content in as an *added child webview* (curator/lector)
    makes a **multi-webview** window, which Tauri exposes only under `get_window`/`windows` — a
    `get_webview_window` lookup returns `None`, silently skipping the wiring so the tab could never
    redock (its origin row stayed stuck on the pop-in affordance). `get_window` is correct for that
    case and for warden's single-webview native-surface window alike.
  - **Dividing line — the same split as `home`: shell-core owns the surface, the app wires the
    actions.** Here that means the window shell, the label convention, and the close trigger live
    here; moving the tab's actual content (warden re-parents a native surface, curator/lector
    recreate a webview) and *all* origin bookkeeping — which window/tab a detached window came from,
    reopening/redocking on return, rebuilding the menu — lives entirely in the app's
    `birth_content`/`on_close` closures. shell-core stores no origin state of its own.
  - **The menu spine's Pop Out Tab item** (`menu::ids::POP_OUT_TAB`, `⌘⇧O` /
    `menu::ACCEL_POP_OUT_TAB`) is built alongside Close Tab in `build_spine` and returned
    (`Spine::pop_out_tab`) for the app to place in its own tab submenu, for the identical reason
    `close_tab` is returned rather than handled: it needs the focused window, which only the app
    can resolve. The spine builds the item; it does not act on it.

**Out — and why (do not "consolidate" these; the divergence is real):**
- **IPC fan-out** — curator centralizes `emit_to_*chrome` helpers with plain event names; warden
  inlines `emit_to` at each site with app-namespaced events (`warden:refresh`) + a `forMe()` filter.
  Different structures; a shared helper would fight both. (curator + lector do share the tiny
  `emit_to_focused_chrome` helper by copy — lifting it would force a `serde` dep on shell-core to
  name the `Serialize` bound, not worth it for a 5-line non-drift helper.)
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
  mechanical cleanup (see the lift-plan's security section).
- **This bypass does NOT extend to core *plugin* commands** (`core:event`, `core:window`, updater,
  process) — those are gated by their own default-denied permission set regardless of the app
  manifest, so each app still ships a `capabilities/*.json` granting the sidebar exactly the plugin
  permissions it uses. (Footgun: a missing grant makes `event.listen`/window-drag silently no-op —
  the rejection comes from the plugin's ACL, not the crate-command dispatch path above.)

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
