// Background service worker for m3u8 studio.
//
// * Observes network requests and remembers every HLS playlist a tab loads
//   (works even when the player feeds a <video> from a blob: URL).
// * Forwards URLs to the desktop app's loopback bridge.
// * Provides the toolbar popup with the detected streams and app status.

const BRIDGE = "http://127.0.0.1:37811";
const MAX_PER_TAB = 24;

// ---------------------------------------------------------------- helpers

const looksHls = (url) =>
  typeof url === "string" &&
  /^https?:/i.test(url) &&
  (/\.m3u8?(\?|#|$)/i.test(url) || /mpegurl/i.test(url));

const keyFor = (tabId) => `detected:${tabId}`;

let writeQueue = Promise.resolve();
const serial = (fn) => {
  writeQueue = writeQueue.then(fn, fn);
  return writeQueue;
};

async function getDetected(tabId) {
  const key = keyFor(tabId);
  const data = await chrome.storage.session.get(key);
  return Array.isArray(data[key]) ? data[key] : [];
}

function setBadge(tabId, count) {
  if (tabId == null || tabId < 0) return;
  const text = count <= 0 ? "" : count > 9 ? "9+" : String(count);
  chrome.action.setBadgeText({ tabId, text }).catch(() => {});
  chrome.action.setBadgeBackgroundColor({ tabId, color: "#7C8CFF" }).catch(() => {});
  if (chrome.action.setBadgeTextColor) {
    chrome.action.setBadgeTextColor({ tabId, color: "#0A0D14" }).catch(() => {});
  }
}

async function addDetection(tabId, url, source = "network", titleHint = "") {
  if (tabId < 0 || !looksHls(url)) return;
  await serial(async () => {
    const list = await getDetected(tabId);
    const existing = list.find((item) => item.url === url);
    if (existing) {
      if (titleHint && !existing.title) {
        existing.title = titleHint;
        await chrome.storage.session.set({ [keyFor(tabId)]: list });
      }
      return;
    }
    let title = titleHint || "";
    if (!title) {
      try {
        const tab = await chrome.tabs.get(tabId);
        title = tab && tab.title ? tab.title : "";
      } catch (_) {}
    }
    list.unshift({ url, ts: Date.now(), title, source });
    const trimmed = list.slice(0, MAX_PER_TAB);
    await chrome.storage.session.set({ [keyFor(tabId)]: trimmed });
    setBadge(tabId, trimmed.length);
  });
}

async function scanTab(tabId) {
  try {
    const res = await chrome.tabs.sendMessage(tabId, { type: "rescan" });
    if (res && Array.isArray(res.urls)) return res.urls;
  } catch (_) {}

  // Fallback for pages opened before the extension was installed/reloaded.
  try {
    const injections = await chrome.scripting.executeScript({
      target: { tabId, allFrames: true },
      func: () => {
        const re = /\.m3u8?(\?|#|$)/i;
        const out = new Set();
        document
          .querySelectorAll("a[href], video, audio, source, track, [data-src], [data-url]")
          .forEach((el) => {
            const values = [
              el.currentSrc,
              el.src,
              el.href,
              el.getAttribute && el.getAttribute("src"),
              el.getAttribute && el.getAttribute("href"),
              el.getAttribute && el.getAttribute("data-src"),
              el.getAttribute && el.getAttribute("data-url"),
            ];
            for (const value of values) {
              if (typeof value === "string" && /^https?:/i.test(value) && re.test(value)) {
                out.add(value);
              }
            }
          });
        return [...out];
      },
    });
    const urls = [];
    for (const injection of injections || []) {
      for (const url of (injection && injection.result) || []) urls.push(url);
    }
    return urls;
  } catch (_) {
    return [];
  }
}

async function bridgeFetch(path, options = {}, timeoutMs = 2500) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  const headers = { "X-M3U8-Studio": "1", ...(options.headers || {}) };
  try {
    const res = await fetch(BRIDGE + path, { ...options, headers, signal: controller.signal });
    const body = await res.json().catch(() => ({}));
    return { res, body };
  } finally {
    clearTimeout(timer);
  }
}

async function sendItems(items, pageTitle) {
  const unique = [];
  const seen = new Set();
  for (const item of items || []) {
    const url = (item && item.url ? String(item.url) : "").trim();
    if (!url || seen.has(url) || !/^https?:/i.test(url)) continue;
    seen.add(url);
    unique.push({ url, title: (item.title || "").slice(0, 120) });
  }
  if (!unique.length) return { ok: false, sent: 0, total: 0, error: "no valid URLs" };

  const results = [];
  for (const item of unique) {
    const query = new URLSearchParams({ url: item.url });
    const title = (item.title || pageTitle || "").trim().slice(0, 120);
    if (title) query.set("title", title);
    try {
      const { res, body } = await bridgeFetch("/add?" + query.toString(), { method: "POST" }, 4000);
      if (res.ok && body.ok) {
        results.push({ url: item.url, ok: true });
      } else {
        results.push({
          url: item.url,
          ok: false,
          error: body && body.error ? body.error : `bridge answered HTTP ${res.status}`,
        });
      }
    } catch (e) {
      results.push({
        url: item.url,
        ok: false,
        error: e && e.name === "AbortError" ? "timed out" : "desktop app not reachable",
      });
    }
  }

  const sent = results.filter((r) => r.ok).length;
  return { ok: sent > 0, sent, total: results.length, results };
}

// ------------------------------------------------------------- detection

chrome.webRequest.onBeforeRequest.addListener(
  (details) => {
    if (details.tabId >= 0 && looksHls(details.url)) {
      addDetection(details.tabId, details.url, "network");
    }
  },
  {
    urls: ["*://*/*"],
    types: ["media", "xmlhttprequest", "other", "object", "main_frame"],
  },
  []
);

chrome.tabs.onRemoved.addListener((tabId) => {
  chrome.storage.session.remove(keyFor(tabId)).catch(() => {});
});

chrome.tabs.onActivated.addListener(({ tabId }) => {
  getDetected(tabId)
    .then((items) => setBadge(tabId, items.length))
    .catch(() => {});
});

chrome.tabs.onUpdated.addListener((tabId, info) => {
  if (!info.status) return;
  getDetected(tabId)
    .then((items) => setBadge(tabId, items.length))
    .catch(() => {});
});

// ------------------------------------------------------------- menus

function createMenus() {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create(
      {
        id: "send-stream",
        title: "Send to m3u8 studio",
        contexts: ["link", "video", "audio", "page", "selection"],
      },
      () => void chrome.runtime.lastError
    );
  });
}

