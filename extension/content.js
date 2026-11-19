// Content script for m3u8 studio.
//
// Watches the page for HLS manifest URLs in anchors, <video>/<source> tags,
// visible text and PerformanceResourceTiming entries, and reports them to the
// background worker. Runs in every frame so iframe players are covered too.

(() => {
  const HLS_RE = /\.m3u8?(\?|#|$)/i;
  const looksHls = (value) => {
    if (typeof value !== "string" || !/^https?:/i.test(value)) return false;
    return HLS_RE.test(value) || /mpegurl/i.test(value);
  };

  const absolute = (value) => {
    try {
      return new URL(value, location.href).href;
    } catch (_) {
      return "";
    }
  };

  function scanDom() {
    const found = new Set();

    document.querySelectorAll("a[href]").forEach((node) => {
      const url = absolute(node.getAttribute("href"));
      if (looksHls(url)) found.add(url);
    });

    document.querySelectorAll("video, audio, source, track").forEach((node) => {
      [node.currentSrc, node.src, node.getAttribute("src")].forEach((value) => {
        const url = absolute(value || "");
        if (looksHls(url)) found.add(url);
      });

      // <source> children that a player injected without a src attribute
      node.querySelectorAll?.("source[src]").forEach((source) => {
        const url = absolute(source.getAttribute("src"));
        if (looksHls(url)) found.add(url);
      });
    });

    document.querySelectorAll("[data-src], [data-url], [data-video]").forEach((node) => {
      for (const attr of ["data-src", "data-url", "data-video"]) {
        const url = absolute(node.getAttribute(attr) || "");
        if (looksHls(url)) found.add(url);
      }
    });

    // Inline text (players sometimes print the manifest URL into the DOM).
    const walker = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT);
    let node;
    let budget = 4000;
    while ((node = walker.nextNode()) && budget-- > 0) {
      const text = node.textContent || "";
      if (text.indexOf(".m3u8") === -1 && text.toLowerCase().indexOf("mpegurl") === -1) continue;
      const matches = text.match(/https?:\/\/[^\s"'<>\\]+/gi);
      if (!matches) continue;
      for (const match of matches) {
        const url = absolute(match);
        if (looksHls(url)) found.add(url);
      }
    }

    return [...found];
  }

  function scanPerformance() {
    const found = new Set();
    try {
      for (const entry of performance.getEntriesByType("resource")) {
        if (looksHls(entry.name)) found.add(entry.name);
      }
    } catch (_) {}
    return [...found];
  }

  let lastPayload = "";
  let timer = 0;

  function report() {
    const urls = [...new Set([...scanDom(), ...scanPerformance()])].slice(0, 40);
    if (!urls.length) return;

    const signature = urls.slice().sort().join("\n");
    if (signature === lastPayload) return;
    lastPayload = signature;

    chrome.runtime.sendMessage({
      type: "scanned",
      urls,
      title: document.title || "",
      pageUrl: location.href,
    }).catch(() => {});
  }

  function scheduleReport() {
    clearTimeout(timer);
    timer = setTimeout(report, 450);
  }

  // A player may swap the manifest on quality change, so re-scan on playback.
  document.addEventListener("play", scheduleReport, true);
  document.addEventListener("loadedmetadata", scheduleReport, true);

  const observer = new MutationObserver((mutations) => {
    for (const mutation of mutations) {
      if (mutation.addedNodes.length || mutation.type === "attributes") {
        scheduleReport();
        return;
      }
    }
  });

  const start = () => {
    try {
      observer.observe(document.documentElement, {
        childList: true,
        subtree: true,
        attributes: true,
        attributeFilter: ["src", "data-src", "data-url", "href"],
      });
    } catch (_) {}
    report();
    // Belt and braces: network requests often finish after document_idle.
    setTimeout(report, 1500);
    setTimeout(report, 5000);
  };

  chrome.runtime.onMessage.addListener((message, _sender, respond) => {
    if (message && message.type === "rescan") {
      lastPayload = "";
      const urls = [...new Set([...scanDom(), ...scanPerformance()])].slice(0, 40);
      respond({ ok: true, urls, title: document.title || "" });
      report();
      return true;
    }
    return false;
  });

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start, { once: true });
  } else {
    start();
  }
})();
