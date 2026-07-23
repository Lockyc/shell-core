//! Content-webview loading bar — a thin determinate progress bar pinned to the top of each content
//! WKWebView, shared by curator + lector (warden hosts native terminals, no WKWebView). Mirrors
//! `mouse_nav`'s split: this core owns the native mechanism (an `NSView` overlay + the timer that
//! drives it); the app calls [`install`] once per content webview and supplies the accent colour.
//!
//! **Determinate, driven by WebKit's own signal.** The bar reads its host WKWebView's
//! `estimatedProgress` (0→1) and fills left→right; at 100% it fades out (alpha decay) and hides; a
//! new navigation drops `estimatedProgress` low again so the bar re-shows on its own.
//!
//! **Poll, not KVO.** A view-owned block `NSTimer` reads progress from `bar.superview()`
//! (== the WKWebView) each tick. KVO is avoided deliberately: `addObserver:forKeyPath:` on the
//! webview crashes if the webview deallocates (tab unload/recreate) while still observed, and wry
//! gives no clean webview-close hook to remove it first. This self-cleans instead: the timers' blocks
//! are the only strong owners of the bar besides the webview, so when the webview deallocates and
//! drops its subview, the next tick sees `superview()` nil and invalidates the timer — releasing the
//! block, so the bar deallocates once the last one goes. The bar is a plain layer-backed subview, so
//! it inherits the webview's show/hide/raise/resize automatically (no hole-rect tracking).
//!
//! **Two timers, because a load is rare and an app is idle almost always.** A *watcher* runs
//! permanently at [`IDLE_TICK_SECS`] with a 50% `setTolerance:` (so macOS can coalesce it with other
//! wakeups) and does nothing but notice `estimatedProgress` dropping below 1.0. Only then does it
//! schedule the *animator* at [`ACTIVE_TICK_SECS`] to draw the fill and the fade-out; the animator
//! invalidates itself the moment the fade completes. A shared `animating` flag (main-thread `Cell`)
//! keeps the watcher from stacking duplicate animators.
//!
//! **Footgun: never collapse these back into one always-on fast timer.** Doing so costs more in
//! `mk_timer_arm` — the kernel *re-arming* the timer — than in the callback itself, and it is paid
//! per content webview for the whole life of the app, not per load. Measured on curator with 5 live
//! tabs: an always-on 30 Hz bar timer was **85% of all main-thread work** while the app sat idle in
//! the background, two-thirds of that in `mk_timer_arm` alone. A high-frequency repeating timer also
//! defeats macOS timer coalescing and deep idle, which is what Activity Monitor's Energy Impact
//! scores hardest. The fast rate is affordable only because it is scoped to an actual page load.

/// Watcher tick — how often an idle bar checks whether a new navigation has started. Runs for the
/// whole life of every content webview, so this is the rate that shows up in a backgrounded app's
/// energy use; it is scheduled with a 50% tolerance so macOS can coalesce it.
#[cfg(target_os = "macos")]
pub const IDLE_TICK_SECS: f64 = 0.25; // 4 Hz

/// Animator tick — the redraw rate while a page is actually loading (fill) or the bar is fading out.
/// Only ever scheduled between a navigation starting and its fade completing.
#[cfg(target_os = "macos")]
pub const ACTIVE_TICK_SECS: f64 = 0.033; // ~30 Hz

