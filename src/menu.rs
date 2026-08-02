//! The app-agnostic menu spine: the App, Config, and Window submenus that are identical for any
//! app in the family regardless of what it hosts. Each app builds its own items (curator's Reload
//! Tab, warden's terminal semantics) and interleaves them with these.
//!
//! This is a shared spine plus per-app items — **not** one menu with parameters. The distinction
//! matters: the app-specific items genuinely cannot be parameterised, which is why they stay put.

use std::path::Path;

/// Menu-item ids the spine owns. Namespaced `shell:` so they can never collide with an app's own.
pub mod ids {
    pub const CHECK_UPDATES: &str = "shell:check_updates";
    pub const EDIT_CONFIG: &str = "shell:edit_config";
    pub const REVEAL_CONFIG: &str = "shell:reveal_config";
    pub const CLOSE_TAB: &str = "shell:close_tab";
    pub const POP_OUT_TAB: &str = "shell:pop_out_tab";
    pub const CLOSE_WINDOW: &str = "shell:close_window";
    pub const OPEN_WINDOW_PREFIX: &str = "shell:open_window:";
    pub const TAB_PREV: &str = "shell:tab_prev";
    pub const TAB_NEXT: &str = "shell:tab_next";
    /// ⌘1 / ⌘2 aliases for Next / Previous Tab, built only in cycle mode.
    pub const TAB_NEXT_DIGIT: &str = "shell:tab_next_digit";
    pub const TAB_PREV_DIGIT: &str = "shell:tab_prev_digit";
    /// Jump-to-position items: `shell:tab_jump:<1-based position>`.
    pub const TAB_JUMP_PREFIX: &str = "shell:tab_jump:";
}

/// The family's close accelerators.
///
/// **⌘W closes a tab; ⌘⇧W closes the window.** Constants, not parameters: this is one convention
/// for every app in the family, and one place is what stops it drifting — curator's ⌘W had drifted
/// onto Close Window, which is precisely what a per-app copy of a convention buys you. Every app
/// has an `unload_tab` and all three mean the same by it (unload the active tab to cold; it
/// respawns on next select), so there is nothing app-specific left to parameterise.
pub const ACCEL_CLOSE_TAB: &str = "Cmd+KeyW";
pub const ACCEL_CLOSE_WINDOW: &str = "Shift+Cmd+KeyW";

/// The family's Pop Out Tab accelerator. ⌘⇧O ("Out") — clear of every other menu accelerator in
/// the family. In warden it must also not collide with libghostty's built-in tab chords, and there
/// it can't: warden gives its menu first refusal on `performKeyEquivalent:`, so a menu accelerator
/// wins over any colliding terminal keybind (curator and lector embed no terminal, so the question
/// doesn't arise for them).
pub const ACCEL_POP_OUT_TAB: &str = "Shift+Cmd+KeyO";

/// The family's tab-cycling accelerators — ⌘⇧[ / ⌘⇧] , the browser convention. Constants for the
/// same reason as the close accelerators: one convention, one place, no per-app copy to drift
/// (warden and curator had already drifted to different spellings of the same chord).
pub const ACCEL_TAB_PREV: &str = "Shift+Cmd+BracketLeft";
pub const ACCEL_TAB_NEXT: &str = "Shift+Cmd+BracketRight";

/// One configured window, for the Window submenu's selector and the home surface's list.
#[derive(Debug, Clone)]
pub struct WindowEntry {
    /// The app's own window id/label — round-tripped through the menu id, opaque here.
    pub id: String,
    pub title: String,
    /// Whether it is currently open (checked + plainly titled) or closed (labelled "(closed)").
    pub open: bool,
    /// The window's accent colour, for the home surface's swatch. `None` = neutral. The menu
    /// ignores it (a macOS menu item carries no swatch); `home.rs` renders it.
    pub colour: Option<String>,
}

/// What an app tells the spine about itself.
pub struct SpineConfig<'a> {
    pub app_name: &'a str,
    pub config_path: &'a Path,
    pub windows: &'a [WindowEntry],
}

