//! The family's shared menus: the App, Config, Window and Edit submenus, the whole Tab submenu,
//! and [`route`], which decides which window a menu command acts on. Each app builds only its own
//! items (curator's Reload Tab, warden's terminal semantics), which genuinely cannot be
//! parameterised: it hands them to [`build_tab_menu`] as groups, or places them in its own menus.

use std::path::Path;

/// Menu-item ids the spine owns. Namespaced `shell:` so they can never collide with an app's own.
pub mod ids {
    pub const CHECK_UPDATES: &str = "shell:check_updates";
    pub const EDIT_CONFIG: &str = "shell:edit_config";
    pub const REVEAL_CONFIG: &str = "shell:reveal_config";
    pub const CLOSE_TAB: &str = "shell:close_tab";
    pub const POP_OUT_TAB: &str = "shell:pop_out_tab";
    pub const FIND_IN_SIDEBAR: &str = "shell:find_in_sidebar";
    pub const CLOSE_WINDOW: &str = "shell:close_window";
    pub const OPEN_WINDOW_PREFIX: &str = "shell:open_window:";
    pub const TAB_PREV: &str = "shell:tab_prev";
    pub const TAB_NEXT: &str = "shell:tab_next";
    /// ⌘1 / ⌘2 aliases for Next / Previous Tab, built only in cycle mode.
    pub const TAB_NEXT_DIGIT: &str = "shell:tab_next_digit";
    pub const TAB_PREV_DIGIT: &str = "shell:tab_prev_digit";
    /// Jump-to-position items: `shell:tab_jump:<1-based position>`.
    pub const TAB_JUMP_PREFIX: &str = "shell:tab_jump:";
    /// The Cycle Tabs with ⌘1/⌘2 check item. The app writes the flipped `tab_digit_keys`.
    pub const TAB_DIGIT_CYCLE: &str = "shell:tab_digit_cycle";
}

/// The family's close accelerators.
///
/// **⌘W closes a tab; ⌘⇧W closes the window.** Constants, not parameters: this is one convention
/// for every app in the family, and one place is what stops a per-app copy drifting. Every app
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

/// The family's Find in Sidebar accelerator. ⌘⇧F, not ⌘F: the menu gets first refusal on key
/// equivalents, so ⌘F here would take find-in-page from curator's and lector's content and
/// find-in-terminal from warden's.
pub const ACCEL_FIND_IN_SIDEBAR: &str = "Shift+Cmd+KeyF";

/// The family's tab-cycling accelerators — ⌘⇧[ / ⌘⇧] , the browser convention. Constants for the
/// same reason as the close accelerators: one convention, one place, no per-app copy to drift.
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
            spawn_reaped(std::process::Command::new("open").arg(config_path));
            true
        }
        ids::REVEAL_CONFIG => {
            spawn_reaped(
                std::process::Command::new("open")
                    .arg("-R")
                    .arg(config_path),
            );
            true
        }
        _ => false,
    }
}

