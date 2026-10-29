// Injected into every content webview (main frame, before page load). WKWebView routes
// target="_blank" / window.open through the native on_new_window handler, but every other link
// click arrives only as an ordinary main-frame navigation — indistinguishable, natively, from the
// page's own redirects. This catches the clicks and reroutes them through a sentinel URL the
// native on_navigation handler recognises: cmd/middle-click always escapes to the default
// browser; a plain click on a link to another host is routed (tab's own site stays in the tab,
// anything else goes to the browser) — so no command is ever exposed to remote pages.
(function () {
  var SENTINEL = "https://shell.escape.invalid/?u=";
  // Per-webview secret, substituted in by Rust at injection (never exposed on window). The
  // native on_navigation handler honours the sentinel only when it carries this key, so a page
  // can't forge an escape by navigating to the host directly.
  var KEY = "__SHELL_KEY__";

  function anchorFrom(target) {
    var el = target;
    while (el && el.tagName !== "A") el = el.parentElement;
    return el && el.href ? el : null;
  }

  function isHttp(href) {
    return /^https?:\/\//i.test(href);
  }

  function escapeTo(href, routed) {
    window.location.href =
      SENTINEL + encodeURIComponent(href) + (routed ? "&r=1" : "") + "&k=" + KEY;
  }

  // Plain click on a link to another host → let Rust route it. Bubble phase on window, after
  // the page's own handlers: a click the page already handled (an SPA route, a window.open)
  // is left alone. Same-host links, new-window targets and downloads keep their native path.
  window.addEventListener("click", function (e) {
    if (e.defaultPrevented || e.button !== 0) return;
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    var a = anchorFrom(e.target);
    if (!a || !isHttp(a.href) || a.hasAttribute("download")) return;
    var t = (a.getAttribute("target") || "").toLowerCase();
    if (t && t !== "_self" && t !== "_top" && t !== "_parent") return;
    if (new URL(a.href).host === location.host) return;
    e.preventDefault();
    escapeTo(a.href, true);
  });

  // cmd-click ("open elsewhere" muscle memory) → escape.
  document.addEventListener(
    "click",
    function (e) {
      if (!e.metaKey) return;
      var a = anchorFrom(e.target);
      if (!a || !isHttp(a.href)) return;
      e.preventDefault();
      e.stopPropagation();
      escapeTo(a.href);
    },
    true
  );

  // middle-click → escape. mousedown(button===1) is the earliest hook and the best chance
  // to suppress WKWebView's own navigation before it starts.
  document.addEventListener(
    "mousedown",
    function (e) {
      if (e.button !== 1) return;
      var a = anchorFrom(e.target);
      if (!a || !isHttp(a.href)) return;
      e.preventDefault();
      e.stopPropagation();
      escapeTo(a.href);
    },
    true
  );

  // Belt-and-suspenders: cancel any default middle-click action that still slips through.
  document.addEventListener(
    "auxclick",
    function (e) {
      if (e.button !== 1) return;
      var a = anchorFrom(e.target);
      if (!a || !isHttp(a.href)) return;
      e.preventDefault();
      e.stopPropagation();
    },
    true
  );
})();
