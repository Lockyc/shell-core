//! The downloads popup: a small page floating over a window's content, listing the app-wide
//! downloads (`super::list`), opened and closed by the app's downloads button ([`toggle`]).
//!
//! It is an `add_child` webview in the app window, added after the content webviews so it stacks
//! above them, and focused when shown. At most one is open, app-wide. The page (`popup.html`) is
//! served over [`POPUP_SCHEME`] (same reason as [`crate::home`]'s scheme) and reports back by
//! requesting a path on it ([`parse_action`]); only a popup webview's request is acted on. It gets
//! its rows by the [`ROWS_EVENT`] event: once it is listening (it requests [`READY_PATH`]) and on
//! every list change while open (`super::with_list` calls [`list_changed`]). The page asks to close
//! on `blur` and Escape; apps also call [`close`] when a tab activates.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::http::Uri;
use tauri::{Emitter, LogicalPosition, LogicalSize, Manager, Webview, WebviewUrl, Window};

use crate::compositing::HoleRect;

/// The popup's custom URI scheme.
pub const POPUP_SCHEME: &str = "shell-downloads";

const POPUP_HTML: &str = include_str!("popup.html");

/// Rust -> page: the rows, a `Vec<DownloadRow>`.
const ROWS_EVENT: &str = "shell-downloads:rows";

/// The page requests this once it listens for [`ROWS_EVENT`], and is answered with its rows.
const READY_PATH: &str = "/ready";

/// A toggle this soon after a close of the same window's popup is the click that closed it (the
/// button's mousedown blurred the popup), so it does not reopen it.
const REOPEN_GUARD: Duration = Duration::from_millis(400);

/// The popup webview's label in window `window_label`.
fn popup_label(window_label: &str) -> String {
    format!("{POPUP_SCHEME}:{window_label}")
}

/// Whether `label` names a downloads popup.
pub fn is_popup_label(label: &str) -> bool {
    label
        .strip_prefix(POPUP_SCHEME)
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|window| !window.is_empty())
}

/// The popup's width, logical px.
const WIDTH: f64 = 360.0;
/// The least gap between the popup and the window's edges.
const MARGIN: f64 = 8.0;
/// The gap between the anchor's bottom and the popup.
const GAP: f64 = 4.0;
/// One row's height; `popup.html`'s `.row` matches it.
const ROW_H: f64 = 44.0;
/// The list's vertical padding, top plus bottom.
const LIST_PAD: f64 = 8.0;
/// The footer's height, its top border included.
const FOOTER_H: f64 = 41.0;
/// The most rows shown before the list scrolls.
const MAX_ROWS: usize = 10;

/// The popup's rect in window logical px for `rows` rows under `anchor` (the downloads button's
/// rect): below it with the left edges aligned, kept `MARGIN` inside the window. Up to
/// [`MAX_ROWS`] rows show (an empty list shows one line); a window too short cuts it, and the page
/// scrolls.
pub(crate) fn placement(anchor: HoleRect, window: (f64, f64), rows: usize) -> HoleRect {
    let (win_w, win_h) = window;
    let width = WIDTH.min(win_w - 2.0 * MARGIN).max(0.0);
    let x = anchor.x.min(win_w - MARGIN - width).max(MARGIN);
    let y = anchor.y + anchor.height + GAP;
    let wanted = rows.clamp(1, MAX_ROWS) as f64 * ROW_H + LIST_PAD + FOOTER_H;
    let height = wanted.min(win_h - MARGIN - y).max(0.0);
    HoleRect {
        x,
        y,
        width,
        height,
    }
}

/// What the popup page asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PopupAction {
    /// Open the row's file.
    Open(u64),
    /// Show the row's file in Finder.
    Reveal(u64),
    /// Clear finished and failed rows.
    Clear,
    Close,
}