/// Spawn `cmd` without blocking the caller, and wait on it from a throwaway thread. Dropping a
/// `Child` never reaps it, so an unwaited `open` would linger as a zombie for the app's life.
fn spawn_reaped(cmd: &mut std::process::Command) {
    if let Ok(mut child) = cmd.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
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
            label: "Next Tab".to_string(),
            accel: "Cmd+Digit1".to_string(),
        });
        v.push(ItemSpec {
            id: ids::TAB_PREV_DIGIT.to_string(),
            label: "Previous Tab".to_string(),
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

/// The Cycle Tabs toggle's title.
const TOGGLE_LABEL: &str = "Cycle Tabs with ⌘1/⌘2";

/// Whether the Cycle Tabs toggle is ticked: exactly when ⌘1/⌘2 cycle.
fn toggle_checked(cycle_digits: bool) -> bool {
    cycle_digits
}

/// One position in the Tab submenu. Kept as data so the menu's order — the one thing every app
/// shares — is unit-testable without a Tauri app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// `nav_spec`'s item at this index.
    Nav(usize),
    Toggle,
    Sep,
    CloseTab,
    PopOutTab,
    FindInSidebar,
    /// The app's group at this index.
    Group(usize),
    /// Every `jump_spec` item.
    Jumps,
}

/// The Tab submenu's order: navigation and the toggle, the spine's tab items, each app group,
/// then the jumps.
fn tab_menu_spec(cycle_digits: bool, n_groups: usize) -> Vec<Slot> {
    let mut v: Vec<Slot> = (0..nav_spec(cycle_digits).len()).map(Slot::Nav).collect();
    v.extend([
        Slot::Toggle,
        Slot::Sep,
        Slot::CloseTab,
        Slot::PopOutTab,
        Slot::FindInSidebar,
    ]);
    for g in 0..n_groups {
        v.extend([Slot::Sep, Slot::Group(g)]);
    }
    v.extend([Slot::Sep, Slot::Jumps]);
    v
}

/// Build the whole Tab submenu: shell-core owns its shape, the app supplies only its own item
/// groups (warden's Split and session items, curator's Reload Tab…), each set off by a separator.
///
/// `cycle_digits` is a plain bool, NOT config-core's `TabDigitKeys`: shell-core must never depend
/// on config-core (the cores stay mutually independent, so each is independently patchable), so
/// the consuming app bridges with `cfg.tab_digit_keys.is_cycle()`.
pub fn build_tab_menu<R: tauri::Runtime, M: tauri::Manager<R>>(
    manager: &M,
    spine: &Spine<R>,
    cycle_digits: bool,
    app_groups: &[&[&dyn IsMenuItem<R>]],
) -> tauri::Result<Submenu<R>> {
    let item = |s: &ItemSpec| {
        MenuItemBuilder::with_id(s.id.clone(), s.label.clone())
            .accelerator(s.accel.clone())
            .build(manager)
    };
    let nav = nav_spec(cycle_digits)
        .iter()
        .map(item)
        .collect::<tauri::Result<Vec<_>>>()?;
    let jumps = jump_spec(cycle_digits)
        .iter()
        .map(item)
        .collect::<tauri::Result<Vec<_>>>()?;
    let toggle = CheckMenuItemBuilder::with_id(ids::TAB_DIGIT_CYCLE, TOGGLE_LABEL)
        .checked(toggle_checked(cycle_digits))
        .build(manager)?;

    let mut menu = SubmenuBuilder::new(manager, "Tab");
    for slot in tab_menu_spec(cycle_digits, app_groups.len()) {
        menu = match slot {
            Slot::Nav(i) => menu.item(&nav[i]),
            Slot::Toggle => menu.item(&toggle),
            Slot::Sep => menu.separator(),
            Slot::CloseTab => menu.item(&spine.close_tab),
            Slot::PopOutTab => menu.item(&spine.pop_out_tab),
            Slot::FindInSidebar => menu.item(&spine.find_in_sidebar),
            Slot::Group(g) => menu.items(app_groups[g]),
            Slot::Jumps => jumps.iter().fold(menu, |m, j| m.item(j)),
        };
    }
    menu.build()
}

/// What a tab-nav menu id means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabNavAction {
    Next,
    Prev,
    /// Jump to this 1-based tab position.
    Jump(usize),
}

/// Route a menu id to its tab-nav action, or `None` for any other id. Both ⌘1/⌘2 alias ids
/// collapse onto `Next`/`Prev`, so an app's handler never learns the digit mode exists.
fn tab_nav_action(id: &str) -> Option<TabNavAction> {
    match id {
        ids::TAB_NEXT | ids::TAB_NEXT_DIGIT => Some(TabNavAction::Next),
        ids::TAB_PREV | ids::TAB_PREV_DIGIT => Some(TabNavAction::Prev),
        _ => id
            .strip_prefix(ids::TAB_JUMP_PREFIX)
            .and_then(|n| n.parse::<usize>().ok())
            .map(TabNavAction::Jump),
    }
}

/// A spine command, as [`route`] resolved it for the window it acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpineAction {
    Nav(TabNavAction),
    /// ⌘W in a main window: unload the active tab.
    CloseTab,
    /// ⌘W in a popped-out window: return its tab to the window it came from.
    DockPoppedOut,
    PopOutTab,
    FindInSidebar,
    CheckUpdates,
    CloseWindow,
}

/// Where a spine command acts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routed {
    pub action: SpineAction,
    /// The window label to act on.
    pub target: String,
    /// Set when the command was fired from a popped-out window and redirected to `target`, its
    /// origin: the app raises `target`, and Next/Previous step from this popped-out tab.
    pub from_detached: Option<String>,
}

