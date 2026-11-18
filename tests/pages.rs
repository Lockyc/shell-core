//! The bundled pages (`detach.html`, `form.html`, `popup.html`) against this Mac's own WebKit, in a
//! real, never-shown `WKWebView`. Each page runs under a stub `window.__TAURI__` and a stub `fetch`
//! that record every call; a driver script appended after the page's own acts on it and reports
//! through `document.title`.
//!
//! - **detach** — the payload's title, accent and load error render; the hole's rect reaches
//!   `set_hole_rect`; the error event shows, dedupes and clears the bar, re-reporting the hole.
//! - **form** — Save reports through `fetch('/result?p=…')` and never through an event emit, which
//!   apps grant no capability for; a taken title disables Save; Delete and Escape report too.
//! - **popup** — `/ready` is requested only once the rows listener is up; rows render with their
//!   controls, and an unavailable control explains itself and sends nothing.
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
    use objc2_foundation::{NSDate, NSPoint, NSRect, NSRunLoop, NSSize, NSString};
    use objc2_web_kit::{
        WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration,
    };
    use serde_json::{json, Value};

    const DETACH: &str = include_str!("../src/detach.html");
    const FORM: &str = include_str!("../src/windows/form.html");
    const POPUP: &str = include_str!("../src/downloads/popup.html");

    /// Records `invoke`, `listen`, `emit` and `fetch` calls in `__calls`; `__emit(name, payload)`
    /// delivers an event to the page's listeners.
    const STUB: &str = r#"
