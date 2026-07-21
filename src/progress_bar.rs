//! Content-webview loading bar — a thin determinate progress bar pinned to the top of each content
//! WKWebView, shared by curator + lector (warden hosts native terminals, no WKWebView). Mirrors
//! `mouse_nav`'s split: this core owns the native mechanism (an `NSView` overlay + the timer that
//! drives it); the app calls [`install`] once per content webview and supplies the accent colour.
//!
//! **Determinate, driven by WebKit's own signal.** The bar reads its host WKWebView's
//! `estimatedProgress` (0→1) and fills left→right; at 100% it fades out (alpha decay) and hides; a
//! new navigation drops `estimatedProgress` low again so the bar re-shows on its own.
//!
//! **Poll, not KVO.** A view-owned block `NSTimer` (~30 Hz) reads progress from `bar.superview()`
//! (== the WKWebView) each tick. KVO is avoided deliberately: `addObserver:forKeyPath:` on the
//! webview crashes if the webview deallocates (tab unload/recreate) while still observed, and wry
//! gives no clean webview-close hook to remove it first. This self-cleans instead: the timer's block
//! is the only strong owner of the bar besides the webview, so when the webview deallocates and drops
//! its subview, the next tick sees `superview()` nil and invalidates the timer — releasing the block,
//! the last owner, so the bar deallocates too. The bar is a plain layer-backed subview, so it
//! inherits the webview's show/hide/raise/resize automatically (no hole-rect tracking).

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

        const HEIGHT: f64 = 3.0;
        const TICK_SECS: f64 = 0.033; // ~30 Hz

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

        // Add above the page content. The webview retains it; our local `bar` ref drops when `install`
        // returns, leaving the webview the sole owner — so its lifetime tracks the webview's.
        let _: () = msg_send![webview_obj, addSubview: &*bar];

        // The block owns the only strong ref to `bar` besides the webview's. When the webview
        // deallocates it releases its subview, `bar.superview()` goes nil, and we invalidate the
        // timer — which releases the block, the last owner, so the bar deallocates too. No weak
        // reference or KVO teardown needed.
        let block = RcBlock::new(move |timer: *mut AnyObject| {
            let Some(host) = bar.superview() else {
                let _: () = msg_send![timer, invalidate];
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
                bar.setHidden(false);
                bar.setAlphaValue(1.0);
                bar.setFrame(NSRect::new(
                    NSPoint::new(0.0, top_y),
                    NSSize::new(bounds.size.width * progress, HEIGHT),
                ));
            } else {
                // Complete: fill full width, then fade out via alpha decay (alpha doubles as the
                // fade state — no stored flag). A new navigation resets progress < 1.0 above.
                bar.setFrame(NSRect::new(
                    NSPoint::new(0.0, top_y),
                    NSSize::new(bounds.size.width, HEIGHT),
                ));
                let alpha: f64 = msg_send![&*bar, alphaValue];
                if alpha <= 0.05 {
                    bar.setHidden(true);
                } else {
                    bar.setAlphaValue(alpha * 0.80);
                }
            }
        });

        // Schedule on the current (main) runloop. The timer retains the block; the runloop retains the
        // timer — both stay alive until the block invalidates it.
        let _: *mut AnyObject = msg_send![
            class!(NSTimer),
            scheduledTimerWithTimeInterval: TICK_SECS,
            repeats: true,
            block: &*block,
        ];
    });
}

#[cfg(not(target_os = "macos"))]
pub fn install(_webview: &tauri::Webview, _accent: (f64, f64, f64, f64)) {}
