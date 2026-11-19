// Cross-browser extension API shim.
// Firefox and Safari expose promise-based `browser.*`; Chromium exposes
// `chrome.*` (which also returns promises in MV3 when no callback is passed).
// Everything else in the extension uses `EXT` instead of touching either one.
var EXT = (function () {
  var root =
    typeof browser !== "undefined" && browser && browser.runtime
      ? browser
      : typeof chrome !== "undefined" && chrome
        ? chrome
        : {};

  var storage = root.storage || {};

  return {
    runtime: root.runtime,
    tabs: root.tabs,
    action: root.action,
    scripting: root.scripting,
    webRequest: root.webRequest,
    contextMenus: root.contextMenus,
    storage: storage,
    // session storage is not available in every engine/version; fall back to
    // local storage and clear it on browser startup so behaviour stays similar.
    session: storage.session || storage.local,
    useLocalFallback: !storage.session,
    engine:
      typeof browser !== "undefined" && browser && browser.runtime
        ? "firefox-or-safari"
        : "chromium",
  };
})();