window.__calls = [];
window.__listeners = {};
window.__TAURI__ = {
  core: { invoke: function (cmd, args) { __calls.push(['invoke', cmd, args]); return Promise.resolve(); } },
  event: {
    listen: function (name, cb) {
      (__listeners[name] = __listeners[name] || []).push(cb);
      __calls.push(['listen', name]);
      return Promise.resolve(function () {});
    },
    emit: function (name, p) { __calls.push(['emit', name, p]); return Promise.resolve(); },
  },
};
window.__emit = function (name, payload) {
  (__listeners[name] || []).forEach(function (cb) { cb({ event: name, payload: payload }); });
};
window.fetch = function (url) {
  __calls.push(['fetch', String(url)]);
  return Promise.resolve({ ok: true, status: 200 });
};
window.__report = function (r) { r.calls = __calls; document.title = 'RESULT ' + JSON.stringify(r); };
window.__key = function (key) {
  document.dispatchEvent(new KeyboardEvent('keydown', { key: key, bubbles: true }));
};
"#;

    /// Load `page` with `init` (its payload global) and the stub at document start and `driver`
    /// appended after the page's own script; return what the driver reports.
    fn run_page(mtm: MainThreadMarker, name: &str, page: &str, init: &str, driver: &str) -> Value {
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        let script = unsafe {
            WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(&format!("{init}\n{STUB}")),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
            )
        };
        unsafe { config.userContentController().addUserScript(&script) };
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(600.0, 400.0));
        let webview = unsafe {
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config)
        };
        let html = page.replacen(
            "</body>",
            &format!("<script>\n{driver}\n</script>\n</body>"),
            1,
        );
        assert_ne!(html, page, "{name}: no </body> to append the driver at");
        unsafe { webview.loadHTMLString_baseURL(&NSString::from_str(&html), None) };

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.05));
            let title = unsafe { webview.title() }
                .map(|t| t.to_string())
                .unwrap_or_default();
            if let Some(json) = title.strip_prefix("RESULT ") {
                return serde_json::from_str(json).expect("the driver reports JSON");
            }
            assert!(Instant::now() < deadline, "{name}: the page never reported");
        }
    }

    /// The payload global an app's init script sets: the JSON text as a JS string.
    fn payload(global: &str, v: Value) -> String {
        format!(
            "window.{global} = {};",
            serde_json::to_string(&v.to_string()).unwrap()
        )
    }

    fn calls<'a>(r: &'a Value, kind: &str) -> Vec<&'a Value> {
        r["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c[0] == kind)
            .collect()
    }

    fn fetched(r: &Value) -> Vec<String> {
        calls(r, "fetch")
            .iter()
            .map(|c| c[1].as_str().unwrap().to_string())
            .collect()
    }

    /// The form's reports, decoded from `/result?p=…`.
    fn form_results(r: &Value) -> Vec<Value> {
        fetched(r)
            .iter()
            .map(|u| {
                let query = u.strip_prefix("/result?").expect("a /result request");
                let (_, p) = url::form_urlencoded::parse(query.as_bytes())
                    .find(|(k, _)| k == "p")
                    .expect("a p parameter");
                serde_json::from_str(&p).unwrap()
            })
            .collect()
    }

    fn detach(mtm: MainThreadMarker) {
        let init = payload(
            "__SHELL_DETACH__",
            json!({ "appName": "App", "title": "Docs", "label": "shell-detach:t",
                    "colour": "#ff0000", "loadError": null }),
        );
        let r = run_page(
            mtm,
            "detach",
            DETACH,
            &init,
            r#"
setTimeout(function () {
  var r = {}, err = document.getElementById('error');
  var invokes = function () { return __calls.filter(function (c) { return c[0] === 'invoke'; }).length; };
  r.docTitle = document.title;
  r.banner = document.getElementById('title').textContent;
  r.accent = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim();
  r.hiddenAtStart = err.hidden;
  r.invokesAtStart = invokes();
  __emit('shell-detach:error', { error: 'bad toml' });
  r.shown = !err.hidden;
  r.text = document.getElementById('error-text').textContent;
  r.invokesAfterError = invokes();
  __emit('shell-detach:error', { error: 'bad toml' });
  r.invokesAfterRepeat = invokes();
  __emit('shell-detach:error', { error: null });
  r.hiddenAfterClear = err.hidden;
  var hole = document.getElementById('hole').getBoundingClientRect();
  r.hole = { x: hole.x, y: hole.y, width: hole.width, height: hole.height };
  __report(r);
}, 50);
"#,
        );
        let checks = [
            (
                "title from the payload",
                r["docTitle"] == "Docs" && r["banner"] == "Docs",
            ),
            ("accent from the payload", r["accent"] == "#ff0000"),
            (
                "no error bar without a load error",
                r["hiddenAtStart"] == true,
            ),
            (
                "the hole is reported at load",
                r["invokesAtStart"].as_u64() >= Some(1),
            ),
            (
                "listens for the error event",
                calls(&r, "listen")
                    .iter()
                    .any(|c| c[1] == "shell-detach:error"),
            ),
            (
                "the error event shows the bar",
                r["shown"] == true && r["text"] == "bad toml",
            ),
            (
                "showing the bar re-reports the hole",
                r["invokesAfterError"].as_u64() > r["invokesAtStart"].as_u64(),
            ),
            (
                "the same error again is no change",
                r["invokesAfterRepeat"] == r["invokesAfterError"],
            ),
            (
                "a cleared error hides the bar",
                r["hiddenAfterClear"] == true,
            ),
            (
                "set_hole_rect carries the hole's rect",
                calls(&r, "invoke")
                    .last()
                    .is_some_and(|c| c[1] == "set_hole_rect" && c[2]["rect"] == r["hole"]),
            ),
            (
                "the hole spans the window's width",
                r["hole"]["width"].as_f64() == Some(600.0),
            ),
            ("the page never emits", calls(&r, "emit").is_empty()),
        ];
        verdict("detach", &checks, &r);
    }

    fn form(mtm: MainThreadMarker) {
        let init = payload(
            "__SHELL_WINDOW_FORM__",
            json!({ "mode": "edit", "title": "work", "colour": "#123", "taken": ["work", "side"] }),
        );
        let r = run_page(
            mtm,
            "form",
            FORM,
            &init,
            r#"
setTimeout(function () {
  var r = {}, f = document.getElementById('f');
  var title = document.getElementById('title');
  var save = function () { return f.querySelector('button[type=submit]'); };
  r.saveLabel = save().textContent;
  title.value = 'side'; title.dispatchEvent(new Event('input'));
  r.takenDisables = save().disabled;
  r.takenWhy = f.querySelector('.err').textContent;
  f.requestSubmit();
  title.value = '  job '; title.dispatchEvent(new Event('input'));
  r.freeEnables = !save().disabled;
  f.requestSubmit();
  __emit('shell:window-form-refresh', JSON.stringify({ mode: 'delete', title: 'job', colour: null, taken: [] }));
  r.deleteShown = !!f.querySelector('button.danger');
  f.querySelector('button.danger').click();
  __key('Escape');
  setTimeout(function () { __report(r); }, 50);
}, 50);
"#,
        );
        let results = form_results(&r);
        let checks = [
            ("an edit form saves", r["saveLabel"] == "Save"),
            (
                "a taken title disables Save",
                r["takenDisables"] == true && r["takenWhy"] != "",
            ),
            ("a free title enables Save", r["freeEnables"] == true),
            (
                "Save reports the trimmed title over /result, nothing for the refused one",
                results.first()
                    == Some(&json!({ "kind": "update", "title": "job", "colour": "#123" })),
            ),
            ("refresh re-renders as delete", r["deleteShown"] == true),
            (
                "Delete reports over /result",
                results.get(1) == Some(&json!({ "kind": "delete" })),
            ),
            (
                "Escape cancels over /result",
                results.get(2) == Some(&json!({ "kind": "cancel" })),
            ),
            ("exactly those three reports", results.len() == 3),
            (
                "the form never emits (53a816a)",
                calls(&r, "emit").is_empty(),
            ),
        ];
        verdict("form", &checks, &r);
    }

    fn popup(mtm: MainThreadMarker) {
        let r = run_page(
            mtm,
            "popup",
            POPUP,
            "",
            r#"
setTimeout(function () {
  var r = {}, list = document.getElementById('list'), clear = document.getElementById('clear');
  r.empty = list.textContent;
  r.clearOff = clear.classList.contains('off');
  r.clearWhy = clear.title;
  clear.click();
  r.fetchesBeforeRows = __calls.filter(function (c) { return c[0] === 'fetch'; }).length;
  __emit('shell-downloads:rows', [
    { id: 1, name: 'a.zip', status: 'done', size: 1500 },
    { id: 2, name: 'b.dmg', status: 'downloading', size: null },
  ]);
  var rows = list.querySelectorAll('.row');
  r.rows = rows.length;
  r.size = rows[0].querySelector('.status').textContent;
  rows[0].querySelector('.open').click();
  rows[0].querySelector('button').click();
  rows[1].querySelector('.open').click();
  rows[1].querySelector('button').click();
  r.busyWhy = rows[1].querySelector('.open').title;
  r.busyOff = rows[1].querySelector('button').classList.contains('off');
  clear.click();
  __key('Escape');
  __report(r);
}, 50);
"#,
        );
        let fetched = fetched(&r);
        let ready_at = fetched.iter().position(|u| u == "/ready");
        let listen_at = r["calls"]
            .as_array()
            .unwrap()
            .iter()
            .position(|c| c[0] == "listen" && c[1] == "shell-downloads:rows");
        let fetch_at = r["calls"]
            .as_array()
            .unwrap()
            .iter()
            .position(|c| c[0] == "fetch" && c[1] == "/ready");
        let checks = [
            (
                "no rows shows the empty state",
                r["empty"] == "No downloads",
            ),
            (
                "nothing to clear is off, with its reason",
                r["clearOff"] == true && r["clearWhy"] == "Nothing to clear",
            ),
            (
                "/ready is requested once",
                ready_at.is_some() && fetched.iter().filter(|u| *u == "/ready").count() == 1,
            ),
            (
                "/ready follows the rows listener",
                listen_at.is_some() && fetch_at > listen_at,
            ),
            ("rows render", r["rows"] == 2 && r["size"] == "1.5 KB"),
            (
                "a downloading row explains itself",
                r["busyWhy"] == "Still downloading" && r["busyOff"] == true,
            ),
            (
                "only the done row's controls, Clear and Escape send",
                fetched
                    .iter()
                    .filter(|u| *u != "/ready")
                    .cloned()
                    .collect::<Vec<_>>()
                    == ["/open?id=1", "/reveal?id=1", "/clear", "/close"],
            ),
        ];
        verdict("popup", &checks, &r);
    }

    fn verdict(name: &str, checks: &[(&str, bool)], r: &Value) {
        let failed: Vec<_> = checks
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(n, _)| *n)
            .collect();
        assert!(failed.is_empty(), "{name}: {failed:?} failed: {r}");
        println!("test pages::{name} ... ok");
    }

    pub fn run() {
        let mtm = MainThreadMarker::new().expect("harness = false runs main() on the main thread");
        let _app = NSApplication::sharedApplication(mtm);
        detach(mtm);
        form(mtm);
        popup(mtm);
    }
}
