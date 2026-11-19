// Popup controller for m3u8 studio.

const $ = (id) => document.getElementById(id);

const pill = $("pill");
const pillText = $("pillText");
const pageHost = $("pageHost");
const appSummary = $("appSummary");
const sendBest = $("sendBest");
const sendBestText = $("sendBestText");
const rescan = $("rescan");
const sendAll = $("sendAll");
const count = $("count");
const list = $("list");
const empty = $("empty");
const offline = $("offline");
const retry = $("retry");
const footerNote = $("footerNote");

let tab = null;
let items = [];
let online = false;
let appItems = new Map(); // url -> { state, progress }
let busy = false;
let pollTimer = 0;

// --------------------------------------------------------------- utilities

async function message(payload) {
  try {
    // Chromium and Firefox/Safari both return a promise when no callback is
    // passed (MV3 chrome.* promises / browser.* promises).
    return (await EXT.runtime.sendMessage(payload)) || null;
  } catch (_) {
    return null;
  }
}

function hostOf(url) {
  try {
    return new URL(url).host.replace(/^www\./, "");
  } catch (_) {
    return url;
  }
}

function titleOf(item) {
  const explicit = (item && item.title ? item.title : "").trim();
  if (explicit) return explicit;
  try {
    const parsed = new URL(item.url);
    const last = parsed.pathname.split("/").filter(Boolean).pop() || parsed.host;
    const stem = last.replace(/\.m3u8?$/i, "").replace(/[_-]+/g, " ").trim();
    return stem || parsed.host;
  } catch (_) {
    return item.url;
  }
}

function appStateOf(url) {
  return appItems.get(url) || null;
}

function chipLabel(entry) {
  if (!entry) return "";
  if (entry.state === "running") {
    const pct = Math.round((entry.progress || 0) * 100);
    return pct > 0 ? `Downloading ${pct}%` : "Downloading";
  }
  if (entry.state === "done") return "Saved";
  if (entry.state === "failed") return "Failed";
  if (entry.state === "stopped") return "Stopped";
  return "Queued";
}

function note(text) {
  footerNote.textContent = text;
  clearTimeout(note._timer);
  note._timer = setTimeout(() => {
    footerNote.textContent = online ? "Loopback only" : "App offline";
  }, 4000);
}

// ------------------------------------------------------------------ status

function setPill(mode, text) {
  pill.className = `pill ${mode}`;
  pillText.textContent = text;
}

async function refreshStatus() {
  const result = await message({ type: "status" });
  const data = result && result.ok ? result.data : null;
  online = !!(data && data.ok);
  appItems = new Map();

  if (online) {
    for (const item of data.items || []) {
      if (item && item.url) appItems.set(item.url, item);
    }
    setPill("online", "Connected");
    const active = data.active || 0;
    const pending = data.pending || 0;
    const parts = [];
    if (active) parts.push(`${active} downloading`);
    if (pending) parts.push(`${pending} queued`);
    appSummary.textContent = parts.length ? parts.join(" · ") : "App idle";
    offline.classList.add("hidden");
    document.body.classList.remove("offline");
    if (!note._timer) footerNote.textContent = "Loopback only";
  } else {
    setPill("offline", "Offline");
    appSummary.textContent = "";
    offline.classList.remove("hidden");
    document.body.classList.add("offline");
    if (!note._timer) footerNote.textContent = "App offline";
  }

  diffRender();
}

// ----------------------------------------------------------------- render

const renderKey = () =>
  items
    .map((item) => `${item.url}|${appStateOf(item.url) ? chipLabel(appStateOf(item.url)) : ""}`)
    .join("\n");

let lastRenderKey = null;

function diffRender() {
  const key = renderKey() + (online ? "|on" : "|off");
  if (key === lastRenderKey) return;
  lastRenderKey = key;
  render();
}

function render() {
  count.textContent = String(items.length);
  const idle = items.length === 0;
  empty.classList.toggle("visible", idle);
  list.style.display = idle ? "none" : "flex";

  const unsent = items.filter((item) => !appStateOf(item.url) || appStateOf(item.url).state === "failed");
  sendBest.disabled = idle || busy || !unsent.length;
  sendAll.disabled = idle || busy || !unsent.length;
  sendBestText.textContent = items.length > 1 ? "Send best stream" : "Send stream";

  list.textContent = "";

  for (const item of items) {
    const row = document.createElement("div");
    row.className = "stream";
    row.setAttribute("role", "listitem");

    const main = document.createElement("div");
    main.className = "stream-main";

    const title = document.createElement("div");
    title.className = "stream-title";
    title.textContent = titleOf(item);
    title.title = item.url;

    const meta = document.createElement("div");
    meta.className = "stream-meta";

    const host = document.createElement("span");
    host.className = "stream-host";
    host.textContent = hostOf(item.url);

    const src = document.createElement("span");
    src.className = `src${item.source === "page" ? " page" : ""}`;
    src.textContent = item.source === "page" ? "page" : "net";

    meta.append(src, host);
    main.append(title, meta);
    row.append(main);

    const entry = appStateOf(item.url);
    if (entry) {
      const chip = document.createElement("span");
      chip.className = `chip${entry.state === "queued" ? " working" : ""}`;
      chip.textContent = chipLabel(entry);
      chip.title = `Desktop app status: ${entry.state}`;
      row.append(chip);
    }

    const button = document.createElement("button");
    button.className = "send";
    const done = entry && entry.state !== "failed" && entry.state !== "stopped";
    button.textContent = done ? "Sent" : "Send";
    button.disabled = done || busy;
    button.addEventListener("click", () => sendOne(item, button));
    row.append(button);

    list.append(row);
  }
}

