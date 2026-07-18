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
/// the family and of libghostty's built-in tab chords; a menu accelerator wins over any colliding
/// terminal keybind regardless (each app gives its menu first refusal on `performKeyEquivalent:`).
pub const ACCEL_POP_OUT_TAB: &str = "Shift+Cmd+KeyO";

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

use tauri::menu::{
    AboutMetadataBuilder, CheckMenuItemBuilder, MenuItem, MenuItemBuilder, Submenu, SubmenuBuilder,
};

/// What `build_spine` hands back: the shared submenus, plus the Close Tab item for the app to
/// place in its own tab submenu (every app's differs).
pub struct Spine<R: tauri::Runtime> {
    pub submenus: Vec<Submenu<R>>,
    pub close_tab: MenuItem<R>,
    pub pop_out_tab: MenuItem<R>,
}

/// Build the App, Config, and Window submenus plus the Close Tab item. Returns them for the app to
/// place among its own — this does NOT set the menu, mirroring how `register_plugins` returns the
/// `Builder` for continued chaining.
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
}
