//! Local HTTP bridge between the browser extension (`extension/`) and the
//! desktop app. It listens on 127.0.0.1 only and never talks to another host.
//!
//!   GET  /ping                 -> {"ok":true,"app":"m3u8 studio","version":"…"}
//!   GET  /status               -> queue + state of every download
//!   GET  /list                 -> {"ok":true,"urls":[…]}
//!   POST /add?url=<m3u8>[&title=<title>]   (requires X-M3U8-Studio: 1)
//!        or POST /add with a JSON body {"url":"…","title":"…"}
//!
//! Every response carries permissive CORS headers so the extension popup can
//! use the bridge directly. A DNS-rebinding guard checks the Host header and
//! plain web-page origins are rejected.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const BRIDGE_PORT: u16 = 37811;
pub const APP_NAME: &str = "m3u8 studio";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct Incoming {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct ItemInfo {
    pub url: String,
    pub title: String,
    pub state: u8,
    pub progress: f32,
}

#[derive(Debug, Clone, Default)]
pub struct SharedState {
    pub active: usize,
    pub pending: usize,
    pub auto_start: bool,
    pub items: Vec<ItemInfo>,
}

pub type StateHandle = Arc<Mutex<SharedState>>;

/// Number of URLs the extension has pushed since the app started.
pub type RequestCount = Arc<AtomicU64>;

pub struct BridgeInfo {
    pub port: u16,
    pub state: StateHandle,
    pub received: RequestCount,
}

/// Spawn the listener. Each connection runs in its own short-lived thread.
pub fn spawn(tx: Sender<Incoming>) -> std::io::Result<BridgeInfo> {
    let listener = TcpListener::bind(("127.0.0.1", BRIDGE_PORT))?;
    let port = listener.local_addr()?.port();
    let state: StateHandle = Arc::new(Mutex::new(SharedState::default()));
    let received: RequestCount = Arc::new(AtomicU64::new(0));

    let thread_state = state.clone();
    let thread_received = received.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let tx = tx.clone();
                    let state = thread_state.clone();
                    let received = thread_received.clone();
                    std::thread::spawn(move || handle(stream, tx, state, received));
                }
                Err(_) => break,
            }
        }
    });

    Ok(BridgeInfo {
        port,
        state,
        received,
    })
}

fn handle(mut stream: TcpStream, tx: Sender<Incoming>, state: StateHandle, received: RequestCount) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut reader = match stream.try_clone() {
        Ok(s) => BufReader::new(s),
        Err(_) => return,
    };

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }

    let mut content_length = 0usize;
    let mut host = String::new();
    let mut origin = String::new();
    let mut marker = false;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
        if let Some(v) = lower.strip_prefix("host:") {
            host = v.trim().to_string();
        }
        if let Some(v) = lower.strip_prefix("origin:") {
            origin = v.trim().to_string();
        }
        if lower.starts_with("x-m3u8-studio:") {
            marker = true;
        }
    }

    let mut body = Vec::new();
    if content_length > 0 && content_length <= 1_048_576 {
        body.resize(content_length, 0);
        if reader.read_exact(&mut body).is_err() {
            return;
        }
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    // ------------------------------------------------------ DNS-rebinding guard
    if !host.is_empty() {
        let hostname = host.split(':').next().unwrap_or("");
        if hostname != "127.0.0.1" && hostname != "localhost" {
            respond(&mut stream, 403, "Forbidden", "{\"ok\":false,\"error\":\"forbidden host\"}");
            return;
        }
    }

    // A normal web page must never be able to drive the downloader. Extension
    // pages use a `chrome-extension://` origin; anything http(s) is rejected.
    if origin.starts_with("http://") || origin.starts_with("https://") {
        respond(&mut stream, 403, "Forbidden", "{\"ok\":false,\"error\":\"forbidden origin\"}");
        return;
    }

    let (status, body_text) = route(&method, &target, &body, marker, &tx, &state, &received);
    respond(&mut stream, status.0, status.1, &body_text);
}

