// Injected into every content webview (main frame, before load). The shell keeps every
// tab live even when it is a covered background webview — but chat apps (Element)
// throttle their own /sync when they believe document.hidden. Force "always visible/focused"
// so the page never backs off. Belt-and-suspenders behind the native z-order approach.
(function () {
  function alwaysVisible() {
    return "visible";
  }
  function alwaysFalse() {
    return false;
  }
  try {
    Object.defineProperty(document, "visibilityState", { configurable: true, get: alwaysVisible });
    Object.defineProperty(document, "hidden", { configurable: true, get: alwaysFalse });
    Object.defineProperty(document, "webkitVisibilityState", { configurable: true, get: alwaysVisible });
    Object.defineProperty(document, "webkitHidden", { configurable: true, get: alwaysFalse });
  } catch (e) {}
  var swallow = function (e) {
    e.stopImmediatePropagation();
  };
  document.addEventListener("visibilitychange", swallow, true);
  document.addEventListener("webkitvisibilitychange", swallow, true);
  document.hasFocus = function () {
    return true;
  };
})();
