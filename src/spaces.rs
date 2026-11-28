//! A new window opens on an ordinary desktop, never on top of the app's own fullscreen space.
//!
//! tao builds every window with `NSWindowCollectionBehaviorDefault`. When one of the app's windows
//! is fullscreen (or a Split View tile) and that space is active, macOS puts a new window with the
//! default behaviour *into* the fullscreen space, floating over the tiles; only fullscreening and
//! un-fullscreening it moves it to a desktop. A window ordered in while it carries
//! `FullScreenNone` is refused by the fullscreen space, so macOS switches to a desktop and places
//! it there.
//!
//! So every window is built hidden ([`ShellWindow::shell_window`]) and [`plugin`]'s
//! `on_window_ready` hook shows it: borrow `FullScreenNone`, order the window in, and hand back the
//! original behaviour [`RESTORE_AFTER`] later, so the window can go fullscreen as usual. Measured
//! on macOS 27 with a fullscreen window active:
//!
//! - the borrow must be in place **before the window's first order-in** — ordering a window out
//!   and back in after it landed on the fullscreen space leaves it there;
//! - the restore must come **on a later turn of the run loop** — restoring straight after the
//!   order-in leaves the window on the fullscreen space;
//! - with nothing fullscreen, the borrow changes nothing about where the window lands.

use std::time::Duration;
use tauri::{Runtime, WebviewWindowBuilder};

/// How long a newly shown window keeps the borrowed `FullScreenNone`. The window can't enter
/// fullscreen until then.
pub const RESTORE_AFTER: Duration = Duration::from_millis(200);

/// What every window builder in shell-core and its apps takes in place of a bare `.build()`'s
/// defaults: the [`crate::focus`] policy, and a hidden build that [`plugin`] then shows on a
/// desktop.
pub trait ShellWindow: Sized {
    fn shell_window(self) -> Self;
}

impl<'a, R: Runtime, M: tauri::Manager<R>> ShellWindow for WebviewWindowBuilder<'a, R, M> {
    fn shell_window(self) -> Self {
        self.focused(crate::focus::focused()).visible(false)
    }
}

/// The plugin that shows each window built through [`ShellWindow::shell_window`].
/// [`crate::register_plugins`] installs it after the geometry plugin, so a window's saved bounds
/// are restored while it is still hidden.
pub fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::<R>::new("shell-spaces")
        .on_window_ready(|window| show(&window))
        .build()
}

/// Tauri runs `on_window_ready` on the main thread. A window that is already visible was not
/// built through [`ShellWindow::shell_window`]; it is left alone.
fn show<R: Runtime>(window: &tauri::Window<R>) {
    if window.is_visible().unwrap_or(true) {
        return;
    }
    #[cfg(target_os = "macos")]
    if let Some(kept) = macos::order_in_on_desktop(window) {
        let later = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(RESTORE_AFTER);
            let target = later.clone();
            let _ = later.run_on_main_thread(move || macos::set_behaviour(&target, kept));
        });
        return;
    }
    let _ = window.show();
}

/// The collection behaviour a window carries while it is ordered in: its own, with every
/// fullscreen role swapped for `FullScreenNone`.
#[cfg(target_os = "macos")]
fn borrowed(kept: usize) -> usize {
    use objc2_app_kit::NSWindowCollectionBehavior as B;
    let roles = B::FullScreenPrimary | B::FullScreenAuxiliary | B::FullScreenAllowsTiling;
    (kept & !roles.0) | B::FullScreenNone.0
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

    fn ns_window<R: tauri::Runtime>(window: &tauri::Window<R>) -> Option<&NSWindow> {
        let ptr = window.ns_window().ok()? as *const NSWindow;
        // SAFETY: tao's live NSWindow for this window, used on the main thread while `window`
        // holds it open.
        unsafe { ptr.as_ref() }
    }

    /// Orders `window` in with the borrowed behaviour, returning the behaviour to restore.
    pub(super) fn order_in_on_desktop<R: tauri::Runtime>(
        window: &tauri::Window<R>,
    ) -> Option<usize> {
        let ns = ns_window(window)?;
        let kept = ns.collectionBehavior().0;
        ns.setCollectionBehavior(NSWindowCollectionBehavior(super::borrowed(kept)));
        if crate::focus::focused() {
            ns.makeKeyAndOrderFront(None);
        } else {
            ns.orderFront(None);
        }
        Some(kept)
    }

    pub(super) fn set_behaviour<R: tauri::Runtime>(window: &tauri::Window<R>, raw: usize) {
        if let Some(ns) = ns_window(window) {
            ns.setCollectionBehavior(NSWindowCollectionBehavior(raw));
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::borrowed;
    use objc2_app_kit::NSWindowCollectionBehavior as B;

    #[test]
    fn borrowed_swaps_every_fullscreen_role_for_none_and_keeps_the_rest() {
        assert_eq!(borrowed(B::Default.0), B::FullScreenNone.0);
        assert_eq!(
            borrowed((B::FullScreenPrimary | B::CanJoinAllSpaces).0),
            (B::FullScreenNone | B::CanJoinAllSpaces).0
        );
        assert_eq!(
            borrowed((B::FullScreenAuxiliary | B::FullScreenAllowsTiling | B::Managed).0),
            (B::FullScreenNone | B::Managed).0
        );
    }
}