fn route(
    method: &str,
    target: &str,
    body: &[u8],
    marker: bool,
    tx: &Sender<Incoming>,
    state: &StateHandle,
    received: &RequestCount,
) -> ((u16, &'static str), String) {
    let path = target.split('?').next().unwrap_or("/");
    let body_text = String::from_utf8_lossy(body);

    match (method, path) {
        ("OPTIONS", _) => ((204, "No Content"), String::new()),

        ("GET", "/ping") => (
            (200, "OK"),
            format!(
                "{{\"ok\":true,\"app\":\"{}\",\"version\":\"{}\"}}",
                APP_NAME, APP_VERSION
            ),
        ),

        ("GET", "/status") => {
            let snapshot = state.lock().map(|s| s.clone()).unwrap_or_default();
            let items = snapshot
                .items
                .iter()
                .map(|i| {
                    format!(
                        "{{\"url\":\"{}\",\"title\":\"{}\",\"state\":\"{}\",\"progress\":{:.3}}}",
                        escape(&i.url),
                        escape(&i.title),
                        state_name(i.state),
                        i.progress
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            (
                (200, "OK"),
                format!(
                    "{{\"ok\":true,\"app\":\"{}\",\"version\":\"{}\",\"active\":{},\"pending\":{},\"autoStart\":{},\"items\":[{}]}}",
                    APP_NAME,
                    APP_VERSION,
                    snapshot.active,
                    snapshot.pending,
                    snapshot.auto_start,
                    items
                ),
            )
        }

        ("GET", "/list") | ("POST", "/list") => {
            let snapshot = state.lock().map(|s| s.clone()).unwrap_or_default();
            let urls = snapshot
                .items
                .iter()
                .map(|i| format!("\"{}\"", escape(&i.url)))
                .collect::<Vec<_>>()
                .join(",");
            ((200, "OK"), format!("{{\"ok\":true,\"urls\":[{}]}}", urls))
        }

        ("POST", "/add") => {
            if !marker {
                return (
                    (403, "Forbidden"),
                    "{\"ok\":false,\"error\":\"missing X-M3U8-Studio header\"}".to_string(),
                );
            }
            let mut url = query(target, "url").unwrap_or_default();
            let mut title = query(target, "title").unwrap_or_default();
            if url.is_empty() {
                url = json_field(&body_text, "url").unwrap_or_default();
            }
            if title.is_empty() {
                title = json_field(&body_text, "title").unwrap_or_default();
            }
            let url = url.trim().to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return (
                    (400, "Bad Request"),
                    "{\"ok\":false,\"error\":\"expected an http(s) url\"}".to_string(),
                );
            }
            let title = title.trim().chars().take(120).collect::<String>();
            if tx.send(Incoming { url: url.clone(), title }).is_err() {
                return (
                    (503, "Service Unavailable"),
                    "{\"ok\":false,\"error\":\"app is shutting down\"}".to_string(),
                );
            }
            received.fetch_add(1, Ordering::SeqCst);
            (
                (200, "OK"),
                format!(
                    "{{\"ok\":true,\"queued\":true,\"url\":\"{}\"}}",
                    escape(&url)
                ),
            )
        }

        _ => (
            (404, "Not Found"),
            "{\"ok\":false,\"error\":\"not found\"}".to_string(),
        ),
    }
}

fn respond(stream: &mut TcpStream, code: u16, reason: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Cache-Control: no-store\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type, X-M3U8-Studio\r\n\
         Access-Control-Max-Age: 86400\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

fn state_name(state: u8) -> &'static str {
    match state {
        1 => "running",
        2 => "done",
        3 => "stopped",
        4 => "failed",
        _ => "queued",
    }
}

fn query(target: &str, key: &str) -> Option<String> {
    let query = target.splitn(2, '?').nth(1)?;
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        if kv.next()? == key {
            return Some(url_decode(kv.next().unwrap_or("")));
        }
    }
    None
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push(hi * 16 + lo);
                    i += 3;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Minimal JSON string-field extractor (the bridge only ever reads `url` and
/// `title` from a flat object).
fn json_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut rest = json.split_once(&needle)?.1;
    rest = rest.trim_start();
    rest = rest.strip_prefix(':')?.trim_start();
    rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            out.push(ch);
                        }
                    }
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Wall-clock helper for diagnostics.
#[allow(dead_code)]
pub fn since(start: Instant) -> String {
    let secs = start.elapsed().as_secs();
    format!("{:02}:{:02}", secs / 60, secs % 60)
}