/// Which window a spine command acts on, given the focused window's label.
///
/// One rule for every app. A popped-out window has no sidebar, so ⌘W docks its tab, Pop Out Tab
/// does nothing, and the sidebar commands (tab navigation, Find in Sidebar, Check for Updates) go
/// to the window it came from. The home surface has no tabs: only Close Window acts there. The
/// app answers `origin_of(detached_label)` from its own bookkeeping (shell-core stores no origin
/// state); `None` drops the command. Edit/Reveal Config and the toggle are not routed: they act on
/// the config file, not a window.
pub fn route(
    id: &str,
    focused: &str,
    origin_of: impl Fn(&str) -> Option<String>,
) -> Option<Routed> {
    let action = match id {
        ids::CLOSE_TAB => SpineAction::CloseTab,
        ids::POP_OUT_TAB => SpineAction::PopOutTab,
        ids::FIND_IN_SIDEBAR => SpineAction::FindInSidebar,
        ids::CHECK_UPDATES => SpineAction::CheckUpdates,
        ids::CLOSE_WINDOW => SpineAction::CloseWindow,
        _ => SpineAction::Nav(tab_nav_action(id)?),
    };
    let here = |action| {
        Some(Routed {
            action,
            target: focused.to_string(),
            from_detached: None,
        })
    };
    if action == SpineAction::CloseWindow {
        return here(action);
    }
    if focused == crate::home::HOME_LABEL {
        return None;
    }
    if crate::detach::is_detached_label(focused) {
        return match action {
            SpineAction::CloseTab => here(SpineAction::DockPoppedOut),
            SpineAction::PopOutTab => None,
            action => Some(Routed {
                action,
                target: origin_of(focused)?,
                from_detached: Some(focused.to_string()),
            }),
        };
    }
    here(action)
}

use tauri::menu::{
    AboutMetadataBuilder, CheckMenuItemBuilder, IsMenuItem, MenuItem, MenuItemBuilder, Submenu,
    SubmenuBuilder,
};

/// What `build_spine` hands back: the shared submenus, plus the Close Tab, Pop Out Tab and Find in
/// Sidebar items [`build_tab_menu`] places (an app may also reuse one, as flotilla's Group menu
/// retitles Close Tab).
pub struct Spine<R: tauri::Runtime> {
    /// App, Config, Window — in that order.
    pub submenus: Vec<Submenu<R>>,
    /// Undo/Redo, Cut/Copy/Paste/Select All. Without an Edit menu the clipboard chords do nothing
    /// in a webview, so an app places it unless something else owns those chords.
    pub edit: Submenu<R>,
    pub close_tab: MenuItem<R>,
    pub pop_out_tab: MenuItem<R>,
    pub find_in_sidebar: MenuItem<R>,
}

