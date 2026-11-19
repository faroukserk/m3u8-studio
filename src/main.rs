//! m3u8 studio — a minimal HLS downloader with a Slint interface and a
//! browser-extension bridge.
//!
//! Pipeline (unchanged from the original rebuild): playlist → first variant →
//! AES-128 key → per-segment CBC decrypt → append to a single output file.
//! Live playlists are re-polled at the auto-restart interval.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod bridge;
mod extension;
mod hls;

slint::include_modules!();

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{Model, VecModel};

const EXTENSIONS: [&str; 5] = ["mkv", "mp4", "mov", "ts", "webm"];
const RESTART_MIN: i32 = 2;
const RESTART_MAX: i32 = 1000;

const ST_QUEUED: u8 = 0;
const ST_RUNNING: u8 = 1;
const ST_DONE: u8 = 2;
const ST_STOPPED: u8 = 3;
const ST_FAILED: u8 = 4;

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

struct Task {
    id: i32,
    url: String,
    title: Mutex<String>,
    stop: Arc<AtomicBool>,
    state: AtomicU8,
    done: AtomicU64,
    total: AtomicU64,
    bytes: AtomicU64,
    speed: AtomicU64,
    last_bytes: AtomicU64,
    announced: AtomicBool,
    status: Arc<Mutex<String>>,
    out_path: Mutex<Option<PathBuf>>,
}

impl Task {
    fn new(id: i32, url: String, title: String) -> Self {
        Self {
            id,
            url,
            title: Mutex::new(title),
            stop: Arc::new(AtomicBool::new(true)),
            state: AtomicU8::new(ST_QUEUED),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            speed: AtomicU64::new(0),
            last_bytes: AtomicU64::new(0),
            announced: AtomicBool::new(true),
            status: Arc::new(Mutex::new("Queued".to_string())),
            out_path: Mutex::new(None),
        }
    }

    fn state(&self) -> u8 {
        self.state.load(Ordering::SeqCst)
    }

    fn title(&self) -> String {
        self.title.lock().map(|t| t.clone()).unwrap_or_default()
    }

    fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Settings (tiny key=value file, no serde needed)
// ---------------------------------------------------------------------------

struct Settings {
    format: usize,
    dest: String,
    restart: i32,
    auto_start: bool,
    activity: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            format: 1,
            dest: extension::default_dest(),
            restart: 10,
            auto_start: true,
            activity: false,
        }
    }
}

fn load_settings() -> Settings {
    let mut out = Settings::default();
    let Some(path) = extension::settings_file() else {
        return out;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "format" => {
                if let Ok(v) = value.trim().parse::<usize>() {
                    if v < EXTENSIONS.len() {
                        out.format = v;
                    }
                }
            }
            "dest" => {
                let v = value.trim();
                if !v.is_empty() {
                    out.dest = v.to_string();
                }
            }
            "restart" => {
                if let Ok(v) = value.trim().parse::<i32>() {
                    out.restart = v.clamp(RESTART_MIN, RESTART_MAX);
                }
            }
            "auto_start" => out.auto_start = value.trim() != "0",
            "activity" => out.activity = value.trim() == "1",
            _ => {}
        }
    }
    out
}

