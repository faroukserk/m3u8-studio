# m3u8 studio — browser bridge

A small MV3 extension that finds HLS (`.m3u8`) streams on the page you're
watching and hands them to the **m3u8 studio** desktop app over a loopback
bridge (`http://127.0.0.1:37811`).

## Install

1. Start `m3u8-studio.exe`. It unpacks this folder next to itself on first run
   (the app's sidebar shows the exact path — *Extension setup → Open folder*).
2. Open `chrome://extensions` (or `edge://extensions`).
3. Enable **Developer mode**.
4. Click **Load unpacked** and pick the `extension` folder that opened.
5. Pin the extension to the toolbar.

The extension needs no account, no remote server, and no extra permissions
beyond reading page media requests.

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

Results are stored per tab in `chrome.storage.session` and cleared when the tab
closes.

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
  `chrome://extensions` so the bundled files are refreshed.
