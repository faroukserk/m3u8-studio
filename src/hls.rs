//! HLS (m3u8) download engine — spec-compliant AES-128 handling.
//!
//! Model adopted from the investigation of the original m3u8down.exe
//! (see ../MECHANICS.md): fetch playlist -> follow master variant if any ->
//! parse #EXT-X-KEY (METHOD=AES-128, URI, optional IV=0x...) -> fetch the
//! 16-byte key -> per segment AES-128-CBC decrypt (IV = tag IV, else media
//! sequence number as 128-bit big-endian) -> strip PKCS#7 -> append to one
//! output file. Live playlists are re-polled every N seconds.

use aes::Aes128;
use cbc::cipher::{BlockDecryptMut, KeyIvInit};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

type Aes128CbcDec = cbc::Decryptor<Aes128>;

#[derive(Debug, Clone)]
pub struct KeyInfo {
    pub method: String,
    pub uri: String,
    pub iv: Option<[u8; 16]>,
}

#[derive(Debug, Clone, Default)]
pub struct Playlist {
    pub is_master: bool,
    pub variant: Option<String>,
    pub media_sequence: u64,
    pub endlist: bool,
    pub key: Option<KeyInfo>,
    pub segments: Vec<String>,
}

/// Browser User-Agent. Some sites return 403 to "downloader" agents and serve
/// fine to browsers, so downloads send a browser-like agent by default.
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

/// `Referer` is passed as the page URL that references the resource: key and
/// segment requests carry it, which is what real browsers do and what several
/// sites check before allowing a download.
pub fn http_get(
    client: &reqwest::blocking::Client,
    url: &str,
    referer: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut req = client
        .get(url)
        .header("User-Agent", BROWSER_UA)
        .header("Accept", "*/*");
    if let Some(referer) = referer.map(str::trim).filter(|r| !r.is_empty()) {
        req = req.header("Referer", referer);
    }
    let resp = req
        .send()
        .map_err(|e| format!("GET {url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("GET {url}: HTTP {status}"));
    }
    resp.bytes()
        .map(|b| b.to_vec())
        .map_err(|e| format!("GET {url}: read body: {e}"))
}

fn resolve(base: &Url, uri: &str) -> Result<String, String> {
    base.join(uri)
        .map(|u| u.to_string())
        .map_err(|e| format!("bad relative URI {uri:?}: {e}"))
}

/// Parse an m3u8 playlist text. `base` is the URL the playlist was fetched from.
pub fn parse_playlist(text: &str, base: &Url) -> Result<Playlist, String> {
    let mut pl = Playlist::default();
    let mut pending_variant: Option<String> = None;
    let mut pending_key: Option<KeyInfo> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(tag) = line.strip_prefix('#') {
            if let Some(rest) = tag.strip_prefix("EXT-X-STREAM-INF") {
                let _ = rest;
                pl.is_master = true;
                continue;
            }
            if let Some(attrs) = tag.strip_prefix("EXT-X-KEY:") {
                let mut method = String::new();
                let mut uri = String::new();
                let mut iv = None;
                for part in split_attrs(attrs) {
                    if let Some(v) = part.strip_prefix("METHOD=") {
                        method = v.trim_matches('"').to_string();
                    } else if let Some(v) = part.strip_prefix("URI=") {
                        uri = v.trim_matches('"').to_string();
                    } else if let Some(v) = part.strip_prefix("IV=") {
                        let hexstr = v.strip_prefix("0x").or(v.strip_prefix("0X")).unwrap_or(v);
                        if let Ok(bytes) = hex::decode(hexstr) {
                            if bytes.len() == 16 {
                                let mut a = [0u8; 16];
                                a.copy_from_slice(&bytes);
                                iv = Some(a);
                            }
                        }
                    }
                }
                pending_key = Some(KeyInfo { method, uri, iv });
                continue;
            }
            if let Some(v) = tag.strip_prefix("EXT-X-MEDIA-SEQUENCE:") {
                pl.media_sequence = v.trim().parse().unwrap_or(0);
                continue;
            }
            if tag.starts_with("EXT-X-ENDLIST") {
                pl.endlist = true;
                continue;
            }
            // other tags (#EXTM3U, #EXTINF, #EXT-X-VERSION, ...) ignored
            continue;
        }
        // non-comment line = a URI (segment, or variant if master)
        if pl.is_master && pending_variant.is_none() {
            pl.variant = Some(resolve(base, line)?);
            pending_variant = Some(line.to_string());
        } else {
            pl.segments.push(resolve(base, line)?);
        }
    }
    if !pl.segments.is_empty() || pl.endlist {
        pl.key = pending_key;
    }
    Ok(pl)
}

