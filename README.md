# m3u8 studio

A minimal desktop downloader for HLS streams, with a custom **Slint** interface
and a browser extension that sends playlists straight from the page you're
watching.

The app is deliberately small: it resolves the playlist, downloads each segment
(decrypting AES-128 when needed), and appends everything into a single file.
There is no ffmpeg dependency and no re-encoding.

```
playlist → first variant → AES-128 key → segment CBC decrypt → append to output
```

> If this project saved you some time:
>
> [![Support me on Ko-fi](https://img.shields.io/badge/Ko--fi-Support%20me%20on%20Ko--fi-ac73f5?logo=ko-fi&logoColor=white)](https://ko-fi.com/faroukserkini)
if you encounter any issues. just post them in ussues tab!
---

## Highlights

- **Dark, quiet UI** — 270 px settings rail, stream cards with live progress,
  collapsible activity log, toast notifications.
- **Send from clipboard** — a "Send clipboard" button in the header parses the
  first HLS link in the clipboard and queues it.
- **Easier removal** — the trash icon opens a confirmation dialog with an
  optional *Also remove the downloaded file* toggle (pre-checked when a file
  exists on disk).
- **System tray** — minimizing or closing the window hides the app into the
  tray; click the icon or use its menu to show the window or quit.
- **Formats** — MKV, MP4, MOV, TS, WebM (the segments are written as-is; the
  extension mostly names the container you plan to remux into).
- **Master playlists** — picks the first variant automatically.
- **Live streams** — playlists without `#EXT-X-ENDLIST` are re-polled; failed
  streams retry after the auto-restart interval (2–1000 s, default 10 s).
- **Browser bridge** — a loopback HTTP server on `127.0.0.1:37811` lets the
  bundled extension queue links, optionally starting them immediately.
- **Site-friendly download headers** — segment, key and playlist requests send
  browser-like headers (`User-Agent`, `Referer`, `Accept`, `Accept-Language`,
  `Sec-Fetch-*`), so sites that 403 plain downloader agents accept them.
- **Bundled extension** — unpacked next to the executable on first launch; the
  sidebar opens the folder for you and shows whether the bridge is listening.
- **Original names** — downloads are named after the stream/page title,
  sanitized for your filesystem, instead of the playlist URL's file name.
- **Safe removal** — the trash icon opens a confirmation dialog with an
  optional *Also remove the downloaded file* toggle (pre-checked when a file
  exists on disk). Cancel leaves both the task and the file untouched.
- **Settings remembered** — format, destination, restart interval, auto-start,
  and activity-panel state live in `data\settings.txt` inside the app folder.
- **One folder, no dependencies** — the Windows build links the C runtime
  statically. Settings (`data\`), downloads (`downloads\`) and the browser
  extension (`extension\`) all live beside the executable, so the app is fully
  portable: install it, or copy the folder to a USB stick.

## Install on Windows

Download `m3u8-studio-<version>-windows-x86_64-setup.exe` from the
[releases page](https://github.com/faroukserk/m3u8-studio/releases) and run it.
The installer is built with **Inno Setup** and needs no administrator rights:

- Installs to `%LOCALAPPDATA%\Programs\m3u8 studio` (one folder).
- Adds Start Menu entries and an optional desktop shortcut, plus an uninstaller
  in *Settings → Apps*.
- Ships the executable, license and the ready-to-load extension folder. The
  first launch creates `data\` (settings) and refreshes `extension\`; the first
  download creates `downloads\`.
- The executable links the C runtime statically, so no VC++ redistributable is
  required.

The `.zip` build remains fully portable: extract it anywhere writable and the
same single-folder layout is used. Set `M3U8_STUDIO_HOME` to force a different
data folder. If the executable sits in a read-only location, the app falls back
to `%APPDATA%\m3u8-studio`.

## Build and run

Requirements: Rust 1.85+ (edition 2021), a desktop with a GPU/OpenGL driver.

```powershell
cargo build --release
.\target\release\m3u8-studio.exe
```

For development:

```powershell
cargo run
```

The first build compiles Slint, which takes a few minutes. The release profile
uses thin LTO and strips symbols. The Windows target links the C runtime
statically (`.cargo/config.toml`), so the executable has no VC++ redistributable
dependency.

To build the Windows installer, install [Inno Setup](https://jrsoftware.org/isinfo.php)
6.3+ (or 7) and run:

```powershell
cargo build --release
& "$env:LOCALAPPDATA\Programs\Inno Setup 7\ISCC.exe" /DMyAppVersion=1.3.0 installer\m3u8-studio.iss
```

The setup executable is written to the repository root.

## Downloads and releases

Pushing a `v*` tag (for example `v1.3.0`) runs
`.github/workflows/release.yml`, which builds archives for **Windows x86_64**
(`.zip` + Inno Setup `-setup.exe`), **Linux x86_64 (GNU and musl)**,
**Linux aarch64**, **macOS arm64** and **macOS x86_64** (`.tar.gz`) and attaches
them to the GitHub Release.
The musl build is statically linked against OpenSSL and works on Alpine and
older glibc-based distros; the aarch64 build targets 64-bit ARM Linux.
You can also run the workflow manually from the Actions tab; manual runs upload
the archives as build artifacts without creating a release.

Each archive contains the executable, this README, the license and the
extension folder. The app also unpacks its embedded extension next to the
executable on first launch, so keep the binary in a writable folder.

- **Linux:** needs a desktop session with OpenGL and X11 or Wayland at runtime.
- **macOS:** binaries are ad-hoc unsigned. If Gatekeeper blocks one, run
  `xattr -d com.apple.quarantine ./m3u8-studio` after extracting.

## Using the browser extension

1. Start the app. It writes the extension to `<app folder>\extension\` and logs
   the exact path.
2. Load that folder in your browser:
   - **Chrome / Edge:** `chrome://extensions` → Developer mode → Load unpacked.
   - **Firefox:** install the `.xpi` from the release page, or in
     `about:debugging#/runtime/this-firefox` choose Load Temporary Add-on and
     select `manifest.firefox.json` (renamed to `manifest.json` in a copy of the
     folder). The unpacked folder's default `manifest.json` is Chrome MV3.
   - **Safari:** install the `.xpi` equivalent, or Develop → Allow Unsigned
     Extensions → Develop → Show Extension Inspector → Load Extension → select
     the folder after swapping in `manifest.firefox.json`. For an App Store
     build, convert it with
     `xcrun safari-web-extension-converter <extension folder>`.
3. Play a video and click the toolbar icon.

Detected manifests appear newest-first. **Send stream** / **Send all** queue
them; with *Auto-download from browser* enabled (default) the download starts
right away. The popup also shows whether the app is reachable and the state of
streams already queued (`Queued`, `Downloading %`, `Saved`).

Right-clicking a link, video, audio element, or page also offers
**Send to m3u8 studio**.

See [`extension/README.md`](extension/README.md) for the detector internals and
troubleshooting.

## Bridge API

The bridge binds to loopback only and rejects foreign `Host` headers. All
responses include permissive CORS headers.

| Method | Path | Result |
| --- | --- | --- |
| `GET` | `/ping` | `{"ok":true,"app":"m3u8 studio","version":"1.3.2"}` |
| `GET` | `/status` | active/pending counts, auto-start flag, per-stream state |
| `GET` | `/list` | URLs currently in the queue |
| `POST` | `/add?url=…&title=…` | queue a stream; needs `X-M3U8-Studio: 1`; also accepts a JSON body |
| `OPTIONS` | any | CORS preflight |

`/status` items look like:

```json
{
  "url": "https://example.com/stream.m3u8",
  "title": "Example stream",
  "state": "running",
  "progress": 0.42
}
```

States: `queued`, `running`, `done`, `stopped`, `failed`.

## Project layout

```
ui/
  tokens.slint    colours, fonts, icon paths
  models.slint    TaskRow model shared with Rust
  widgets.slint   Label, Glyph, buttons, pills, toggle, stepper, progress, card
  sidebar.slint   settings rail and bridge card
  app.slint       window, header, add bar, task list, activity, setup overlay
src/
  main.rs         app state, task workers, UI wiring
  hls.rs          playlist parsing, AES-128-CBC decryption, download loop
  bridge.rs       loopback HTTP bridge
  extension.rs    unpacks the bundled extension, settings helpers
extension/        MV3 extension (manifest, background, content, popup)
```

## Notes and limits

- **No DRM** — Widevine/FairPlay protected streams won't work; the segments
  remain encrypted end-to-end and playback keys are not available.
- **AES-128 only** — SAMPLE-AES and SAMPLE-AES-CTR are not implemented.
- **Single file** — segments are appended verbatim, so the output is a valid
  MPEG-TS payload even when a different container extension is selected.
  Remux with ffmpeg if a strict MP4/MKV container is required.
- **First variant only** — bandwidth ladders are not probed.
- **Loopback only** — the bridge is not exposed to the network.