fn open_window_id(window_id: &str) -> String {
    format!("{}{window_id}", ids::OPEN_WINDOW_PREFIX)
}

fn window_id_from(menu_id: &str) -> Option<&str> {
    menu_id.strip_prefix(ids::OPEN_WINDOW_PREFIX)
}

fn window_item_label(title: &str, open: bool) -> String {
    if open {
        title.to_string()
    } else {
        format!("{title}  (closed)")
    }
}

/// Handle the spine's file-acting ids (Edit Config, Reveal Config) — they need no window, so an
/// app can call this before its own focused-window lookup. Returns whether it consumed the event.
///
/// `CHECK_UPDATES` is deliberately NOT handled: chrome-core owns self-update (its dividing-line
/// exemplar), so the app forwards that event to its chrome's `checkForUpdateNow()`. The spine only
/// builds the item.
pub fn handle_spine_event(id: &str, config_path: &Path) -> bool {
    match id {
        ids::EDIT_CONFIG => {
            let _ = std::process::Command::new("open").arg(config_path).spawn();
            true
        }
        ids::REVEAL_CONFIG => {
            let _ = std::process::Command::new("open")
                .arg("-R")
                .arg(config_path)
                .spawn();
            true
        }
        _ => false,
    }
}

/// One tab-nav menu item, resolved from the digit mode. Kept as *data* so the mode's entire
/// effect — which ids exist, which chord each claims — is unit-testable without a Tauri app.
struct ItemSpec {
    id: String,
    label: String,
    accel: String,
}

/// Previous/Next Tab, plus the ⌘1/⌘2 aliases in cycle mode. A menu item carries exactly one
/// accelerator, which is why cycle mode needs distinct alias items firing the same action rather
/// than a second chord on the ⌘⇧[ / ⌘⇧] items.
fn nav_spec(cycle_digits: bool) -> Vec<ItemSpec> {
    let mut v = vec![
        ItemSpec {
            id: ids::TAB_PREV.to_string(),
            label: "Previous Tab".to_string(),
            accel: ACCEL_TAB_PREV.to_string(),
        },
        ItemSpec {
            id: ids::TAB_NEXT.to_string(),
            label: "Next Tab".to_string(),
            accel: ACCEL_TAB_NEXT.to_string(),
        },
    ];
    if cycle_digits {
        v.push(ItemSpec {
            id: ids::TAB_NEXT_DIGIT.to_string(),
            label: "Next Tab (⌘1)".to_string(),
            accel: "Cmd+Digit1".to_string(),
        });
        v.push(ItemSpec {
            id: ids::TAB_PREV_DIGIT.to_string(),
            label: "Previous Tab (⌘2)".to_string(),
            accel: "Cmd+Digit2".to_string(),
        });
    }
    v
}

/// Jump-to-position items. ⌘1–⌘9 normally; ⌘3–⌘9 when the cycle aliases took 1 and 2 (positions
/// 1–2 then have no direct chord — that is the trade the mode makes).
fn jump_spec(cycle_digits: bool) -> Vec<ItemSpec> {
    let first = if cycle_digits { 3 } else { 1 };
    (first..=9)
        .map(|n| ItemSpec {
            id: format!("{}{n}", ids::TAB_JUMP_PREFIX),
            label: format!("Tab {n}"),
            accel: format!("Cmd+Digit{n}"),
        })
        .collect()
}

/// The tab-navigation items, in the two blocks an app places in its own tab submenu.
///
/// Two blocks rather than a built submenu because each app's tab submenu genuinely differs —
/// curator's also carries Reload Tab / Reset All Tabs / Open Developer Tools, lector's carries
/// neither, warden splices Reopen Last Closed into the Window submenu. Ordering *within* a block
/// is fixed here; composition stays the app's.
pub struct TabNav<R: tauri::Runtime> {
    /// Previous Tab, Next Tab — plus the ⌘1/⌘2 aliases when `cycle_digits`.
    pub nav: Vec<MenuItem<R>>,
    /// Jump-to-position: ⌘1–⌘9, or ⌘3–⌘9 when the aliases took 1 and 2.
    pub jumps: Vec<MenuItem<R>>,
}

