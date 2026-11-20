# MECHANICS — m3u8down.exe (original) and this rebuild

Target analyzed: `<local>\m3u8-downloader-portable\m3u8down.exe`
- PE32, 6,189,280 bytes, sha256 `400c9ac075285355fb63b6bebb1e8d5455afe7873729d37583a21036d328a008`
- Delphi VCL app ("M3U8 Downloader", VOVSOFT, package `com.vovsoft.m3u8down`), 15,406 functions, 25 libraries, no packer. Branding is NOT copied into this rebuild.

All addresses below are virtual addresses in the original exe unless marked otherwise.
Items marked **[estimate]** are derived/inferred, not read directly from a disassembly of that exact behavior.

## 1. How the original downloads "hard encrypted" m3u8 (adopted model)

Observed facts (evidence in the investigation record):
- Networking = WinHTTP (19 imports; IAT slots 0x8d5ff0–0x8d6030, thunks 0x6f3eb0–0x6f3f40; WinHttpOpen slot 0x8d6004/thunk 0x6f3ed8, WinHttpSendRequest slot 0x8d6018/thunk 0x6f3f18) plus Indy strings (`IdHTTP`, `IdThread`, …); worker threads via TThread/CreateThread. [fnd_2]
- MD5 is implemented fully in-app: transform `sub_792688` @0x792688 (114 blocks; MD5 F/G round functions in pseudocode; K[16]=0x5a827999 immediate; rotate helper `sub_74bbb0` called from 24 sites inside it); 64-entry MD5 K table at 0x88bbf0–0x88bcef in .data (K[0]=0xd76aa478 … K[63]=0xeb86d391), referenced from code at 0x792f98 (pointer bytes `f0 bb 88 00`). Wrappers `sub_7927e4` / `sub_792cf0` / `sub_792910` (called from 0x793ad0, 0x799600, 0x7eb61c, 0x7eb6a8); the last caller sits inside the big download worker `sub_7ec920` (149 blocks, 195 callees). [fnd_4]
- AES S-box / inverse-S-box / Rcon / te0 table bytes are ALL ABSENT from stored bytes (patterns `63 7c 77 7b f2 6b 6f f5`, `52 09 6a d5`, `01 02 04 08`, `c6 63 63 a5` = 0 matches). `1b 1b 1b 1b` has 60 matches but only 2 in .text (0x6068b5, 0x8646b9), not proven to be AES xtime. advapi32 delay-loads only Cert*/Reg*/SystemFunction* — **no Crypt* imports**. [fnd_4/fnd_6, unk_8dae3b97c9e8d2a1 resolved]
- ALL HLS tag literals are absent in both utf8 and utf16le (#EXTM3U, #EXT-X-KEY, #EXT-X, EXTINF, METHOD=, URI=, IV=, AES-128, TARGETDURATION, MEDIA-SEQUENCE, .ts, .key, .m3u8, EXT- = 0 matches; only fragments: utf8 `METHOD`×10 near 0x776e1f–0x779a2a, `M3U8`×10, utf16le `EXT`×5, `IV`×2, `URI`×1 @0x6bb738, `m3u8`×4, `AES`×1 @0x73ecc0, `decrypt`×6, `encrypt`×10). → tags are built char-by-char at runtime (typical Delphi). [fnd_5]
- Parser/download cluster: 0x716c24–0x717d4c (sub_716c24 296 blocks, sub_717104, sub_7171c0, sub_717338, sub_717690, sub_717974 with 87 incoming refs, sub_7179d8, sub_717a14, sub_717be8, sub_717c94, sub_717d4c), sub_71c337, dispatcher `sub_71d444` (1 block, 50 internal calls, 16 callers), worker `sub_7ec920`. [unk_e7fcf9a749c5a156 resolved]
- HTTP wrappers: session `sub_6f6ba4` (WinHttpOpen + WinHttpSetStatusCallback(`sub_6f6a10`, 0x10010 = ALL_NOTIFICATIONS)); lazy 2nd session `sub_6f8eec` → global 0x8a8900; per-request `sub_6f7c24` (WinHttpConnect + WinHttpOpenRequest flag 0x800000 = WINHTTP_FLAG_SECURE + SetTimeouts) → `sub_6f684c`; headers `sub_6f7e70` (WinHttpAddRequestHeaders); send/recv `sub_6f6e90` (SetOption 0x2d & 0x3f, WinHttpSendRequest, WinHttpWriteData loop, WinHttpReceiveResponse; 0 direct callers → pointer-called); response headers `sub_6f6888` (WinHttpQueryHeaders, errors 0x2f76 / 0x7a); body read `sub_6f7e0c` / `sub_6f7720`; orchestrator `sub_6f8444` → `sub_6f75e0`. [unk_6b7151deb05f3f4c]
- Stored strings: MIME `m3u8` / `application/vnd.apple.mpegurl` / `application/x-mpegURL` @0x6dbcf8; `http://` @0x6bb420; hex charsets `0123456789ABCDEF` utf8 @0x6ae170, lowercase @0x88bbe0, `fedcba9876543210` @0x88bbf1; `MD5` utf16 @0x776b00 (+4 more sites incl. 0x7ee1e2 "MD5 generate problem!"); single `decrypt` string.

**Adopted decryption model for the rebuild [derived — the exact original routine is not statically provable; static analysis never shows code ran]:** spec-compliant HLS AES-128 (what every HLS player uses, so the rebuild interoperates with real encrypted streams):
1. GET the playlist (follow redirects). If it contains `#EXT-X-STREAM-INF` (master), follow the first variant URI.
2. Parse `#EXT-X-KEY:METHOD=AES-128,URI="…",IV=0x…`, `#EXT-X-MEDIA-SEQUENCE:n`, `#EXT-X-ENDLIST`, segment URI lines. Resolve relative URIs against the playlist URL.
3. Fetch the key from `URI=` — must be exactly 16 bytes.
4. Per segment: GET bytes → AES-128-CBC decrypt with IV = hex-decoded `IV=` attribute if present, else the segment's media sequence number (base `#EXT-X-MEDIA-SEQUENCE` + index) as a 128-bit **big-endian** value → strip PKCS#7 padding → append plaintext to the single output file.
5. Output extension from the format radio: MKV / MP4 / MOV / TS / WebM. One output file per URL, segments appended directly (no ffmpeg). [unk_e2344ed56909a678 — rebuild decision]
6. Live playlists (no `#EXT-X-ENDLIST`): re-poll every N seconds, N = auto-restart setting; download only new segments. On failure, restart after N seconds. One worker per URL, sequential segments. [unk_d141ee33c92daaad]

## 2. UI constants (observed in DFM resources, fnd_1)

- Main window toolbar: Add M3U8 / Download / Feedback / About.
- URL ListView columns: `M3U8`, `Status`.
- Output format radio buttons: **MKV, MP4, MOV, TS, WebM**.
- Destination folder edit + Browse button.
- Progress bar + status bar.
- "Automatically Restart Download" SpinEdit: **min 2, max 1000, default 10** (seconds). `TimerChecker` timer drives re-poll/restart.
- Save dialog filter: `Video Files (*.mp4;*.mkv;*.avi;*.mpg;*.mpeg;*.mov;*.wmv;*.flv)`.
- Add-URL dialog placeholder: `https://test.com/video.m3u8`.
- Forms: TAppForm, TFormAddM3U8, TNewVer, TSourceForm, TTranslateF…; DFM blobs ~0x94397c and 0x9a4700.

## 3. Rebuild implementation notes (Rust + Slint, in this folder)

- HTTP: `reqwest` blocking with redirect following. Crypto: `aes` + `cbc` crates (AES-128-CBC), `hex` for IV parsing, `url` for relative-URL resolution. No `md-5` is needed for spec streams.
- UI: Slint (`ui/*.slint`) with a custom dark theme — no `std-widgets` chrome except the URL `LineEdit` and the two `ScrollView`s. Layout: a 286 px settings rail (format pills, destination, auto-restart stepper, auto-download toggle, bridge status) plus the download header, add-URL bar, task cards, collapsible activity log, setup overlay and toast.
- Worker: one `std::thread` per URL with its own `Arc<AtomicBool>` stop flag. Progress lives in atomics (`done`, `total`, `bytes`, `speed`); the human status string stays in `Arc<Mutex<String>>` shared with `hls::download_stream`. A 250 ms `slint::Timer` drives row updates, speed calculation, bridge draining and the toast.
- Bridge: `src/bridge.rs` listens on `127.0.0.1:37811`, checks the Host header, rejects http(s) `Origin`s, and requires `X-M3U8-Studio: 1` for `POST /add`. The app publishes its queue to `/status`, which the extension popup reads.
- Downloads: `src/hls.rs` sends a browser-like `User-Agent` and the playlist URL as `Referer` for the manifest, key and segments (same as browsers), and the client keeps cookies between requests. A 403 to a non-browser agent is the most common cause; if it persists, right-click a 403 stream link in the tab and use **Send to m3u8 studio** (or paste the URL in the app), because signed links from many CDNs expire quickly.
- Extension: `extension/` is an MV3 add-on. It observes `webRequest` traffic, scans the DOM and `PerformanceResourceTiming` in every frame, stores detections per tab in `chrome.storage.session`, and pushes them to the bridge. Files are embedded with `include_str!`/`include_bytes!` and unpacked to `<app folder>/extension/` by `src/extension.rs`.
- Settings: `<app folder>\data\settings.txt` (format, destination, restart
  seconds, auto-start, activity panel visibility). The folder is the executable's
  directory when writable (portable / per-user install), otherwise
  `%APPDATA%\m3u8-studio`; old settings are migrated on first launch.
- Packaging: `installer/m3u8-studio.iss` builds a per-user Inno Setup installer
  (no admin, no VC++ redistributable: the Windows target links the CRT
  statically). The installer and the portable zip share the same single-folder
  layout: executable, `extension\`, `data\`, `downloads\`.
- Dropped from the old egui build: `--screenshot`, `--package-source`, `--verify-zip`, `src/package.rs`, `src/verifyzip.rs` and the `image` dependency.
