//! Launching an app without taking the operator's focus.
//!
//! With [`UNFOCUSED_ENV`] set to `1` at launch (`open -g -j --env SHELL_CORE_UNFOCUSED=1 -a
//! <app>`), the app never activates: not at launch ([`crate::register_plugins`]), not when a window
//! is built ([`focused`] on every builder), and not when one is brought forward
//! ([`bring_forward`]). Its windows still open, paint and stack, so an agent can look at them with
//! a screenshot while the operator keeps typing elsewhere.

use std::sync::OnceLock;

/// The environment variable that launches an app unfocused.
pub const UNFOCUSED_ENV: &str = "SHELL_CORE_UNFOCUSED";

static UNFOCUSED: OnceLock<bool> = OnceLock::new();

/// Whether this process was launched unfocused. The first call reads [`UNFOCUSED_ENV`] and removes
/// it from the environment, so processes the app spawns (warden's shells, and every app launched
/// from them) start ordinarily. An app without the `runtime` feature must call it first thing in
/// `main`, before building Tauri (removing an env var is safe only before other threads read the
/// environment); [`crate::register_plugins`] makes that first call on the main
/// thread, before the app starts any thread of its own.
pub fn unfocused() -> bool {
    *UNFOCUSED.get_or_init(|| {
        let on = std::env::var(UNFOCUSED_ENV).is_ok_and(|v| v == "1");
        std::env::remove_var(UNFOCUSED_ENV);
        on
    })
}

/// The value for a window builder's `.focused(…)`: a new window takes focus unless the app was
/// launched unfocused.
pub fn focused() -> bool {
    !unfocused()
}

/// A window [`bring_forward`] can raise: a bare [`tauri::Window`] or a [`tauri::WebviewWindow`].
pub trait Raise {
    fn unminimize(&self) -> tauri::Result<()>;
    fn show(&self) -> tauri::Result<()>;
    fn set_focus(&self) -> tauri::Result<()>;
}

impl<R: tauri::Runtime> Raise for tauri::Window<R> {
    /// A phone has no minimised window, and tauri offers no `unminimize` there.
    fn unminimize(&self) -> tauri::Result<()> {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        return tauri::Window::unminimize(self);
        #[cfg(any(target_os = "ios", target_os = "android"))]
        return Ok(());
    }
    fn show(&self) -> tauri::Result<()> {
        tauri::Window::show(self)
    }
    fn set_focus(&self) -> tauri::Result<()> {
        tauri::Window::set_focus(self)
    }
}

impl<R: tauri::Runtime> Raise for tauri::WebviewWindow<R> {
    /// A phone has no minimised window, and tauri offers no `unminimize` there.
    fn unminimize(&self) -> tauri::Result<()> {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        return tauri::WebviewWindow::unminimize(self);
        #[cfg(any(target_os = "ios", target_os = "android"))]
        return Ok(());
    }
    fn show(&self) -> tauri::Result<()> {
        tauri::WebviewWindow::show(self)
    }
    fn set_focus(&self) -> tauri::Result<()> {
        tauri::WebviewWindow::set_focus(self)
    }
}

/// Unminimizes `window` and brings it to the front with focus, activating the app. Launched
/// unfocused, it comes to the front of the app's own windows and the app stays inactive: `show`
/// orders the window front without activating, and only `set_focus` activates.
pub fn bring_forward(window: &impl Raise) {
    let _ = window.unminimize();
    let _ = window.show();
    if !unfocused() {
        let _ = window.set_focus();
    }
}
