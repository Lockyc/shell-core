//! The scroll-clamp shim (`src/web/inject/scroll-clamp.js`) against this Mac's own WebKit, in a
//! real `WKWebView`. Two checks:
//!
//! - **canary** — without the shim, WebKit still drags a bottom-pinned scroller up when a re-render
//!   removes and re-inserts its last child with a layout read in between. When this fails, WebKit
//!   has started deferring the clamp the way Chromium does, and the shim is dead weight *on this
//!   macOS*: it can go once the oldest macOS the apps support ships that WebKit.
//! - **guard** — with the shim, every case lands where Chromium puts it.
//!
//! `harness = false` (see Cargo.toml): WebKit must run on the process's main thread, which libtest
//! never gives a test.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    mac::run();
}

#[cfg(target_os = "macos")]
mod mac {
    use std::time::{Duration, Instant};

    use objc2::{MainThreadMarker, MainThreadOnly as _};
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{NSDate, NSPoint, NSProcessInfo, NSRect, NSRunLoop, NSSize, NSString};
    use objc2_web_kit::{
        WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration,
    };
    use serde_json::Value;

    const SHIM: &str = include_str!("../src/web/inject/scroll-clamp.js");

    /// Five 300px scrollers of 120px rows over a 51px last row, each pinned to its bottom by the page
    /// (as Chat pins its list), then each changed in its own task. Reports where each ended up.
    /// Chromium's results, which the guard expects: A at bottom, B 100, C 500, D at its old bottom
    /// (now short of the new one), E at the new bottom.
    const PAGE: &str = r#"<!doctype html>
<style>body{margin:0}.s{height:300px;overflow:auto;display:flex;flex-direction:column}.m{flex:none;height:120px}.b{height:51px}</style>
<script>
function scroller(){var s=document.createElement('div');s.className='s';for(var i=0;i<12;i++){var d=document.createElement('div');d.className='m';s.appendChild(d)}var b=document.createElement('div');b.className='m b';s.appendChild(b);document.body.appendChild(s);return s}
function max(s){return s.scrollHeight-s.clientHeight}
addEventListener('load',function(){
  var A=scroller(),B=scroller(),C=scroller(),D=scroller(),E=scroller();
  [A,B,C,D,E].forEach(function(s){s.scrollTop=max(s)});
  C.scrollTop=500;
  var pinned=[A,B,C,D,E].map(max);
  setTimeout(function(){
    // A: Chat's re-render — remove the last row, read layout, insert its replacement.
    var l=A.lastElementChild,n=l.cloneNode(true);l.remove();void A.scrollHeight;A.appendChild(n);
    // B: the page scrolls the list itself mid-re-render: its position stands.
    setTimeout(function(){var l=B.lastElementChild;l.remove();void B.scrollHeight;B.scrollTop=100;B.appendChild(l.cloneNode())},0);
    // C: off the bottom, so not pinned: never touched.
    setTimeout(function(){var l=C.lastElementChild,n=l.cloneNode(true);l.remove();void C.scrollHeight;C.appendChild(n)},0);
    // D: growth below a pinned scroller does not follow it down.
    setTimeout(function(){var d=document.createElement('div');d.className='m';D.appendChild(d)},0);
    // E: a real shrink clamps to the new bottom and stays.
    setTimeout(function(){E.lastElementChild.remove()},0);
    setTimeout(function(){
      document.title='RESULT '+JSON.stringify({
        pinned:pinned,
        A:[A.scrollTop,max(A)],B:B.scrollTop,C:C.scrollTop,D:[D.scrollTop,max(D)],E:[E.scrollTop,max(E)]});
    },300);
  },100);
});
</script>"#;

    /// Load [`PAGE`] (with or without the shim) and return what it reports.
    fn measure(mtm: MainThreadMarker, shim: bool) -> Value {
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        if shim {
            let script = unsafe {
                WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    WKUserScript::alloc(mtm),
                    &NSString::from_str(SHIM),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    true,
                )
            };
            unsafe { config.userContentController().addUserScript(&script) };
        }
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(600.0, 900.0));
        let webview = unsafe {
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config)
        };
        unsafe { webview.loadHTMLString_baseURL(&NSString::from_str(PAGE), None) };

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.05));
            let title = unsafe { webview.title() }
                .map(|t| t.to_string())
                .unwrap_or_default();
            if let Some(json) = title.strip_prefix("RESULT ") {
                return serde_json::from_str(json).expect("page reports JSON");
            }
            assert!(
                Instant::now() < deadline,
                "the test page never reported (shim: {shim})"
            );
        }
    }

    fn at_bottom(pair: &Value) -> bool {
        pair[0].as_f64().unwrap() >= pair[1].as_f64().unwrap() - 1.0
    }

    pub fn run() {
        let mtm = MainThreadMarker::new().expect("harness = false runs main() on the main thread");
        let _app = NSApplication::sharedApplication(mtm);
        let macos = NSProcessInfo::processInfo()
            .operatingSystemVersionString()
            .to_string();

        let bare = measure(mtm, false);
        assert!(
            !at_bottom(&bare["A"]),
            "canary: WebKit on macOS {macos} no longer drags a bottom-pinned scroller up during a \
             remove/re-insert re-render ({bare}). The scroll-clamp shim \
             (src/web/inject/scroll-clamp.js) is redundant on this macOS — remove it, and this test, \
             once the oldest macOS the apps support has this WebKit."
        );
        println!(
            "test scroll_clamp::canary ... ok (WebKit still clamps mid-change on macOS {macos})"
        );

        let shimmed = measure(mtm, true);
        let pinned_d = shimmed["pinned"][3].as_f64().unwrap();
        let checks = [
            ("A re-render keeps the bottom", at_bottom(&shimmed["A"])),
            (
                "B page's own scroll stands",
                shimmed["B"].as_f64() == Some(100.0),
            ),
            (
                "C off-bottom scroller untouched",
                shimmed["C"].as_f64() == Some(500.0),
            ),
            (
                "D growth below doesn't follow",
                shimmed["D"][0].as_f64() == Some(pinned_d) && !at_bottom(&shimmed["D"]),
            ),
            (
                "E real shrink rests at the new bottom",
                at_bottom(&shimmed["E"]),
            ),
        ];
        let failed: Vec<_> = checks
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(name, _)| *name)
            .collect();
        assert!(
            failed.is_empty(),
            "guard: {failed:?} failed with the shim: {shimmed}"
        );
        println!("test scroll_clamp::guard ... ok");
    }
}