/// The action a page request names: `/open?id=`, `/reveal?id=`, `/clear` or `/close`.
fn parse_action(uri: &Uri) -> Option<PopupAction> {
    let id = || {
        uri.query()?
            .split('&')
            .find_map(|kv| kv.strip_prefix("id="))?
            .parse::<u64>()
            .ok()
    };
    match uri.path() {
        "/open" => id().map(PopupAction::Open),
        "/reveal" => id().map(PopupAction::Reveal),
        "/clear" => Some(PopupAction::Clear),
        "/close" => Some(PopupAction::Close),
        _ => None,
    }
}

/// The open popup.
struct Open {
    webview: Webview,
    anchor: HoleRect,
}

static OPEN: Mutex<Option<Open>> = Mutex::new(None);

/// The window whose popup last closed, and when ([`REOPEN_GUARD`]).
static LAST_CLOSED: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// The rows as the popup shows them now.
fn rows() -> Vec<super::DownloadRow> {
    super::with_list_quiet(|l| {
        l.rows(|p| {
            std::fs::metadata(p)
                .ok()
                .filter(|m| m.is_file())
                .map(|m| m.len())
        })
    })
}

/// `window`'s inner size in logical px.
fn logical_size(window: &Window) -> (f64, f64) {
    let scale = window.scale_factor().unwrap_or(1.0);
    window
        .inner_size()
        .map(|s| s.to_logical::<f64>(scale))
        .map(|s| (s.width, s.height))
        .unwrap_or((0.0, 0.0))
}

/// Sends `rows` to the popup and fits it to their number.
fn push(webview: &Webview, anchor: HoleRect, rows: Vec<super::DownloadRow>) {
    let window = webview.window();
    let r = placement(anchor, logical_size(&window), rows.len());
    let _ = webview.set_position(LogicalPosition::new(r.x, r.y));
    let _ = webview.set_size(LogicalSize::new(r.width, r.height));
    let _ = webview
        .app_handle()
        .emit_to(webview.label(), ROWS_EVENT, rows);
}

/// Pushes the rows to the open popup, if any. `super::with_list` calls it after every change.
pub(super) fn list_changed() {
    let open = OPEN
        .lock()
        .unwrap()
        .as_ref()
        .map(|o| (o.webview.clone(), o.anchor));
    if let Some((webview, anchor)) = open {
        push(&webview, anchor, rows());
    }
}

/// Opens `window`'s popup under `anchor` (the downloads button's rect, window logical px), marking
/// every row seen, or closes it when it is open. Opening closes any other window's popup.
pub fn toggle(window: &Window, anchor: HoleRect) {
    let open_here = OPEN
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|o| o.webview.window().label() == window.label());
    if open_here {
        close(window);
        return;
    }
    let just_closed = LAST_CLOSED
        .lock()
        .unwrap()
        .take()
        .is_some_and(|(w, at)| w == window.label() && at.elapsed() < REOPEN_GUARD);
    if just_closed {
        return;
    }
    close_any();

    super::with_list(|l| l.mark_seen());
    let r = placement(anchor, logical_size(window), rows().len());
    let url: tauri::Url = format!("{POPUP_SCHEME}://localhost/")
        .parse()
        .expect("POPUP_SCHEME url is a fixed, valid literal");
    let builder = tauri::webview::WebviewBuilder::new(
        popup_label(window.label()),
        WebviewUrl::CustomProtocol(url),
    )
    .background_color(tauri::webview::Color(30, 30, 30, 255))
    .focused(true);
    let Ok(webview) = window.add_child(
        builder,
        LogicalPosition::new(r.x, r.y),
        LogicalSize::new(r.width, r.height),
    ) else {
        return;
    };
    let _ = webview.set_focus();
    *OPEN.lock().unwrap() = Some(Open { webview, anchor });
}

/// Closes `window`'s popup. A no-op when it has none open.
pub fn close(window: &Window) {
    let taken = take_open(|o| o.webview.window().label() == window.label());
    if let Some(webview) = taken {
        let _ = webview.close();
    }
}