/// Build the tab-navigation items for the given digit mode.
///
/// `cycle_digits` is a plain bool, NOT config-core's `TabDigitKeys`: shell-core must never depend
/// on config-core (the cores stay mutually independent, so each is independently patchable), so
/// the consuming app bridges with `cfg.tab_digit_keys.is_cycle()`.
pub fn build_tab_nav<R: tauri::Runtime, M: tauri::Manager<R>>(
    manager: &M,
    cycle_digits: bool,
) -> tauri::Result<TabNav<R>> {
    fn build<R: tauri::Runtime, M: tauri::Manager<R>>(
        manager: &M,
        specs: Vec<ItemSpec>,
    ) -> tauri::Result<Vec<MenuItem<R>>> {
        specs
            .into_iter()
            .map(|s| {
                MenuItemBuilder::with_id(s.id, s.label)
                    .accelerator(s.accel)
                    .build(manager)
            })
            .collect()
    }
    Ok(TabNav {
        nav: build(manager, nav_spec(cycle_digits))?,
        jumps: build(manager, jump_spec(cycle_digits))?,
    })
}

/// What a tab-nav menu id means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabNavAction {
    Next,
    Prev,
    /// Jump to this 1-based tab position.
    Jump(usize),
}

/// Route a menu id to its tab-nav action, or `None` for any other id.
///
/// Both ⌘1/⌘2 alias ids collapse onto `Next`/`Prev`, so an app's menu handler never learns the
/// digit mode exists — the mode is entirely `build_tab_nav`'s business.
pub fn tab_nav_action(id: &str) -> Option<TabNavAction> {
    match id {
        ids::TAB_NEXT | ids::TAB_NEXT_DIGIT => Some(TabNavAction::Next),
        ids::TAB_PREV | ids::TAB_PREV_DIGIT => Some(TabNavAction::Prev),
        _ => id
            .strip_prefix(ids::TAB_JUMP_PREFIX)
            .and_then(|n| n.parse::<usize>().ok())
            .map(TabNavAction::Jump),
    }
}

use tauri::menu::{
    AboutMetadataBuilder, CheckMenuItemBuilder, MenuItem, MenuItemBuilder, Submenu, SubmenuBuilder,
};

/// What `build_spine` hands back: the shared submenus, plus the Close Tab and Pop Out Tab items for
/// the app to place in its own tab submenu (every app's differs).
pub struct Spine<R: tauri::Runtime> {
    pub submenus: Vec<Submenu<R>>,
    pub close_tab: MenuItem<R>,
    pub pop_out_tab: MenuItem<R>,
}

