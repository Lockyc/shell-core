//! Content-webview compositing primitives shared by curator and lector — the hole-punch layout
//! where the sidebar chrome is the window's *main* webview and the `add_child` content webviews are
//! positioned by Rust to fill the reported content hole.
//!
//! warden is deliberately **not** a consumer: it composites a native `NSView` and positions it
//! through its own `geometry.rs` (a bottom-left → top-left Y-flip + HiDPI backing-size conversion),
//! so it keeps its own rect type and native positioning. Only curator and lector place child
//! webviews this way, and their code for it was byte-identical — so it lives here once.

use tauri::{LogicalPosition, LogicalSize, Runtime, Window};

/// Default sidebar width in logical px — the chrome's reset/first-run default and the offset
/// [`initial_hole`] uses before the chrome's first `set_hole_rect`. MUST match `chrome.js`'s own
/// literal `240` fallback: no value crosses the IPC boundary to keep the two in sync, so the
/// constant and the JS fallback are single-sourced only by this shared definition + that comment.
pub const CHROME_W: f64 = 240.0;

/// The content hole's rect in logical px (top-left origin), exactly as the chrome measures its
/// `#content-hole`/`#terminal-hole` element via `getBoundingClientRect` and reports it through
/// `set_hole_rect`. This is the single source of truth for content-webview placement — the chrome
/// owns the sidebar width and its resize clamp; Rust just tracks and applies the rect it reports.
/// (Tauri's `LogicalPosition`/`LogicalSize` are top-left too, so — unlike warden's bottom-left
/// native `NSView` surface — no coordinate flip is needed; the reported rect is used as-is.)
#[derive(Debug, Clone, Copy)]
pub struct HoleRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The best-guess hole before the chrome's first `set_hole_rect`: full height, offset by the
/// default sidebar width. The first report corrects it — this only has to place launch-time
/// `load_on_open` tabs sensibly for the frame or two before the chrome mounts and measures.
pub fn initial_hole(win_w: f64, win_h: f64) -> HoleRect {
    HoleRect {
        x: CHROME_W,
        y: 0.0,
        width: (win_w - CHROME_W).max(0.0),
        height: win_h,
    }
}

/// Position every content webview to fill the reported hole. The chrome is the window's main
/// webview (auto-sized to the window as its content view; its label IS the window label), so it's
/// skipped — only the `add_child` content webviews are placed. All loaded tabs stack in the same
/// hole; the app raises the active one, `load_on_open` tabs sit live behind it.
pub fn layout_webviews<R: Runtime>(window: &Window<R>, hole: HoleRect) {
    for wv in window.webviews() {
        if wv.label() == window.label() {
            continue;
        }
        let _ = wv.set_position(LogicalPosition::new(hole.x, hole.y));
        let _ = wv.set_size(LogicalSize::new(hole.width.max(0.0), hole.height.max(0.0)));
    }
}