/// Closes whichever popup is open.
fn close_any() {
    if let Some(webview) = take_open(|_| true) {
        let _ = webview.close();
    }
}

/// Takes the open popup out of [`OPEN`] when `pick` accepts it, recording its close for
/// [`REOPEN_GUARD`].
fn take_open(pick: impl Fn(&Open) -> bool) -> Option<Webview> {
    let mut open = OPEN.lock().unwrap();
    if !open.as_ref().is_some_and(&pick) {
        return None;
    }
    let webview = open.take()?.webview;
    *LAST_CLOSED.lock().unwrap() = Some((webview.window().label().to_string(), Instant::now()));
    Some(webview)
}

/// Closes the popup `label` from inside its own request: the close is posted to the main thread
/// from another thread, so the webview is never closed while its scheme request is still running.
fn close_from_page(label: &str) {
    if let Some(webview) = take_open(|o| o.webview.label() == label) {
        let app = webview.app_handle().clone();
        std::thread::spawn(move || {
            let _ = app.run_on_main_thread(move || {
                let _ = webview.close();
            });
        });
    }
}

/// Runs `action` from popup `label`.
fn act(label: &str, action: PopupAction) {
    let reveal = |id: u64, flag: Option<&str>| {
        let Some(path) = super::with_list_quiet(|l| l.path_of(id)) else {
            return;
        };
        if !path.exists() {
            return;
        }
        let mut cmd = std::process::Command::new("open");
        cmd.args(flag).arg(path);
        crate::spawn_reaped(&mut cmd);
    };
    match action {
        PopupAction::Open(id) => reveal(id, None),
        PopupAction::Reveal(id) => reveal(id, Some("-R")),
        PopupAction::Clear => {
            super::with_list(|l| l.clear());
            return;
        }
        PopupAction::Close => {}
    }
    close_from_page(label);
}