/// Build the App, Config, Window and Edit submenus plus the tab items `Spine` names. Returns them
/// for the app to place among its own — this does NOT set the menu, mirroring how
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

    // Same rationale again; the app forwards it to its sidebar's `focusSearch()`.
    let find_in_sidebar = MenuItemBuilder::with_id(ids::FIND_IN_SIDEBAR, "Find in Sidebar")
        .accelerator(ACCEL_FIND_IN_SIDEBAR)
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

    let edit = SubmenuBuilder::new(manager, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    Ok(Spine {
        submenus: vec![app_menu, config_menu, window_menu],
        edit,
        close_tab,
        pop_out_tab,
        find_in_sidebar,
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
        // standard lives.
        assert_eq!(ACCEL_CLOSE_TAB, "Cmd+KeyW");
        assert_eq!(ACCEL_CLOSE_WINDOW, "Shift+Cmd+KeyW");
    }

    #[test]
    fn pop_out_accelerator_is_the_family_standard() {
        assert_eq!(ACCEL_POP_OUT_TAB, "Shift+Cmd+KeyO");
    }

    #[test]
    fn find_in_sidebar_accelerator_is_the_family_standard_and_unclaimed() {
        assert_eq!(ACCEL_FIND_IN_SIDEBAR, "Shift+Cmd+KeyF");
        let others = [
            ACCEL_CLOSE_TAB,
            ACCEL_CLOSE_WINDOW,
            ACCEL_POP_OUT_TAB,
            ACCEL_TAB_PREV,
            ACCEL_TAB_NEXT,
        ];
        assert!(!others.contains(&ACCEL_FIND_IN_SIDEBAR));
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

    #[test]
    fn tab_menu_order() {
        use Slot::*;
        assert_eq!(
            tab_menu_spec(false, 0),
            [
                Nav(0),
                Nav(1),
                Toggle,
                Sep,
                CloseTab,
                PopOutTab,
                FindInSidebar,
                Sep,
                Jumps
            ]
        );
        assert_eq!(
            tab_menu_spec(true, 2),
            [
                Nav(0),
                Nav(1),
                Nav(2),
                Nav(3),
                Toggle,
                Sep,
                CloseTab,
                PopOutTab,
                FindInSidebar,
                Sep,
                Group(0),
                Sep,
                Group(1),
                Sep,
                Jumps
            ]
        );
    }

    #[test]
    fn cycle_aliases_are_plainly_labelled() {
        let nav = nav_spec(true);
        assert_eq!(nav[2].label, "Next Tab");
        assert_eq!(nav[3].label, "Previous Tab");
    }

    #[test]
    fn the_toggle_is_ticked_in_cycle_mode() {
        assert!(!toggle_checked(false));
        assert!(toggle_checked(true));
        assert_eq!(TOGGLE_LABEL, "Cycle Tabs with ⌘1/⌘2");
        assert_eq!(ids::TAB_DIGIT_CYCLE, "shell:tab_digit_cycle");
    }

    const SPINE_IDS: [&str; 7] = [
        ids::TAB_NEXT,
        "shell:tab_jump:4",
        ids::CLOSE_TAB,
        ids::POP_OUT_TAB,
        ids::FIND_IN_SIDEBAR,
        ids::CHECK_UPDATES,
        ids::CLOSE_WINDOW,
    ];

    fn origin(label: &str) -> Option<String> {
        (label == "shell-detach:x").then(|| "w1".to_string())
    }

    #[test]
    fn route_in_a_main_window_targets_it() {
        for id in SPINE_IDS {
            let r = route(id, "w1", origin).unwrap_or_else(|| panic!("{id}"));
            assert_eq!((r.target.as_str(), r.from_detached), ("w1", None), "{id}");
        }
        assert_eq!(
            route(ids::CLOSE_TAB, "w1", origin).unwrap().action,
            SpineAction::CloseTab
        );
        assert_eq!(
            route(ids::TAB_PREV, "w1", origin).unwrap().action,
            SpineAction::Nav(TabNavAction::Prev)
        );
    }

    #[test]
    fn route_in_a_popped_out_window() {
        let pop = "shell-detach:x";
        let r = route(ids::CLOSE_TAB, pop, origin).unwrap();
        assert_eq!(
            (r.action, r.target.as_str()),
            (SpineAction::DockPoppedOut, pop)
        );
        assert!(route(ids::POP_OUT_TAB, pop, origin).is_none());
        for id in [
            ids::TAB_NEXT,
            "shell:tab_jump:4",
            ids::FIND_IN_SIDEBAR,
            ids::CHECK_UPDATES,
        ] {
            let r = route(id, pop, origin).unwrap();
            assert_eq!(r.target, "w1", "{id}");
            assert_eq!(r.from_detached.as_deref(), Some(pop), "{id}");
        }
        let r = route(ids::CLOSE_WINDOW, pop, origin).unwrap();
        assert_eq!(
            (r.action, r.target.as_str()),
            (SpineAction::CloseWindow, pop)
        );
    }

    #[test]
    fn route_with_an_unknown_origin() {
        let pop = "shell-detach:gone";
        assert!(route(ids::TAB_NEXT, pop, origin).is_none());
        assert!(route(ids::CHECK_UPDATES, pop, origin).is_none());
        assert_eq!(
            route(ids::CLOSE_TAB, pop, origin).unwrap().action,
            SpineAction::DockPoppedOut
        );
    }

    #[test]
    fn route_on_the_home_surface() {
        for id in SPINE_IDS {
            let r = route(id, crate::home::HOME_LABEL, origin);
            assert_eq!(r.is_some(), id == ids::CLOSE_WINDOW, "{id}");
        }
    }

    #[test]
    fn route_ignores_other_ids() {
        for id in [
            ids::EDIT_CONFIG,
            ids::REVEAL_CONFIG,
            ids::TAB_DIGIT_CYCLE,
            "app:x",
        ] {
            assert!(route(id, "w1", origin).is_none(), "{id}");
        }
    }

    #[test]
    fn nav_and_jump_accelerators_never_collide() {
        // jump_mode_gives_every_digit_a_tab_position and cycle_mode_takes_digits_1_and_2_from_the_jumps
        // each pin one block; this pins the combined set both blocks actually produce, so two menu
        // items can never claim the same chord.
        for cycle in [false, true] {
            let nav = nav_spec(cycle);
            let jumps = jump_spec(cycle);
            let accels: Vec<&str> = nav
                .iter()
                .chain(jumps.iter())
                .map(|s| s.accel.as_str())
                .collect();
            let mut deduped = accels.clone();
            deduped.sort_unstable();
            deduped.dedup();
            assert_eq!(
                deduped.len(),
                accels.len(),
                "duplicate accelerator across nav_spec/jump_spec in cycle={cycle} mode: {accels:?}"
            );
        }
    }
}