/// Build the App, Config, and Window submenus plus the Close Tab and Pop Out Tab items. Returns
/// them for the app to place among its own — this does NOT set the menu, mirroring how
/// `register_plugins` returns the `Builder` for continued chaining.
///
/// The About box carries the app's version plus the `build_stamp()` sha/date, so a glance confirms
/// the installed app matches a given commit.
pub fn build_spine<R: tauri::Runtime, M: tauri::Manager<R>>(
    manager: &M,
    cfg: SpineConfig<'_>,
    version: &str,
    build_sha: &str,
    build_date: &str,
) -> tauri::Result<Spine<R>> {
    let about = AboutMetadataBuilder::new()
        .name(Some(cfg.app_name))
        .version(Some(version))
        .short_version(Some(build_sha))
        .comments(Some(format!("commit {build_sha} · built {build_date}")))
        .build();

    let check_updates =
        MenuItemBuilder::with_id(ids::CHECK_UPDATES, "Check for Updates…").build(manager)?;
    let app_menu = SubmenuBuilder::new(manager, cfg.app_name)
        .about(Some(about))
        .separator()
        .item(&check_updates)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let edit_cfg = MenuItemBuilder::with_id(ids::EDIT_CONFIG, "Edit Config").build(manager)?;
    let reveal_cfg =
        MenuItemBuilder::with_id(ids::REVEAL_CONFIG, "Reveal Config in Finder").build(manager)?;
    let config_menu = SubmenuBuilder::new(manager, "Config")
        .items(&[&edit_cfg, &reveal_cfg])
        .build()?;

    // ⌘W closes a TAB, ⌘⇧W the window — the family standard, in one place. Returned for the app's
    // own tab submenu; built here so the id and accelerator can't drift per app.
    let close_tab = MenuItemBuilder::with_id(ids::CLOSE_TAB, "Close Tab")
        .accelerator(ACCEL_CLOSE_TAB)
        .build(manager)?;

    // Same rationale as close_tab: returned for the app's own tab submenu, built here so the id
    // and accelerator can't drift per app.
    let pop_out_tab = MenuItemBuilder::with_id(ids::POP_OUT_TAB, "Pop Out Tab")
        .accelerator(ACCEL_POP_OUT_TAB)
        .build(manager)?;

    let close_window = MenuItemBuilder::with_id(ids::CLOSE_WINDOW, "Close Window")
        .accelerator(ACCEL_CLOSE_WINDOW)
        .build(manager)?;
    let mut window_menu = SubmenuBuilder::new(manager, "Window")
        .minimize()
        .maximize()
        .fullscreen()
        .separator()
        .item(&close_window)
        .separator();
    // Built up-front so the `&` refs outlive the chained `.item()` calls.
    let entries = cfg
        .windows
        .iter()
        .map(|e| {
            CheckMenuItemBuilder::with_id(
                open_window_id(&e.id),
                window_item_label(&e.title, e.open),
            )
            .checked(e.open)
            .build(manager)
        })
        .collect::<Result<Vec<_>, _>>()?;
    for it in &entries {
        window_menu = window_menu.item(it);
    }
    let window_menu = window_menu.build()?;

    Ok(Spine {
        submenus: vec![app_menu, config_menu, window_menu],
        close_tab,
        pop_out_tab,
    })
}