/// Registers [`POPUP_SCHEME`]: serves [`POPUP_HTML`], answers [`READY_PATH`] with the rows, and
/// runs each [`PopupAction`]. Only a popup webview's request is answered; the scheme is registered
/// on every webview, so any other page gets a 403.
pub(crate) fn register_protocol<R: tauri::Runtime>(
    builder: tauri::Builder<R>,
) -> tauri::Builder<R> {
    builder.register_uri_scheme_protocol(POPUP_SCHEME, |ctx, req| {
        let respond = |status: u16, ct: &str, body: Vec<u8>| {
            tauri::http::Response::builder()
                .status(status)
                .header(tauri::http::header::CONTENT_TYPE, ct)
                .body(body)
                .expect("static response")
        };
        let label = ctx.webview_label();
        if !is_popup_label(label) {
            return respond(403, "text/plain", b"not the downloads popup".to_vec());
        }
        if req.uri().path() == READY_PATH {
            list_changed();
            return respond(200, "text/plain", b"ok".to_vec());
        }
        if let Some(action) = parse_action(req.uri()) {
            act(label, action);
            return respond(200, "text/plain", b"ok".to_vec());
        }
        respond(
            200,
            "text/html; charset=utf-8",
            POPUP_HTML.as_bytes().to_vec(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: (f64, f64) = (800.0, 600.0);

    fn rect(x: f64, y: f64, width: f64, height: f64) -> HoleRect {
        HoleRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn placement_sits_below_anchor_left_aligned() {
        let p = placement(rect(100.0, 20.0, 24.0, 24.0), WIN, 2);
        assert_eq!(p.x, 100.0);
        assert!(p.y >= 44.0, "below the anchor, got {}", p.y);
        assert_eq!(p.width, WIDTH);
        assert_eq!(p.height, 2.0 * ROW_H + LIST_PAD + FOOTER_H);
    }

    #[test]
    fn placement_clamps_right_edge() {
        let p = placement(rect(700.0, 20.0, 24.0, 24.0), WIN, 1);
        assert_eq!(p.width, WIDTH);
        assert!(p.x + WIDTH <= 800.0 - MARGIN, "x = {}", p.x);
        assert!(p.x >= MARGIN);
    }

    #[test]
    fn placement_clamps_small_window() {
        let p = placement(rect(10.0, 20.0, 24.0, 24.0), (200.0, 600.0), 1);
        assert!(p.width <= 200.0 - 2.0 * MARGIN, "width = {}", p.width);
        assert!(p.x >= MARGIN && p.x + p.width <= 200.0 - MARGIN);
        let p = placement(rect(10.0, 20.0, 24.0, 24.0), (5.0, 600.0), 1);
        assert!(p.width >= 0.0);
    }

    #[test]
    fn placement_caps_height_at_ten_rows() {
        let ten = placement(rect(0.0, 0.0, 24.0, 24.0), (800.0, 2000.0), 10);
        let many = placement(rect(0.0, 0.0, 24.0, 24.0), (800.0, 2000.0), 20);
        assert_eq!(ten.height, 10.0 * ROW_H + LIST_PAD + FOOTER_H);
        assert_eq!(many.height, ten.height);
        // Empty shows one "No downloads" line.
        let empty = placement(rect(0.0, 0.0, 24.0, 24.0), (800.0, 2000.0), 0);
        assert_eq!(empty.height, ROW_H + LIST_PAD + FOOTER_H);
        // A short window cuts the popup at its bottom margin; the page scrolls.
        let short = placement(rect(0.0, 0.0, 24.0, 24.0), (800.0, 200.0), 10);
        assert!(short.y + short.height <= 200.0 - MARGIN);
    }

    #[test]
    fn parse_action_reads_each_path() {
        let u = |s: &str| s.parse::<Uri>().unwrap();
        assert_eq!(
            parse_action(&u("shell-downloads://localhost/open?id=7")),
            Some(PopupAction::Open(7))
        );
        assert_eq!(
            parse_action(&u("shell-downloads://localhost/reveal?id=12")),
            Some(PopupAction::Reveal(12))
        );
        assert_eq!(
            parse_action(&u("shell-downloads://localhost/clear")),
            Some(PopupAction::Clear)
        );
        assert_eq!(
            parse_action(&u("shell-downloads://localhost/close")),
            Some(PopupAction::Close)
        );
    }

    #[test]
    fn parse_action_rejects_unknown() {
        let u = |s: &str| s.parse::<Uri>().unwrap();
        assert_eq!(parse_action(&u("shell-downloads://localhost/")), None);
        assert_eq!(parse_action(&u("shell-downloads://localhost/delete")), None);
        assert_eq!(parse_action(&u("shell-downloads://localhost/open")), None);
        assert_eq!(
            parse_action(&u("shell-downloads://localhost/open?id=x")),
            None
        );
        assert_eq!(
            parse_action(&u("shell-downloads://localhost/reveal?ids=1")),
            None
        );
    }

    /// The page names the event and paths as string literals it can't import, so a renamed
    /// constant would compile and silently cut the popup off.
    #[test]
    fn the_page_listens_for_rows_and_requests_each_path() {
        assert!(POPUP_HTML.contains(&format!("'{ROWS_EVENT}'")));
        for path in [READY_PATH, "/open?id=", "/reveal?id=", "/clear", "/close"] {
            assert!(POPUP_HTML.contains(&format!("'{path}")), "{path}");
        }
    }

    #[test]
    fn is_popup_label_matches_only_popups() {
        assert!(is_popup_label(&popup_label("main")));
        assert!(is_popup_label("shell-downloads:w-2"));
        assert!(!is_popup_label("main"));
        assert!(!is_popup_label("main:tab-1"));
        assert!(!is_popup_label("shell-downloads"));
        assert!(!is_popup_label("x:shell-downloads:main"));
    }
}