chrome.runtime.onInstalled.addListener(createMenus);
chrome.runtime.onStartup.addListener(createMenus);

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId !== "send-stream" || !tab) return;

  const candidates = [
    info.linkUrl,
    info.srcUrl,
    info.selectionText && /^https?:/i.test(info.selectionText.trim())
      ? info.selectionText.trim()
      : "",
    info.pageUrl,
  ].filter(Boolean);

  let url = candidates.find(looksHls);
  if (!url && tab.id != null) {
    const detected = await getDetected(tab.id);
    url = detected.length ? detected[0].url : "";
  }
  if (!url) return;

  await sendItems([{ url, title: tab.title || "" }], tab.title || "");
});

// ------------------------------------------------------------- messaging

chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (!message || !message.type) return;

  if (message.type === "ping") {
    bridgeFetch("/ping")
      .then(({ res, body }) => respond({ ok: res.ok && !!body.ok, app: body.app, version: body.version }))
      .catch(() => respond({ ok: false }));
    return true;
  }

  if (message.type === "status") {
    bridgeFetch("/status")
      .then(({ body }) => respond({ ok: true, data: body }))
      .catch(() => respond({ ok: false }));
    return true;
  }

  if (message.type === "detected") {
    getDetected(message.tabId)
      .then((items) => respond({ ok: true, items }))
      .catch(() => respond({ ok: true, items: [] }));
    return true;
  }

  if (message.type === "scanned") {
    const tabId = sender && sender.tab ? sender.tab.id : -1;
    if (tabId < 0) return false;
    const title = (message.title || "").slice(0, 120);
    const urls = Array.isArray(message.urls) ? message.urls.slice(0, 40) : [];
    Promise.all(urls.map((url) => addDetection(tabId, url, "page", title)))
      .then(() => getDetected(tabId))
      .then((items) => respond({ ok: true, items }))
      .catch(() => respond({ ok: false }));
    return true;
  }

  if (message.type === "rescan-page") {
    const tabId = message.tabId;
    if (tabId == null || tabId < 0) {
      respond({ ok: false, items: [] });
      return false;
    }
    scanTab(tabId)
      .then(async (urls) => {
        const title = (message.title || "").slice(0, 120);
        for (const url of urls.slice(0, 40)) {
          await addDetection(tabId, url, "page", title);
        }
        const items = await getDetected(tabId);
        setBadge(tabId, items.length);
        respond({ ok: true, items });
      })
      .catch(() => respond({ ok: false, items: [] }));
    return true;
  }

  if (message.type === "send") {
    sendItems(message.items, message.title).then(respond);
    return true;
  }

  if (message.type === "clear") {
    chrome.storage.session
      .remove(keyFor(message.tabId))
      .then(() => {
        setBadge(message.tabId, 0);
        respond({ ok: true });
      })
      .catch(() => respond({ ok: false }));
    return true;
  }
});