/// Install the loading bar on `webview`'s content WKWebView. `accent` is the bar colour as sRGB
/// `(r, g, b, a)` in 0.0–1.0 (raw components, not a `config_core::Colour`, to keep shell-core free of
/// a config-core dependency — the app converts from its own colour type). Call once per content
/// webview at creation, on the main thread.
#[cfg(target_os = "macos")]
pub fn install(webview: &tauri::Webview, accent: (f64, f64, f64, f64)) {
    let _ = webview.with_webview(move |pw| unsafe {
        use block2::RcBlock;
        use objc2::rc::Retained;
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        use objc2_app_kit::{NSColor, NSView};
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        use std::cell::Cell;
        use std::rc::Rc;

        const HEIGHT: f64 = 3.0;

        let wk = pw.inner() as *mut AnyObject;
        let Some(webview_obj) = wk.as_ref() else {
            return;
        };

        // The bar view: layer-backed so we can colour it with a CALayer background. Initial frame is
        // zero-width (hidden until the first progress tick positions it).
        let alloc: *mut AnyObject = msg_send![class!(NSView), alloc];
        let bar_ptr: *mut AnyObject =
            msg_send![alloc, initWithFrame: NSRect::new(NSPoint::ZERO, NSSize::ZERO)];
        let bar: Retained<NSView> =
            Retained::from_raw(bar_ptr.cast()).expect("NSView initWithFrame returned nil");
        bar.setWantsLayer(true);
        bar.setHidden(true);

        // Colour the layer from the accent (sRGB). `CGColor`/`setBackgroundColor:` via msg_send to
        // avoid pulling QuartzCore/CoreGraphics crates just for the colour.
        let (r, g, b, a) = accent;
        let color: Retained<NSColor> = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
        let cg: *mut AnyObject = msg_send![&*color, CGColor];
        let layer: *mut AnyObject = msg_send![&*bar, layer];
        let _: () = msg_send![layer, setBackgroundColor: cg];

        // Add above the page content. The webview retains it; our local `bar` ref drops when
        // `install` returns, leaving the webview and the timer blocks below as the only owners — so
        // its lifetime tracks the webview's.
        let _: () = msg_send![webview_obj, addSubview: &*bar];

        // Set by the watcher when it schedules an animator, cleared by the animator when it stops —
        // so the watcher never stacks two animators on one bar. Main thread only, hence `Cell`.
        let animating = Rc::new(Cell::new(false));

        // ── The animator: drives the fill and the fade, then retires ────────────────────────────
        // Scheduled by the watcher on a fresh navigation and invalidated by itself once the fade
        // finishes, so it exists only while there is something to draw.
        let bar_a = bar.clone();
        let animating_a = animating.clone();
        let animator = RcBlock::new(move |timer: *mut AnyObject| {
            let Some(host) = bar_a.superview() else {
                let _: () = msg_send![timer, invalidate];
                animating_a.set(false);
                return;
            };
            let progress: f64 = msg_send![&*host, estimatedProgress];
            let bounds: NSRect = host.bounds();
            let flipped: bool = host.isFlipped();
            // Top edge: y=0 when flipped (top-left origin), else height-HEIGHT.
            let top_y = if flipped {
                0.0
            } else {
                bounds.size.height - HEIGHT
            };

            if progress < 1.0 {
                // Loading: full-height strip, width proportional to progress, fully opaque.
                bar_a.setHidden(false);
                bar_a.setAlphaValue(1.0);
                bar_a.setFrame(NSRect::new(
                    NSPoint::new(0.0, top_y),
                    NSSize::new(bounds.size.width * progress, HEIGHT),
                ));
            } else {
                // Complete: fill full width, then fade out via alpha decay (alpha doubles as the
                // fade state — no stored flag). Once faded, stand down; the watcher re-arms us on
                // the next navigation.
                bar_a.setFrame(NSRect::new(
                    NSPoint::new(0.0, top_y),
                    NSSize::new(bounds.size.width, HEIGHT),
                ));
                let alpha: f64 = msg_send![&*bar_a, alphaValue];
                if alpha <= 0.05 {
                    bar_a.setHidden(true);
                    let _: () = msg_send![timer, invalidate];
                    animating_a.set(false);
                } else {
                    bar_a.setAlphaValue(alpha * 0.80);
                }
            }
        });

        // ── The watcher: permanent, cheap, and the only thing running while idle ────────────────
        // Holds the strong ref to `bar` (and, via `animator`, the second one) that makes the
        // self-cleaning work: when the webview deallocates it releases its subview, `superview()`
        // goes nil, and invalidating here releases this block — and with it the animator block and
        // the bar. No weak reference or KVO teardown needed.
        let bar_w = bar.clone();
        let animating_w = animating.clone();
        let watcher = RcBlock::new(move |timer: *mut AnyObject| {
            let Some(host) = bar_w.superview() else {
                let _: () = msg_send![timer, invalidate];
                return;
            };
            if animating_w.get() {
                return; // an animator already owns the bar
            }
            let progress: f64 = msg_send![&*host, estimatedProgress];
            if progress < 1.0 {
                // A navigation started — hand the bar to a fast animator until its fade completes.
                // The block is reused across loads; NSTimer retains it for the timer's lifetime.
                animating_w.set(true);
                let _: *mut AnyObject = msg_send![
                    class!(NSTimer),
                    scheduledTimerWithTimeInterval: ACTIVE_TICK_SECS,
                    repeats: true,
                    block: &*animator,
                ];
            }
        });

        // Schedule on the current (main) runloop. The timer retains the block; the runloop retains
        // the timer — both stay alive until the block invalidates it. The tolerance is what lets
        // macOS coalesce this wakeup with others rather than arming a dedicated one every tick.
        let watcher_timer: *mut AnyObject = msg_send![
            class!(NSTimer),
            scheduledTimerWithTimeInterval: IDLE_TICK_SECS,
            repeats: true,
            block: &*watcher,
        ];
        let _: () = msg_send![watcher_timer, setTolerance: IDLE_TICK_SECS * 0.5];
    });
}

#[cfg(not(target_os = "macos"))]
pub fn install(_webview: &tauri::Webview, _accent: (f64, f64, f64, f64)) {}