/// Split an HLS attribute list on commas that are not inside quotes.
fn split_attrs(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                cur.push(c);
            }
            ',' if !in_quotes => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// AES-128-CBC decrypt with PKCS#7 unpadding.
pub fn decrypt_segment(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, String> {
    if data.is_empty() || data.len() % 16 != 0 {
        return Err(format!(
            "segment length {} is not a multiple of the 16-byte AES block",
            data.len()
        ));
    }
    let mut buf = data.to_vec();
    let pt = Aes128CbcDec::new(key.into(), iv.into())
        .decrypt_padded_mut::<cbc::cipher::block_padding::Pkcs7>(&mut buf)
        .map_err(|e| format!("AES-128-CBC decrypt failed (bad key/IV/segment): {e}"))?;
    Ok(pt.to_vec())
}

/// Media-sequence-number IV: 128-bit big-endian.
pub fn seq_iv(seq: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[8..].copy_from_slice(&seq.to_be_bytes());
    iv
}

/// Fetch playlist, follow first master variant, return (media playlist URL, playlist).
pub fn fetch_media_playlist(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<(String, Playlist), String> {
    let base = Url::parse(url).map_err(|e| format!("bad URL {url:?}: {e}"))?;
    let body = http_get(client, url, Some(url))?;
    let text = String::from_utf8_lossy(&body);
    let pl = parse_playlist(&text, &base)?;
    if pl.is_master {
        if let Some(variant) = &pl.variant {
            let vbase = Url::parse(variant).map_err(|e| format!("bad variant URL: {e}"))?;
            let vbody = http_get(client, variant, Some(url))?;
            let vtext = String::from_utf8_lossy(&vbody);
            let vpl = parse_playlist(&vtext, &vbase)?;
            return Ok((variant.clone(), vpl));
        }
        return Err("master playlist has no variant URI".to_string());
    }
    Ok((url.to_string(), pl))
}

/// Sanitize a URL into a file name stem.
pub fn file_stem(url: &str) -> String {
    let path = Url::parse(url)
        .ok()
        .and_then(|u| u.path_segments().map(|s| s.last().unwrap_or("").to_string()))
        .unwrap_or_default();
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or("video")
        .split('?')
        .next()
        .unwrap_or("video");
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('.');
    // A manifest is normally called `index.m3u8`; drop that suffix so the
    // output reads `index.mp4`, not `index.m3u8.mp4`.
    let lower = cleaned.to_ascii_lowercase();
    let base = if let Some(stripped) = lower.strip_suffix(".m3u8") {
        &cleaned[..stripped.len()]
    } else if let Some(stripped) = lower.strip_suffix(".m3u") {
        &cleaned[..stripped.len()]
    } else {
        cleaned
    };
    if base.is_empty() {
        "video".to_string()
    } else {
        base.to_string()
    }
}

/// Progress callback: (segments_done, segments_total, bytes_written).
pub type ProgressFn<'a> = dyn FnMut(u64, u64, u64) + Send + 'a;

/// Download one HLS stream into `out_path`. Handles AES-128 keys, live re-poll
/// every `repoll_secs` (only when the playlist has no #EXT-X-ENDLIST), and
/// auto-restart after `repoll_secs` on failure. `stop` cancels the worker.
pub fn download_stream(
    client: &reqwest::blocking::Client,
    url: &str,
    out_path: &Path,
    repoll_secs: u64,
    stop: Arc<AtomicBool>,
    status: Arc<std::sync::Mutex<String>>,
    mut progress: &mut ProgressFn<'_>,
) -> Result<(), String> {
    let set_status = |s: &str| {
        if let Ok(mut g) = status.lock() {
            *g = s.to_string();
        }
    };

    let mut attempt = 0u32;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped by user".to_string());
        }
        attempt += 1;
        match download_once(
            client,
            url,
            out_path,
            repoll_secs,
            &stop,
            &status,
            &mut progress,
        ) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if stop.load(Ordering::SeqCst) {
                    return Err(e);
                }
                set_status(&format!("Error (attempt {attempt}): {e} — restarting in {repoll_secs}s"));
                if !sleep_interruptible(repoll_secs, &stop) {
                    return Err("stopped by user".to_string());
                }
            }
        }
    }
}