// ------------------------------------------------------------------ sending

async function sendOne(item, button) {
  if (busy) return;
  busy = true;
  if (button) {
    button.disabled = true;
    button.textContent = "…";
  }
  render();
  const result = await message({ type: "send", items: [item], title: item.title || "" });
  busy = false;

  if (result && result.ok) {
    appItems.set(item.url, { state: "queued", progress: 0 });
    lastRenderKey = null;
    note("Sent to the desktop app");
    refreshStatus();
  } else {
    const error = result && result.results && result.results[0] && result.results[0].error;
    note(error ? `Could not send: ${error}` : "Could not reach the app");
    render();
  }
}

async function sendMany(source) {
  if (busy) return;
  const queue = items.filter((item) => {
    const entry = appStateOf(item.url);
    return !entry || entry.state === "failed" || entry.state === "stopped";
  });
  if (!queue.length) {
    note("Everything is already in the app");
    return;
  }
  busy = true;
  render();

  const result = await message({ type: "send", items: queue, title: tab ? tab.title || "" : "" });
  busy = false;

  if (result && result.ok) {
    for (const row of result.results || []) {
      if (row.ok) appItems.set(row.url, { state: "queued", progress: 0 });
    }
    note(`Sent ${result.sent} of ${result.total} stream${result.total === 1 ? "" : "s"}`);
  } else {
    const error =
      result && result.results && result.results.find((r) => !r.ok && r.error)?.error;
    note(error ? `Could not send: ${error}` : "Could not reach the app");
  }

  lastRenderKey = null;
  refreshStatus();
}

// ------------------------------------------------------------------ rescan

async function rescanPage() {
  if (!tab || tab.id == null) return;
  rescan.classList.add("spinning");
  rescan.disabled = true;
  const result = await message({
    type: "rescan-page",
    tabId: tab.id,
    title: tab.title || "",
  });
  rescan.classList.remove("spinning");
  rescan.disabled = false;

  if (result && result.ok && Array.isArray(result.items)) {
    items = result.items;
    lastRenderKey = null;
    render();
    note(items.length ? `Found ${items.length} stream${items.length === 1 ? "" : "s"}` : "No streams found yet");
  } else {
    note("Could not scan this page");
  }
}

// -------------------------------------------------------------------- boot

async function loadDetected() {
  if (!tab || tab.id == null) return;
  const result = await message({ type: "detected", tabId: tab.id });
  items = result && result.ok && Array.isArray(result.items) ? result.items : [];
  lastRenderKey = null;
  render();
}

async function boot() {
  const tabs = await EXT.tabs.query({ active: true, currentWindow: true });
  tab = tabs && tabs[0] ? tabs[0] : null;

  if (tab) {
    pageHost.textContent = hostOf(tab.url || "");
    pageHost.title = tab.url || "";
  } else {
    pageHost.textContent = "no active tab";
  }

  if (tab && !/^https?:/i.test(tab.url || "")) {
    // Chrome pages can't be scanned; show the empty state without noise.
    pageHost.textContent = "unsupported page";
  }

  setPill("checking", "Checking");
  render();
  await Promise.all([refreshStatus(), loadDetected()]);

  pollTimer = setInterval(() => {
    refreshStatus();
    if (!busy) loadDetected();
  }, 2000);
}

sendBest.addEventListener("click", () => {
  const item = items.find((entry) => {
    const state = appStateOf(entry.url);
    return !state || state.state === "failed" || state.state === "stopped";
  });
  if (item) sendOne(item);
});

sendAll.addEventListener("click", () => sendMany());
rescan.addEventListener("click", rescanPage);
$("emptyRescan").addEventListener("click", rescanPage);
retry.addEventListener("click", () => {
  setPill("checking", "Checking");
  refreshStatus();
});

window.addEventListener("unload", () => clearInterval(pollTimer));

boot();
