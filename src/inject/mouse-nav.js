// Injected into content webviews (main frame, before page load) by curator and lector. Maps the
// mouse side-buttons to history navigation: WKWebView delivers them as ordinary DOM mouse events
// (button 3 = back, button 4 = forward) but, unlike Safari, never acts on them, so we drive the
// page's own history. mouseup is the reliable hook for the side buttons. Self-contained — no
// per-webview key, no app state — so it is single-sourced here and shared, not copied per app.
(function () {
  document.addEventListener(
    "mouseup",
    function (e) {
      if (e.button === 3) {
        e.preventDefault();
        e.stopPropagation();
        history.back();
      } else if (e.button === 4) {
        e.preventDefault();
        e.stopPropagation();
        history.forward();
      }
    },
    true
  );
})();