fn save_settings(state: &AppState) {
    let Some(path) = extension::settings_file() else {
        return;
    };
    let body = format!(
        "format={}\ndest={}\nrestart={}\nauto_start={}\nactivity={}\n",
        state.format,
        state.dest,
        state.restart,
        if state.auto_start { 1 } else { 0 },
        if state.activity_open { 1 } else { 0 }
    );
    let _ = std::fs::write(path, body);
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

struct AppState {
    ui: slint::Weak<AppWindow>,
    tasks: Vec<Arc<Task>>,
    next_id: i32,
    selected_id: i32,
    format: usize,
    dest: String,
    restart: i32,
    auto_start: bool,
    activity_open: bool,
    logs: VecDeque<String>,
    rows: Rc<VecModel<TaskRow>>,
    started: Instant,
    last_tick: Instant,
    tick_count: u64,
    pulse: bool,
    bridge_rx: std::sync::mpsc::Receiver<bridge::Incoming>,
    bridge_state: bridge::StateHandle,
    bridge_received: bridge::RequestCount,
    bridge_address: String,
    received: u32,
    extension_dir: Option<PathBuf>,
    toast_until: Option<Instant>,
}

impl AppState {
    fn ui(&self) -> Option<AppWindow> {
        self.ui.upgrade()
    }

    fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        let stamp = {
            let secs = self.started.elapsed().as_secs();
            format!("{:02}:{:02}", secs / 60, secs % 60)
        };
        self.logs.push_front(format!("[{stamp}] {message}"));
        while self.logs.len() > 240 {
            self.logs.pop_back();
        }
        if let Some(ui) = self.ui() {
            ui.set_log_text(self.logs.iter().cloned().collect::<Vec<_>>().join("\n").into());
            ui.set_status_line(message.into());
        }
    }

    fn toast(&mut self, message: impl Into<String>) {
        let message: String = message.into();
        self.toast_until = Some(Instant::now() + Duration::from_secs(4));
        if let Some(ui) = self.ui() {
            ui.set_toast_text(message.into());
            ui.set_toast_visible(true);
        }
    }

    fn set_dest(&mut self, dest: String) {
        self.dest = dest;
        if let Some(ui) = self.ui() {
            ui.set_dest_dir(self.dest.clone().into());
        }
        save_settings(self);
    }

    fn set_format(&mut self, format: usize) {
        if format >= EXTENSIONS.len() {
            return;
        }
        self.format = format;
        if let Some(ui) = self.ui() {
            ui.set_selected_format(format as i32);
        }
        save_settings(self);
    }

    fn set_restart(&mut self, secs: i32) {
        self.restart = secs.clamp(RESTART_MIN, RESTART_MAX);
        if let Some(ui) = self.ui() {
            ui.set_restart_secs(self.restart);
        }
        save_settings(self);
    }

    fn set_auto_start(&mut self, on: bool) {
        self.auto_start = on;
        if let Some(ui) = self.ui() {
            ui.set_auto_start(on);
        }
        save_settings(self);
    }

    fn toggle_activity(&mut self) {
        self.activity_open = !self.activity_open;
        if let Some(ui) = self.ui() {
            ui.set_activity_open(self.activity_open);
        }
        save_settings(self);
    }

    // ---------------------------------------------------------------- tasks

    fn find(&self, id: i32) -> Option<Arc<Task>> {
        self.tasks.iter().find(|t| t.id == id).cloned()
    }

    fn add_task(&mut self, url: String, page_title: Option<String>) -> (i32, bool) {
        if let Some(existing) = self.tasks.iter().find(|t| t.url == url) {
            if let Some(title) = page_title
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
            {
                if let Ok(mut current) = existing.title.lock() {
                    *current = truncate_title(&title);
                }
            }
            return (existing.id, false);
        }

        let id = self.next_id;
        self.next_id += 1;
        let title = page_title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .map(|t| truncate_title(&t))
            .unwrap_or_else(|| derive_title(&url));
        self.tasks.push(Arc::new(Task::new(id, url, title)));
        self.selected_id = id;
        (id, true)
    }

    fn add_from_input(&mut self, text: String) {
        let url = text.trim().to_string();
        if url.is_empty() {
            return;
        }
        if !is_http_url(&url) {
            self.log(format!("Not an http(s) URL: {url}"));
            self.toast("That doesn't look like an http(s) link");
            return;
        }
        let (id, is_new) = self.add_task(url.clone(), None);
        if let Some(ui) = self.ui() {
            ui.set_url_input("".into());
        }
        if is_new {
            let title = self.find(id).map(|t| t.title()).unwrap_or_default();
            self.log(format!("Added \"{title}\""));
        } else {
            self.selected_id = id;
            self.log("That stream is already in the queue.");
        }
    }

    fn start_task(&mut self, id: i32) {
        let Some(task) = self.find(id) else { return };
        if task.state() == ST_RUNNING {
            return;
        }

        let dir = PathBuf::from(self.dest.trim());
        if dir.as_os_str().is_empty() {
            self.log("Pick an output folder first.");
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.log(format!("Cannot create {}: {e}", dir.display()));
            self.toast("Couldn't create the output folder");
            return;
        }

        let ext = EXTENSIONS[self.format];
        let stem = hls::file_stem(&task.url);
        let mut path = dir.join(format!("{stem}.{ext}"));
        let mut suffix = 2;
        while self.tasks.iter().any(|other| {
            !Arc::ptr_eq(other, &task)
                && other
                    .out_path
                    .lock()
                    .map(|p| p.as_deref() == Some(path.as_path()))
                    .unwrap_or(false)
        }) {
            path = dir.join(format!("{stem}-{suffix}.{ext}"));
            suffix += 1;
        }

        task.stop.store(false, Ordering::SeqCst);
        task.state.store(ST_RUNNING, Ordering::SeqCst);
        task.done.store(0, Ordering::SeqCst);
        task.total.store(0, Ordering::SeqCst);
        task.bytes.store(0, Ordering::SeqCst);
        task.speed.store(0, Ordering::SeqCst);
        task.last_bytes.store(0, Ordering::SeqCst);
        task.announced.store(false, Ordering::SeqCst);
        if let Ok(mut status) = task.status.lock() {
            *status = "Starting…".to_string();
        }
        if let Ok(mut slot) = task.out_path.lock() {
            *slot = Some(path.clone());
        }
        self.selected_id = id;

        let repoll = self.restart.max(RESTART_MIN) as u64;
        self.log(format!(
            "Downloading \"{}\" → {}",
            task.title(),
            path.display()
        ));
        spawn_download(task, path, repoll);
    }

    fn stop_task(&mut self, id: i32) {
        let Some(task) = self.find(id) else { return };
        if task.state() != ST_RUNNING {
            return;
        }
        task.stop.store(true, Ordering::SeqCst);
        if let Ok(mut status) = task.status.lock() {
            *status = "Stopping…".to_string();
        }
        self.log(format!("Stopping \"{}\"", task.title()));
    }

    fn remove_task(&mut self, id: i32, remove_file: bool) {
        let Some(task) = self.find(id) else { return };
        task.stop.store(true, Ordering::SeqCst);

        if remove_file {
            if let Ok(guard) = task.out_path.lock() {
                if let Some(path) = guard.as_ref() {
                    if path.is_file() {
                        match std::fs::remove_file(path) {
                            Ok(()) => self.log(format!("Deleted file {}", path.display())),
                            Err(e) => {
                                self.log(format!("Could not delete {}: {e}", path.display()));
                                self.toast("Couldn't delete the file from disk");
                            }
                        }
                    }
                }
            }
        }

        self.tasks.retain(|t| t.id != id);
        if self.selected_id == id {
            self.selected_id = self.tasks.last().map(|t| t.id).unwrap_or(-1);
        }
        self.log(format!("Removed \"{}\"", task.title()));
    }

    fn clear_finished(&mut self) {
        let before = self.tasks.len();
        self.tasks.retain(|t| t.state() < ST_DONE);
        if !self.tasks.iter().any(|t| t.id == self.selected_id) {
            self.selected_id = self.tasks.last().map(|t| t.id).unwrap_or(-1);
        }
        let removed = before - self.tasks.len();
        if removed > 0 {
            self.log(format!("Cleared {removed} finished item(s)."));
        }
    }

    fn select_task(&mut self, id: i32) {
        if self.tasks.iter().any(|t| t.id == id) {
            self.selected_id = id;
        }
    }

    fn start_all(&mut self) {
        let ids: Vec<i32> = self
            .tasks
            .iter()
            .filter(|t| matches!(t.state(), ST_QUEUED | ST_STOPPED | ST_FAILED))
            .map(|t| t.id)
            .collect();
        for id in ids {
            self.start_task(id);
        }
    }

    fn stop_all(&mut self) {
        for task in &self.tasks {
            if task.state() == ST_RUNNING {
                task.stop.store(true, Ordering::SeqCst);
                if let Ok(mut status) = task.status.lock() {
                    *status = "Stopping…".to_string();
                }
            }
        }
        if self.tasks.iter().any(|t| t.state() == ST_RUNNING) {
            self.log("Stopping all downloads…");
        }
    }

    fn open_task(&mut self, id: i32) {
        let Some(task) = self.find(id) else { return };
        let path = task.out_path.lock().ok().and_then(|p| p.clone());
        match path {
            Some(path) if path.exists() => {
                if let Err(e) = extension::reveal_file(&path) {
                    self.log(format!("Could not reveal {}: {e}", path.display()));
                }
            }
            _ => {
                self.log("The output file isn't there yet.");
                self.toast("Nothing to open yet");
            }
        }
    }

    fn open_destination(&mut self) {
        let dir = PathBuf::from(self.dest.trim());
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.log(format!("Cannot open {}: {e}", dir.display()));
            return;
        }
        if let Err(e) = extension::open_folder(&dir) {
            self.log(format!("Could not open {}: {e}", dir.display()));
        }
    }

    fn browse_destination(&mut self) {
        let mut dialog = rfd::FileDialog::new();
        let current = PathBuf::from(self.dest.trim());
        if current.is_dir() {
            dialog = dialog.set_directory(&current);
        }
        if let Some(folder) = dialog.pick_folder() {
            let dest = folder.display().to_string();
            self.set_dest(dest);
            self.log("Output folder updated.");
        }
    }

    fn open_extension_folder(&mut self) {
        match &self.extension_dir {
            Some(dir) => {
                if let Err(e) = extension::open_folder(dir) {
                    self.log(format!("Could not open {}: {e}", dir.display()));
                }
            }
            None => {
                self.log("The bundled extension folder could not be unpacked.");
                self.toast("Extension folder unavailable");
            }
        }
    }

    fn open_kofi(&mut self) {
        let url = "https://ko-fi.com/H4D12AXINB";
        if let Err(e) = extension::open_url(url) {
            self.log(format!("Could not open Ko-fi: {e}"));
            self.toast("Couldn't open the Ko-fi page");
        } else {
            self.log("Opening the Ko-fi page in your browser.");
        }
    }

    fn copy_bridge(&mut self) {
        let address = format!("http://{}", self.bridge_address);
        if extension::copy_text(&address) {
            self.toast(format!("Copied {address}"));
        } else {
            self.log(format!("Bridge address: {address}"));
            self.toast("Couldn't access the clipboard");
        }
    }

    // ------------------------------------------------------- extension input

    fn drain_bridge(&mut self) {
        loop {
            match self.bridge_rx.try_recv() {
                Ok(incoming) => self.accept_from_browser(incoming),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn accept_from_browser(&mut self, incoming: bridge::Incoming) {
        let url = incoming.url.trim().to_string();
        if !is_http_url(&url) {
            self.log(format!("Extension sent an invalid URL: {url}"));
            return;
        }
        let title_hint = if incoming.title.trim().is_empty() {
            None
        } else {
            Some(incoming.title)
        };
        let (id, is_new) = self.add_task(url, title_hint);
        let title = self.find(id).map(|t| t.title()).unwrap_or_default();

        if is_new {
            self.log(format!("From browser: \"{title}\""));
        } else {
            self.log("Stream already queued — reusing it.");
        }
        self.toast(format!(
            "{} \"{title}\"",
            if is_new { "Received" } else { "Already queued:" }
        ));

        if self.auto_start {
            self.start_task(id);
        }
    }

    // -------------------------------------------------------------- ticking

    fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64();
        if dt >= 0.4 {
            self.last_tick = now;
            for task in &self.tasks {
                if task.state() == ST_RUNNING {
                    let bytes = task.bytes.load(Ordering::SeqCst);
                    let last = task.last_bytes.swap(bytes, Ordering::SeqCst);
                    let speed = if bytes >= last {
                        ((bytes - last) as f64 / dt).round() as u64
                    } else {
                        0
                    };
                    task.speed.store(speed, Ordering::SeqCst);
                } else {
                    task.speed.store(0, Ordering::SeqCst);
                    task.last_bytes
                        .store(task.bytes.load(Ordering::SeqCst), Ordering::SeqCst);
                }
            }
        }

        self.drain_bridge();

        let received = self.bridge_received.load(Ordering::SeqCst) as u32;
        if received != self.received {
            self.received = received;
            if let Some(ui) = self.ui() {
                ui.set_received(received as i32);
                ui.set_received_text(
                    if received == 1 {
                        "1 link received from the extension".to_string()
                    } else {
                        format!("{received} links received from the extension")
                    }
                    .into(),
                );
            }
        }

        self.announce_finished();

        self.tick_count += 1;
        if self.tick_count % 3 == 0 {
            self.pulse = !self.pulse;
        }
        if let Some(ui) = self.ui() {
            ui.set_pulse(self.pulse);
        }

        self.sync_ui();
        self.publish_bridge_state();

        if let Some(until) = self.toast_until {
            if now >= until {
                self.toast_until = None;
                if let Some(ui) = self.ui() {
                    ui.set_toast_visible(false);
                }
            }
        }
    }

    fn announce_finished(&mut self) {
        let mut announcements: Vec<(i32, u8, String, PathBuf)> = Vec::new();
        for task in &self.tasks {
            let state = task.state();
            if state >= ST_DONE && !task.announced.load(Ordering::SeqCst) {
                task.announced.store(true, Ordering::SeqCst);
                let path = task
                    .out_path
                    .lock()
                    .ok()
                    .and_then(|p| p.clone())
                    .unwrap_or_default();
                announcements.push((task.id, state, task.title(), path));
            }
        }
        for (id, state, title, path) in announcements {
            match state {
                ST_DONE => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| title.clone());
                    self.log(format!("Completed \"{title}\" → {name}"));
                    self.toast(format!("Saved {name}"));
                }
                ST_STOPPED => self.log(format!("Stopped \"{title}\".")),
                ST_FAILED => {
                    let reason = self
                        .find(id)
                        .map(|t| t.status())
                        .unwrap_or_else(|| "unknown error".to_string());
                    self.log(format!("Failed \"{title}\": {reason}"));
                }
                _ => {}
            }
        }
    }

    fn sync_ui(&mut self) {
        if !self.tasks.iter().any(|t| t.id == self.selected_id) {
            self.selected_id = self.tasks.last().map(|t| t.id).unwrap_or(-1);
        }

        let rows: Vec<TaskRow> = self.tasks.iter().map(|t| self.row_for(t)).collect();
        if self.rows.row_count() != rows.len() {
            self.rows.set_vec(rows);
        } else {
            for (i, row) in rows.into_iter().enumerate() {
                self.rows.set_row_data(i, row);
            }
        }

        let active = self
            .tasks
            .iter()
            .filter(|t| t.state() == ST_RUNNING)
            .count();
        let pending = self
            .tasks
            .iter()
            .filter(|t| matches!(t.state(), ST_QUEUED | ST_STOPPED | ST_FAILED))
            .count();
        let finished = self.tasks.iter().filter(|t| t.state() >= ST_DONE).count();
        let total = self.tasks.len();
        let speed: u64 = self
            .tasks
            .iter()
            .map(|t| t.speed.load(Ordering::SeqCst))
            .sum();

        let summary = if total == 0 {
            "No streams yet — paste a link or send one from the browser".to_string()
        } else {
            let mut parts = vec![format!(
                "{total} stream{}",
                if total == 1 { "" } else { "s" }
            )];
            if active > 0 {
                parts.push(format!(
                    "{active} downloading{}",
                    if speed > 0 {
                        format!(" · {}/s", human_bytes(speed))
                    } else {
                        String::new()
                    }
                ));
            }
            if finished > 0 {
                parts.push(format!("{finished} finished"));
            }
            parts.join("  ·  ")
        };

        if let Some(ui) = self.ui() {
            ui.set_selected_id(self.selected_id);
            ui.set_active_count(active as i32);
            ui.set_pending_count(pending as i32);
            ui.set_finished_count(finished as i32);
            ui.set_summary(summary.into());
            ui.set_activity_open(self.activity_open);
            ui.set_log_text(self.logs.iter().cloned().collect::<Vec<_>>().join("\n").into());
        }
    }

    fn row_for(&self, task: &Task) -> TaskRow {
        let state = task.state();
        let done = task.done.load(Ordering::SeqCst);
        let total = task.total.load(Ordering::SeqCst);
        let bytes = task.bytes.load(Ordering::SeqCst);
        let speed = task.speed.load(Ordering::SeqCst);
        let status = task.status();

        let progress = if total > 0 {
            (done as f32 / total as f32).clamp(0.0, 1.0)
        } else if state == ST_RUNNING {
            0.02
        } else {
            0.0
        };

        let badge = match state {
            ST_RUNNING if status.to_ascii_lowercase().contains("live") => "Live",
            ST_RUNNING => "Downloading",
            ST_DONE => "Complete",
            ST_STOPPED => "Stopped",
            ST_FAILED => "Failed",
            _ => "Queued",
        };

        let detail = match state {
            ST_RUNNING if total == 0 => status.clone(),
            ST_RUNNING => {
                let mut text = format!("{done}/{total} segments · {}", human_bytes(bytes));
                if speed > 0 {
                    text.push_str(&format!(" · {}/s", human_bytes(speed)));
                }
                text
            }
            ST_DONE => format!("{} · complete", human_bytes(bytes)),
            ST_STOPPED => "Stopped".to_string(),
            ST_FAILED => "Failed — check the activity log".to_string(),
            _ => "Ready to download".to_string(),
        };

        let can_open = matches!(state, ST_DONE | ST_STOPPED)
            && task
                .out_path
                .lock()
                .map(|p| p.as_ref().map(|p| p.exists()).unwrap_or(false))
                .unwrap_or(false);

        let file_name = task
            .out_path
            .lock()
            .ok()
            .and_then(|p| {
                p.as_ref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default();

        TaskRow {
            id: task.id,
            title: task.title().into(),
            url: task.url.clone().into(),
            badge: badge.into(),
            detail: detail.into(),
            progress,
            state: state as i32,
            can_open,
            file_name: file_name.into(),
        }
    }

    fn publish_bridge_state(&mut self) {
        let items: Vec<bridge::ItemInfo> = self
            .tasks
            .iter()
            .map(|task| {
                let total = task.total.load(Ordering::SeqCst);
                let done = task.done.load(Ordering::SeqCst);
                bridge::ItemInfo {
                    url: task.url.clone(),
                    title: task.title(),
                    state: task.state(),
                    progress: if total > 0 {
                        (done as f32 / total as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    },
                }
            })
            .collect();
        let active = items.iter().filter(|i| i.state == ST_RUNNING).count();
        let pending = items
            .iter()
            .filter(|i| matches!(i.state, ST_QUEUED | ST_STOPPED | ST_FAILED))
            .count();
        if let Ok(mut shared) = self.bridge_state.lock() {
            shared.active = active;
            shared.pending = pending;
            shared.auto_start = self.auto_start;
            shared.items = items;
        }
    }
}

// ---------------------------------------------------------------------------
// Download worker
// ---------------------------------------------------------------------------

fn spawn_download(task: Arc<Task>, path: PathBuf, repoll: u64) {
    std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("m3u8-studio/", env!("CARGO_PKG_VERSION")))
            .build();

        let client = match client {
            Ok(client) => client,
            Err(e) => {
                task.state.store(ST_FAILED, Ordering::SeqCst);
                if let Ok(mut status) = task.status.lock() {
                    *status = format!("HTTP client error: {e}");
                }
                return;
            }
        };

        let progress_task = task.clone();
        let mut progress = move |done: u64, total: u64, bytes: u64| {
            progress_task.done.store(done, Ordering::SeqCst);
            progress_task.total.store(total, Ordering::SeqCst);
            progress_task.bytes.store(bytes, Ordering::SeqCst);
        };

        let result = hls::download_stream(
            &client,
            &task.url,
            &path,
            repoll,
            task.stop.clone(),
            task.status.clone(),
            &mut progress,
        );

        match result {
            Ok(()) => {
                task.state.store(ST_DONE, Ordering::SeqCst);
                if let Ok(mut status) = task.status.lock() {
                    *status = "Completed".to_string();
                }
            }
            Err(e) => {
                if task.stop.load(Ordering::SeqCst) {
                    task.state.store(ST_STOPPED, Ordering::SeqCst);
                    if let Ok(mut status) = task.status.lock() {
                        *status = "Stopped".to_string();
                    }
                } else {
                    task.state.store(ST_FAILED, Ordering::SeqCst);
                    if let Ok(mut status) = task.status.lock() {
                        *status = e;
                    }
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_http_url(text: &str) -> bool {
    match url::Url::parse(text) {
        Ok(u) => {
            (u.scheme() == "http" || u.scheme() == "https") && u.host_str().is_some()
        }
        Err(_) => false,
    }
}

fn truncate_title(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.chars().count() <= 90 {
        trimmed.to_string()
    } else {
        let mut out: String = trimmed.chars().take(87).collect();
        out.push('…');
        out
    }
}

fn derive_title(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return "Stream".to_string();
    };
    let host = parsed
        .host_str()
        .unwrap_or("stream")
        .trim_start_matches("www.")
        .to_string();
    let last = parsed
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or("");
    let stem = last
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(last)
        .trim();
    let generic = matches!(
        stem.to_ascii_lowercase().as_str(),
        "" | "index" | "playlist" | "master" | "chunklist" | "stream" | "video" | "manifest" | "prog_index"
    );

    if generic {
        return format!("{host} stream");
    }

    let mut words = stem
        .replace(['_', '-', '+'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if words.is_empty() {
        words = host.clone();
    }
    let mut chars = words.chars();
    let pretty = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => host,
    };
    truncate_title(&pretty)
}

fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

// ---------------------------------------------------------------------------
// Wiring
// ---------------------------------------------------------------------------

fn wire(state: Rc<RefCell<AppState>>, ui: &AppWindow) {
    ui.on_add_url({
        let state = state.clone();
        move |text| {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.add_from_input(text.to_string());
            }
        }
    });

    ui.on_select_task({
        let state = state.clone();
        move |id| {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.select_task(id);
            }
        }
    });

    ui.on_start_task({
        let state = state.clone();
        move |id| {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.start_task(id);
            }
        }
    });

    ui.on_stop_task({
        let state = state.clone();
        move |id| {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.stop_task(id);
            }
        }
    });

    ui.on_remove_task({
        let state = state.clone();
        move |id, remove_file| {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.remove_task(id, remove_file);
            }
        }
    });

    ui.on_open_task({
        let state = state.clone();
        move |id| {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.open_task(id);
            }
        }
    });

    ui.on_start_all({
        let state = state.clone();
        move || {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.start_all();
            }
        }
    });

    ui.on_stop_all({
        let state = state.clone();
        move || {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.stop_all();
            }
        }
    });

    ui.on_clear_finished({
        let state = state.clone();
        move || {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.clear_finished();
            }
        }
    });

    ui.on_browse_destination({
        let state = state.clone();
        move || {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.browse_destination();
            }
        }
    });

    ui.on_open_destination({
        let state = state.clone();
        move || {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.open_destination();
            }
        }
    });

    ui.on_format_changed({
        let state = state.clone();
        move |i| {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.set_format(i.max(0) as usize);
            }
        }
    });

    ui.on_restart_changed({
        let state = state.clone();
        move |v| {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.set_restart(v);
            }
        }
    });

    ui.on_auto_start_changed({
        let state = state.clone();
        move |on| {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.set_auto_start(on);
            }
        }
    });

    ui.on_toggle_activity({
        let state = state.clone();
        move || {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.toggle_activity();
            }
        }
    });

    ui.on_clear_log({
        let state = state.clone();
        move || {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.logs.clear();
                s.log("Activity cleared.");
            }
        }
    });

    ui.on_open_setup({
        let state = state.clone();
        move || {
            if let Some(ui) = state.borrow().ui() {
                ui.set_setup_open(true);
            }
        }
    });

    ui.on_close_setup({
        let state = state.clone();
        move || {
            if let Some(ui) = state.borrow().ui() {
                ui.set_setup_open(false);
            }
        }
    });

    ui.on_open_extension_folder({
        let state = state.clone();
        move || {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.open_extension_folder();
            }
        }
    });

    ui.on_open_kofi({
        let state = state.clone();
        move || {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.open_kofi();
            }
        }
    });

    ui.on_copy_bridge({
        let state = state.clone();
        move || {
            if let Ok(mut s) = state.try_borrow_mut() {
                s.copy_bridge();
            }
        }
    });
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;

    let settings = load_settings();
    let extension_dir = extension::ensure_installed();

    let (tx, rx) = std::sync::mpsc::channel::<bridge::Incoming>();
    let (bridge_ok, bridge_address, bridge_state, bridge_received) = match bridge::spawn(tx) {
        Ok(info) => (
            true,
            format!("127.0.0.1:{}", info.port),
            info.state,
            info.received,
        ),
        Err(_) => (
            false,
            format!("127.0.0.1:{}", bridge::BRIDGE_PORT),
            Arc::new(Mutex::new(bridge::SharedState::default())),
            Arc::new(AtomicU64::new(0)),
        ),
    };

    let rows = Rc::new(VecModel::<TaskRow>::default());
    ui.set_tasks(rows.clone().into());

    // Initial UI state
    ui.set_selected_format(settings.format as i32);
    ui.set_dest_dir(settings.dest.clone().into());
    ui.set_restart_secs(settings.restart);
    ui.set_auto_start(settings.auto_start);
    ui.set_activity_open(settings.activity);
    ui.set_bridge_ok(bridge_ok);
    ui.set_bridge_address(bridge_address.clone().into());
    ui.set_extension_path(
        extension_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "extension folder unavailable".to_string())
            .into(),
    );
    ui.set_received_text("0 links received".into());
    ui.set_summary("No streams yet — paste a link or send one from the browser".into());
    ui.set_status_line("Ready".into());

    let started = Instant::now();
    let state = Rc::new(RefCell::new(AppState {
        ui: ui.as_weak(),
        tasks: Vec::new(),
        next_id: 1,
        selected_id: -1,
        format: settings.format,
        dest: settings.dest.clone(),
        restart: settings.restart,
        auto_start: settings.auto_start,
        activity_open: settings.activity,
        logs: VecDeque::new(),
        rows,
        started,
        last_tick: started,
        tick_count: 0,
        pulse: false,
        bridge_rx: rx,
        bridge_state,
        bridge_received,
        bridge_address: bridge_address.clone(),
        received: 0,
        extension_dir,
        toast_until: None,
    }));

    wire(state.clone(), &ui);

    {
        let mut s = state.borrow_mut();
        let dest = s.dest.clone();
        s.log(format!("Welcome to m3u8 studio. Saving to {dest}"));
        if bridge_ok {
            s.log(format!(
                "Browser bridge listening on http://{bridge_address} (loopback only)."
            ));
        } else {
            s.log(format!(
                "Bridge unavailable on port {} — is another copy running?",
                bridge::BRIDGE_PORT
            ));
            s.toast("Browser bridge offline");
        }
        if let Some(dir) = s.extension_dir.clone() {
            s.log(format!("Extension ready at {}", dir.display()));
        }
        s.sync_ui();
        if !bridge_ok {
            s.toast("Bridge offline — extension can't connect");
        }
    }

    let timer = slint::Timer::default();
    {
        let state = state.clone();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(250),
            move || {
                if let Ok(mut s) = state.try_borrow_mut() {
                    s.tick();
                }
            },
        );
    }

    let result = ui.run();
    drop(timer);
    result
}
