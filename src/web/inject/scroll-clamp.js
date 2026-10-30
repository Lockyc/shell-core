// Injected into every content webview (main frame, before page load). Keeps a scroller that sits
// at its bottom from being dragged up by a re-render.
//
// WebKit clamps a scroller's position the moment any layout sees its content shorter; Chromium
// holds the clamp until the DOM change is done. Chat apps re-render the newest message by removing
// it and inserting a replacement, reading layout in between — in WebKit the list is one message
// short at that read, the position is clamped up by that message's height, and nothing restores it
// when the replacement lands (Google Chat after every send; a whole block of messages on load).
//
// So: when a scroller that was at its bottom ends a DOM-mutating task higher than it started —
// without the user scrolling or the page scrolling it itself — the position is put back, capped to
// the new bottom. Content growing below a pinned scroller never moves it (an infinite feed is left
// alone), and a scroller the user has moved off the bottom is never touched.
(function () {
  var AT_BOTTOM_PX = 2;
  // Wheel/trackpad (incl. momentum) and touch scroll natively, off the page's task loop, so a
  // mutation can land mid-gesture: hold off while one is recent.
  var INPUT_GRACE_MS = 400;
  var NAV_KEYS = { PageUp: 1, PageDown: 1, Home: 1, End: 1, ArrowUp: 1, ArrowDown: 1, " ": 1 };

  var lastInput = -Infinity;
  var buttonDown = false; // a scrollbar drag
  var scripted = false; // the page scrolled something itself in the current task
  var known = new Map(); // scroller -> { top, pinned } as of its last scroll or scripted scroll

  var desc = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTop");
  if (!desc || !desc.get || !desc.set || !window.MutationObserver) return;
  var getTop = desc.get;
  var setTop = desc.set;

  function record(el) {
    var top = getTop.call(el);
    known.set(el, { top: top, pinned: top >= el.scrollHeight - el.clientHeight - AT_BOTTOM_PX });
  }

  function markScripted() {
    if (scripted) return;
    scripted = true;
    // After the task and its microtasks — so the mutation check below still sees the flag.
    setTimeout(function () {
      scripted = false;
    }, 0);
  }

  function now() {
    return performance.now();
  }
  function onInput() {
    lastInput = now();
  }
  window.addEventListener("wheel", onInput, { capture: true, passive: true });
  window.addEventListener("touchmove", onInput, { capture: true, passive: true });
  window.addEventListener(
    "keydown",
    function (e) {
      if (NAV_KEYS[e.key]) onInput();
    },
    true
  );
  window.addEventListener("mousedown", function () {
    buttonDown = true;
  }, true);
  window.addEventListener("mouseup", function () {
    buttonDown = false;
    onInput();
  }, true);
  window.addEventListener("blur", function () {
    buttonDown = false;
  });

  document.addEventListener(
    "scroll",
    function (e) {
      // Element scrollers only: a scroll of the document itself targets the Document node.
      if (e.target && e.target.nodeType === 1) record(e.target);
    },
    { capture: true, passive: true }
  );

  // The page's own scrolls are deliberate: note them, and keep the known position current.
  Object.defineProperty(Element.prototype, "scrollTop", {
    configurable: true,
    enumerable: desc.enumerable,
    get: getTop,
    set: function (v) {
      setTop.call(this, v);
      markScripted();
      record(this);
    },
  });
  [
    [Element.prototype, "scrollTo"],
    [Element.prototype, "scroll"],
    [Element.prototype, "scrollBy"],
    [Element.prototype, "scrollIntoView"],
    [Element.prototype, "scrollIntoViewIfNeeded"],
    [HTMLElement.prototype, "focus"],
  ].forEach(function (p) {
    var orig = p[0][p[1]];
    if (typeof orig !== "function") return;
    p[0][p[1]] = function () {
      markScripted();
      var r = orig.apply(this, arguments);
      known.forEach(function (_, el) {
        record(el);
      });
      return r;
    };
  });

  new MutationObserver(function () {
    if (scripted || buttonDown || now() - lastInput < INPUT_GRACE_MS) return;
    known.forEach(function (s, el) {
      if (!el.isConnected) {
        known.delete(el);
        return;
      }
      if (!s.pinned) return;
      var max = el.scrollHeight - el.clientHeight;
      var top = getTop.call(el);
      if (top < s.top - 1 && top < max - AT_BOTTOM_PX) setTop.call(el, Math.min(s.top, max));
      record(el);
    });
  }).observe(document, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["class", "style", "hidden"],
  });
})();
