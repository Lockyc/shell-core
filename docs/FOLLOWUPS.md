---
type: roadmap
links:
  - rel: part-of
    to: CLAUDE.md
---

# Follow-ups

- **A page can read the sentinel key.** `escape-click.js`'s `escapeTo` carries the key in a
  main-frame `location.href` navigation, and this Mac's WKWebView implements the Navigation API: a
  page on an https origin that registers `navigation.addEventListener('navigate', …)` receives
  `destination.url`, `k=` included, on the first cross-host click. With it the page can forge any
  key-verified sentinel for that tab (an app's notify and badge sentinels, browser escapes), so
  `sentinel_key_ok` doesn't stop a hostile page. Measured in a `tests/pages.rs` case loading a
  blank page with an `https://` base URL; under `loadHTMLString` with no base URL the API stays
  inactive and the case passes vacuously. Unlock: a channel the page can't observe — the click
  script in an isolated `WKContentWorld` posting to a script-message handler — wired in shell-core
  and each consuming app's content webviews, then that test case asserting the key never reaches
  a page listener.
