// Injected into every content webview (main frame, before page load). The browser status bar:
// hovering a link shows its destination in a small overlay pinned to the bottom-left of the
// tab, dodging to the bottom-right when the pointer is underneath it. Purely in-page — it signals
// nothing to Rust, so it carries no sentinel key.
//
// The overlay lives in a closed shadow root and is styled only through the CSSOM (`el.style`), so
// the page's stylesheets can't reach it and a strict page `style-src` CSP (which blocks injected
// <style> elements, not CSSOM writes) can't strip it. Built lazily on the first link hover.
(function () {
  var HIDE_DELAY_MS = 120; // bridges the gap between adjacent links without a flicker
  var EDGE = 4;

  var host = null;
  var label = null;
  var hideTimer = null;
  var shownHref = null;
  var onRight = false;

  function setStyles(el, styles) {
    for (var k in styles) el.style.setProperty(k, styles[k], "important");
  }

  function palette() {
    var dark = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
    return dark
      ? { bg: "rgba(40, 40, 40, 0.96)", fg: "#e8e8e8", border: "rgba(255, 255, 255, 0.14)" }
      : { bg: "rgba(246, 246, 246, 0.96)", fg: "#1d1d1f", border: "rgba(0, 0, 0, 0.14)" };
  }

  function ensureOverlay() {
    if (host && host.isConnected) return true;
    var root = document.documentElement;
    if (!root) return false;
    if (!host) {
      host = document.createElement("shell-link-status");
      setStyles(host, {
        all: "initial",
        position: "fixed",
        bottom: EDGE + "px",
        left: EDGE + "px",
        "z-index": "2147483647",
        "pointer-events": "none",
        display: "none",
      });
      var shadow = host.attachShadow({ mode: "closed" });
      label = document.createElement("div");
      setStyles(label, {
        "box-sizing": "border-box",
        "max-width": "min(60vw, calc(100vw - " + 2 * EDGE + "px))",
        padding: "3px 8px",
        "border-radius": "5px",
        "border-width": "0.5px",
        "border-style": "solid",
        font: "12px/16px -apple-system, BlinkMacSystemFont, system-ui, sans-serif",
        "white-space": "nowrap",
        overflow: "hidden",
        "text-overflow": "ellipsis",
        "box-shadow": "0 1px 4px rgba(0, 0, 0, 0.12)",
      });
      shadow.appendChild(label);
    }
    root.appendChild(host);
    return true;
  }

  function linkFrom(e) {
    // composedPath() sees through open shadow roots, where `target` would stop at the host.
    var path = e.composedPath ? e.composedPath() : [e.target];
    for (var i = 0; i < path.length; i++) {
      var el = path[i];
      if (el && (el.tagName === "A" || el.tagName === "AREA") && el.href) return el;
    }
    return null;
  }

  function display(href) {
    if (/^javascript:/i.test(href)) return null;
    try {
      return decodeURI(href);
    } catch (_) {
      return href;
    }
  }

  function place(right) {
    onRight = right;
    host.style.setProperty("left", right ? "auto" : EDGE + "px", "important");
    host.style.setProperty("right", right ? EDGE + "px" : "auto", "important");
  }

  function show(href, x, y) {
    clearTimeout(hideTimer);
    var text = display(href);
    if (text === null) return hide();
    if (!ensureOverlay()) return;
    if (href !== shownHref) {
      var c = palette();
      setStyles(label, { background: c.bg, color: c.fg, "border-color": c.border });
      label.textContent = text;
      shownHref = href;
      host.style.setProperty("display", "block", "important");
      place(false);
    }
    dodge(x, y);
  }

  // Keep the overlay out from under the pointer, as browsers do: it sits bottom-left unless the
  // pointer is over that spot. Decided from the left-hand slot alone, so it can't oscillate when
  // a wide overlay would cover the pointer on both sides.
  function dodge(x, y) {
    var r = host.getBoundingClientRect();
    var overLeftSlot = y >= r.top - EDGE && x <= EDGE + r.width + EDGE;
    if (overLeftSlot !== onRight) place(overLeftSlot);
  }

  function hide() {
    clearTimeout(hideTimer);
    shownHref = null;
    if (host) host.style.setProperty("display", "none", "important");
  }

  function scheduleHide() {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(hide, HIDE_DELAY_MS);
  }

  document.addEventListener(
    "mouseover",
    function (e) {
      var a = linkFrom(e);
      if (a) show(a.href, e.clientX, e.clientY);
      else if (shownHref !== null) scheduleHide();
    },
    true
  );

  document.addEventListener(
    "mousemove",
    function (e) {
      if (shownHref !== null) dodge(e.clientX, e.clientY);
    },
    true
  );

  // Pointer left the page (into the sidebar, off the window) or the page is navigating away.
  document.addEventListener(
    "mouseout",
    function (e) {
      if (!e.relatedTarget) hide();
    },
    true
  );
  window.addEventListener("blur", hide);
  window.addEventListener("pagehide", hide);
})();