fn sleep_interruptible(secs: u64, stop: &AtomicBool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs.max(1));
    while std::time::Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    !stop.load(Ordering::SeqCst)
}

fn download_once(
    client: &reqwest::blocking::Client,
    url: &str,
    out_path: &Path,
    repoll_secs: u64,
    stop: &Arc<AtomicBool>,
    status: &Arc<std::sync::Mutex<String>>,
    progress: &mut ProgressFn<'_>,
) -> Result<(), String> {
    let set_status = |s: &str| {
        if let Ok(mut g) = status.lock() {
            *g = s.to_string();
        }
    };

    set_status("Fetching playlist…");
    let (pl_url, mut pl) = fetch_media_playlist(client, url)?;

    // Fetch the AES-128 key if the playlist declares one.
    let key: Option<[u8; 16]> = match &pl.key {
        Some(k) if k.method.eq_ignore_ascii_case("AES-128") => {
            let key_url = resolve(&Url::parse(&pl_url).map_err(|e| e.to_string())?, &k.uri)?;
            set_status("Fetching decryption key…");
            let kb = http_get(client, &key_url, Some(pl_url.as_str()))?;
            if kb.len() != 16 {
                return Err(format!(
                    "key at {key_url} is {} bytes, expected 16 (AES-128)",
                    kb.len()
                ));
            }
            let mut a = [0u8; 16];
            a.copy_from_slice(&kb);
            Some(a)
        }
        Some(k) if k.method.eq_ignore_ascii_case("NONE") => None,
        Some(k) => return Err(format!("unsupported EXT-X-KEY METHOD={}", k.method)),
        None => None, // unencrypted stream
    };
    let key_iv = pl.key.as_ref().and_then(|k| k.iv);

    // Truncate/create the single output file; segments are appended to it.
    {
        let f = File::create(out_path).map_err(|e| format!("create {}: {e}", out_path.display()))?;
        f.set_len(0).ok();
    }
    let mut out = OpenOptions::new()
        .append(true)
        .open(out_path)
        .map_err(|e| format!("open {}: {e}", out_path.display()))?;

    let mut next_seq: u64 = pl.media_sequence; // next media sequence number to download
    let mut bytes_written: u64 = 0;

    loop {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped by user".to_string());
        }
        // Download every segment whose sequence number we have not done yet.
        let total = pl.segments.len() as u64;
        let mut done = 0u64;
        for seg in &pl.segments {
            if stop.load(Ordering::SeqCst) {
                return Err("stopped by user".to_string());
            }
            let seq = next_seq;
            set_status(&format!(
                "Downloading segment {seq} ({}/{})",
                done + 1,
                total
            ));
            let data = http_get(client, seg, Some(pl_url.as_str()))?;
            let plain = match &key {
                Some(k) => {
                    let iv = key_iv.unwrap_or_else(|| seq_iv(seq));
                    decrypt_segment(k, &iv, &data)?
                }
                None => data,
            };
            out.write_all(&plain)
                .map_err(|e| format!("write {}: {e}", out_path.display()))?;
            bytes_written += plain.len() as u64;
            done += 1;
            next_seq += 1;
            progress(done, total, bytes_written);
        }

        if pl.endlist {
            set_status(&format!("Done — {} bytes written", bytes_written));
            progress(total, total, bytes_written);
            return Ok(());
        }
        // Live playlist: re-poll every N seconds for new segments.
        set_status(&format!(
            "Live stream — waiting {repoll_secs}s for new segments…"
        ));
        if !sleep_interruptible(repoll_secs, stop) {
            return Err("stopped by user".to_string());
        }
        if stop.load(Ordering::SeqCst) {
            return Err("stopped by user".to_string());
        }
        let body = http_get(client, &pl_url, Some(pl_url.as_str()))?;
        let base = Url::parse(&pl_url).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&body);
        let new_pl = parse_playlist(&text, &base)?;
        // Keep the key if the new playlist doesn't restate it.
        let mut new_pl = new_pl;
        if new_pl.key.is_none() {
            new_pl.key = pl.key.clone();
        }
        pl = new_pl;
        // Drop already-downloaded segments (media sequence advanced).
        let base_seq = pl.media_sequence;
        while next_seq < base_seq {
            next_seq = base_seq;
        }
        let skip = (next_seq - base_seq) as usize;
        if skip < pl.segments.len() {
            pl.segments.drain(..skip);
            pl.media_sequence = next_seq;
        } else {
            pl.segments.clear();
        }
    }
}
