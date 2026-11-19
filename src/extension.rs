//! The browser extension ships inside the executable: on first launch (or when
//! the version changes) it is unpacked to `<exe dir>/extension/` so it can be
//! loaded directly from chrome://extensions without hunting for source files.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const TEXT_FILES: &[(&str, &str)] = &[
    ("manifest.json", include_str!("../extension/manifest.json")),
    ("background.js", include_str!("../extension/background.js")),
    ("content.js", include_str!("../extension/content.js")),
    ("popup.html", include_str!("../extension/popup.html")),
    ("popup.css", include_str!("../extension/popup.css")),
    ("popup.js", include_str!("../extension/popup.js")),
    ("README.md", include_str!("../extension/README.md")),
];

const ICON_FILES: &[(&str, &[u8])] = &[
    ("icons/icon16.png", include_bytes!("../extension/icons/icon16.png")),
    ("icons/icon32.png", include_bytes!("../extension/icons/icon32.png")),
    ("icons/icon48.png", include_bytes!("../extension/icons/icon48.png")),
    ("icons/icon128.png", include_bytes!("../extension/icons/icon128.png")),
];

/// Unpack (or refresh) the extension next to the executable.
pub fn ensure_installed() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.join("extension");
    std::fs::create_dir_all(dir.join("icons")).ok()?;

    let stamp = dir.join(".installed-version");
    let wanted = env!("CARGO_PKG_VERSION");
    let current = std::fs::read_to_string(&stamp).unwrap_or_default();
    let complete = TEXT_FILES.iter().all(|(name, _)| dir.join(name).is_file())
        && ICON_FILES.iter().all(|(name, _)| dir.join(name).is_file());
    if current.trim() == wanted && complete {
        return Some(dir);
    }

    for (name, contents) in TEXT_FILES {
        std::fs::write(dir.join(name), contents).ok()?;
    }
    for (name, bytes) in ICON_FILES {
        std::fs::write(dir.join(name), bytes).ok()?;
    }
    std::fs::write(&stamp, wanted).ok()?;
    Some(dir)
}

/// Reveal a folder in the platform file manager.
pub fn open_folder(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer").arg(path).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(path).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

/// Reveal a file in the platform file manager, selecting it when possible.
pub fn reveal_file(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(path).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(dir) = path.parent() {
            Command::new("xdg-open").arg(dir).spawn()?;
        }
    }
    Ok(())
}

/// Best-effort clipboard copy without pulling in an extra dependency.
pub fn copy_text(text: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        if let Ok(mut child) = Command::new("cmd")
            .args(["/C", "clip"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
        false
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::Stdio;
        if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
        false
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for program in ["wl-copy", "xclip -selection clipboard"] {
            let mut parts = program.split_whitespace();
            let Some(name) = parts.next() else { continue };
            if let Ok(mut child) = Command::new(name)
                .args(parts)
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                if child.wait().map(|s| s.success()).unwrap_or(false) {
                    return true;
                }
            }
        }
        false
    }
}

/// Open a link in the platform's default browser.
pub fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        // `cmd /c start` needs a console, which a GUI app doesn't have.
        // rundll32 opens the default browser without one.
        Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(url).spawn()?;
    }
    Ok(())
}

/// Default output folder.
pub fn default_dest() -> String {
    std::env::var("USERPROFILE")
        .map(|p| format!("{p}\\Downloads"))
        .or_else(|_| std::env::var("HOME").map(|p| format!("{p}/Downloads")))
        .unwrap_or_else(|_| ".".to_string())
}

/// Persist the handful of user settings as a tiny `key=value` file.
pub fn settings_file() -> Option<PathBuf> {
    let base = std::env::var("APPDATA")
        .ok()
        .or_else(|| std::env::var("XDG_CONFIG_HOME").ok())
        .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.config")))?;
    let dir = PathBuf::from(base).join("m3u8-studio");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("settings.txt"))
}
