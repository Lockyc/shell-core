//! Determinate content-webview loading bar — a thin native bar pinned to the top of each content
//! WKWebView, shared by curator + lector (warden hosts native terminals, no WKWebView, doesn't use
//! it). Like Safari/Chrome's: it fills as the page loads, then hides when done.
//!
//! The bar is a plain `NSView` added as a **subview of the content WKWebView**, so it inherits the
//! webview's show/hide/raise/resize for free and renders above the page. A view-owned repeating
//! `NSTimer` reads the host webview's `estimatedProgress`/`isLoading` and drives the fill width.
//!
//! **Why poll, not KVO:** `addObserver:forKeyPath:` on the WKWebView crashes if the webview
//! deallocates (tab unload / recreate) while still observed, and wry gives no clean webview-close
//! hook to remove the observer first. The timer self-cleans instead: its block holds the bar, the
//! bar is otherwise owned by its superview (the webview), so when the webview goes away the bar's
//! `superview` becomes nil and the next tick invalidates the timer — releasing the block, the bar,
//! and (via the runloop) the timer. No dangling registration, no retain-cycle leak.
//!
//! Accent is passed as raw `(r, g, b, a)` f64s so this core needs no `config_core` dependency (the
//! three-cores independence rule) — each app converts from its own `Colour`.

/// Install the loading bar on `webview`'s content WKWebView. Call once per content webview at
/// creation, on the main thread. `accent` is the fill colour (sRGB, 0..=1).
#[cfg(target_os = "macos")]
pub fn install(webview: &tauri::Webview, accent: (f64, f64, f64, f64)) {
    let _ = webview.with_webview(move |pw| unsafe { install_on_webview(pw.inner(), accent) });
}

#[cfg(target_os = "macos")]
unsafe fn install_on_webview(webview_ptr: *mut std::ffi::c_void, accent: (f64, f64, f64, f64)) {
    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSColor, NSView};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSTimer};

    const BAR_H: f64 = 3.0;

    // NSView is main-thread-only; content-webview creation (our only caller) runs on the main thread.
    let (Some(mtm), Some(webview)) = (
        MainThreadMarker::new(),
        (webview_ptr as *mut AnyObject).as_ref(),
    ) else {
        return;
    };

    // The bar view, coloured via its backing layer.
    let bar: Retained<NSView> = NSView::initWithFrame(
        mtm.alloc(),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, BAR_H)),
    );
    bar.setWantsLayer(true);
    let (r, g, b, a) = accent;
    let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
    let cg: *mut AnyObject = msg_send![&color, CGColor];
    let layer: *mut AnyObject = msg_send![&bar, layer];
    let _: () = msg_send![layer, setBackgroundColor: cg];
    let _: () = msg_send![webview, addSubview: &*bar];

    // Drive the fill from the host webview's estimatedProgress. The block holds `bar` strongly; when
    // the webview deallocs, `bar`'s superview link clears and the next tick invalidates the timer,
    // releasing block → bar → timer. ~30 Hz.
    let bar_for_timer = bar.clone();
    let block = RcBlock::new(move |timer: core::ptr::NonNull<NSTimer>| {
        let Some(host) = bar_for_timer.superview() else {
            unsafe { timer.as_ref().invalidate() };
            return;
        };
        let progress: f64 = unsafe { msg_send![&*host, estimatedProgress] };
        let loading: bool = unsafe { msg_send![&*host, isLoading] };
        let bounds = host.bounds();
        let filled = (bounds.size.width * progress).max(0.0);
        // Non-flipped coords: pin to the top edge (y = height − barHeight).
        bar_for_timer.setFrame(NSRect::new(
            NSPoint::new(0.0, bounds.size.height - BAR_H),
            NSSize::new(filled, BAR_H),
        ));
        // Done (fully loaded or not loading) → hide; a new navigation drops progress and re-shows it.
        bar_for_timer.setHidden(progress >= 1.0 || !loading);
    });
    let _: Retained<NSTimer> =
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.0 / 30.0, true, &block);
}

#[cfg(not(target_os = "macos"))]
pub fn install(_webview: &tauri::Webview, _accent: (f64, f64, f64, f64)) {}