/// The window id behind an `open_window` menu id, or `None` for any other id. Public so an app's
/// handler can route the selector without knowing the prefix.
pub fn selected_window(menu_id: &str) -> Option<&str> {
    window_id_from(menu_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_window_ids_round_trip() {
        let id = open_window_id("w1234");
        assert_eq!(id, "shell:open_window:w1234");
        assert_eq!(window_id_from(&id), Some("w1234"));
        assert_eq!(window_id_from("shell:edit_config"), None);
    }

    #[test]
    fn closed_windows_are_labelled_as_such() {
        // warden's shape: an open window is checked and plainly titled; a closed one says so, so
        // the menu shows state rather than just listing names (curator's plain items don't).
        assert_eq!(window_item_label("Docs", true), "Docs");
        assert_eq!(window_item_label("Docs", false), "Docs  (closed)");
    }

    #[test]
    fn spine_consumes_only_its_file_acting_ids() {
        // Check for Updates is deliberately NOT handled here: chrome-core owns self-update, and
        // the app forwards the event to its chrome. Close Tab/Window need the focused window,
        // which only the app can resolve. The spine builds those items; it doesn't act on them.
        let p = std::path::Path::new("/tmp/does-not-matter.toml");
        assert!(!handle_spine_event(ids::CHECK_UPDATES, p));
        assert!(!handle_spine_event(ids::CLOSE_TAB, p));
        assert!(!handle_spine_event(ids::CLOSE_WINDOW, p));
        assert!(!handle_spine_event("app:something_else", p));
    }

    #[test]
    fn the_close_accelerators_are_the_family_standard() {
        // ⌘W closes a TAB; ⌘⇧W closes the window. Pinned here because this is the one place the
        // standard lives — curator's ⌘W had drifted onto Close Window, which is the bug that
        // proved a per-app copy of this convention can't hold.
        assert_eq!(ACCEL_CLOSE_TAB, "Cmd+KeyW");
        assert_eq!(ACCEL_CLOSE_WINDOW, "Shift+Cmd+KeyW");
    }

    #[test]
    fn pop_out_accelerator_is_the_family_standard() {
        assert_eq!(ACCEL_POP_OUT_TAB, "Shift+Cmd+KeyO");
    }

    #[test]
    fn jump_mode_gives_every_digit_a_tab_position() {
        let nav = nav_spec(false);
        assert_eq!(nav.len(), 2);
        assert_eq!(nav[0].id, ids::TAB_PREV);
        assert_eq!(nav[0].accel, "Shift+Cmd+BracketLeft");
        assert_eq!(nav[1].id, ids::TAB_NEXT);
        assert_eq!(nav[1].accel, "Shift+Cmd+BracketRight");
        let jumps = jump_spec(false);
        assert_eq!(jumps.len(), 9);
        assert_eq!(jumps[0].id, "shell:tab_jump:1");
        assert_eq!(jumps[0].label, "Tab 1");
        assert_eq!(jumps[0].accel, "Cmd+Digit1");
        assert_eq!(jumps[8].accel, "Cmd+Digit9");
    }

    #[test]
    fn cycle_mode_takes_digits_1_and_2_from_the_jumps() {
        // A menu item carries exactly ONE accelerator, so cycle mode needs distinct alias items
        // rather than a second chord on Previous/Next Tab.
        let nav = nav_spec(true);
        assert_eq!(nav.len(), 4);
        assert_eq!(nav[2].id, ids::TAB_NEXT_DIGIT);
        assert_eq!(nav[2].accel, "Cmd+Digit1");
        assert_eq!(nav[3].id, ids::TAB_PREV_DIGIT);
        assert_eq!(nav[3].accel, "Cmd+Digit2");
        let jumps = jump_spec(true);
        assert_eq!(jumps.len(), 7);
        assert_eq!(jumps[0].label, "Tab 3");
        assert_eq!(jumps[0].accel, "Cmd+Digit3");
        // No jump may claim a chord the aliases took — that would be a duplicate accelerator.
        assert!(jumps
            .iter()
            .all(|j| j.accel != "Cmd+Digit1" && j.accel != "Cmd+Digit2"));
    }

    #[test]
    fn tab_nav_action_routes_every_built_id_and_hides_the_mode() {
        // Whatever the mode builds must route; an app's handler never learns the mode exists.
        for cycle in [false, true] {
            for s in nav_spec(cycle).iter().chain(jump_spec(cycle).iter()) {
                assert!(tab_nav_action(&s.id).is_some(), "unrouted id {}", s.id);
            }
        }
        assert_eq!(
            tab_nav_action(ids::TAB_NEXT_DIGIT),
            Some(TabNavAction::Next)
        );
        assert_eq!(
            tab_nav_action(ids::TAB_PREV_DIGIT),
            Some(TabNavAction::Prev)
        );
        assert_eq!(tab_nav_action(ids::TAB_NEXT), Some(TabNavAction::Next));
        assert_eq!(tab_nav_action(ids::TAB_PREV), Some(TabNavAction::Prev));
        assert_eq!(
            tab_nav_action("shell:tab_jump:7"),
            Some(TabNavAction::Jump(7))
        );
        // Foreign ids are left for the app / the rest of the spine.
        assert!(tab_nav_action(ids::CLOSE_TAB).is_none());
        assert!(tab_nav_action("shell:tab_jump:x").is_none());
        assert!(tab_nav_action("app:whatever").is_none());
    }

    #[test]
    fn the_tab_nav_accelerators_are_the_family_standard() {
        assert_eq!(ACCEL_TAB_PREV, "Shift+Cmd+BracketLeft");
        assert_eq!(ACCEL_TAB_NEXT, "Shift+Cmd+BracketRight");
    }
}
