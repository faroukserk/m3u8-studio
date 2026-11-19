# m3u8 studio — browser bridge

A small MV3 extension that finds HLS (`.m3u8`) streams on the page you're
watching and hands them to the **m3u8 studio** desktop app over a loopback
bridge (`http://127.0.0.1:37811`).

## Install

1. Start `m3u8-studio.exe`. It unpacks this folder next to itself on first run
   (the app's sidebar shows the exact path — *Extension setup → Open folder*).
2. Load the folder in your browser:
   - **Chrome / Edge:** `chrome://extensions` → Developer mode → Load unpacked.
   - **Firefox:** `about:debugging#/runtime/this-firefox` → Load Temporary
     Add-on → select `manifest.json` in the folder.
   - **Safari:** Develop → Allow Unsigned Extensions, then Develop → Show
     Extension Inspector → Load Extension → select the folder. For an App Store
     build run `xcrun safari-web-extension-converter <extension folder>`.
3. Pin the extension to the toolbar.

The extension needs no account, no remote server, and no extra permissions
beyond reading page media requests. It uses one MV3 manifest for all browsers;
Chrome 121+ is required because older Chromium refuses the `background.scripts`
key that Firefox and Safari need.

## Use

- Play a video, then click the toolbar icon. Every manifest the page loaded is
  listed, newest first. The badge shows how many streams were found on the tab.
- **Send stream** / **Send all** queues it in the desktop app. With
  *Auto-download new links* enabled (default), the download starts immediately.
- Right-click any link, video, or the page itself → **Send to m3u8 studio**.
- The popup's status pill turns green when the app is running. Streams that are
  already queued show an in-app chip (`Queued`, `Downloading %`, `Saved`).

## How detection works

| Source | What it catches |
| --- | --- |
| `webRequest` observation | Every HLS request the tab makes, including players that use `blob:` video sources |
| DOM scan (`content.js`) | `<a href>`, `<video>`/`<source>`, `data-src` attributes and manifest URLs printed into the page |
| `PerformanceResourceTiming` | Manifests fetched before the content script woke up |

Results are stored per tab in `storage.session` (or `storage.local` on older
engines) and cleared when the tab closes. The `api.js` shim lets the same files
run on Chromium's callback-style `chrome.*` and Firefox/Safari's promise-based
`browser.*`.

There are two manifests:

- `manifest.json` — pure MV3 for **Chrome/Edge**: `background.service_worker`
  only (no `background.scripts` or `persistent`, which Chrome MV3 rejects).
- `manifest.firefox.json` — MV3 for **Firefox/Safari**: `background.scripts`
  event page. The XPI release swaps this in as `manifest.json`.

The desktop app unpacks both files; if you want to load the unpacked folder in
Firefox/Safari, copy `manifest.firefox.json` over `manifest.json` first (or just
install the provided `.xpi`).

## Bridge endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/ping` | `{"ok":true,"app":"m3u8 studio","version":"…"}` |
| `GET` | `/status` | `{"active":n,"pending":n,"autoStart":bool,"items":[…]}` |
| `GET` | `/list` | `{"ok":true,"urls":[…]}` |
| `POST` | `/add?url=…&title=…` | Queue a stream; needs `X-M3U8-Studio: 1`; also accepts a JSON body |

Only loopback connections are accepted; the bridge rejects foreign `Host`
headers. CORS is open so the popup can call it directly.

## Troubleshooting

- **Pill stays red** — start the desktop app; if it's already running, another
  copy may own port `37811` (the app log says so).
- **No streams detected** — start playback first, then hit *Rescan*. Some
  players only request the manifest after a click. DRM-protected streams
  (Widevine/FairPlay) cannot be downloaded.
- **Reloading the extension** — after updating the app, click *Reload* on
  `chrome://extensions`, or re-load the temporary add-on in Firefox/Safari, so
  the bundled files are refreshed. The app writes a fresh copy whenever its
  version changes.
