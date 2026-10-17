//! Settings, persistent state and on-disk locations.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::doc::model::Background;

const APP_DIR: &str = "shotcraft";

/// `SHOTCRAFT_HOME` keeps settings and data in that one folder instead of the platform's (tests, sandboxes).
fn home_override() -> Option<PathBuf> {
    std::env::var_os("SHOTCRAFT_HOME").filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn config_dir() -> PathBuf {
    home_override().unwrap_or_else(|| dirs::config_dir().unwrap_or_else(std::env::temp_dir).join(APP_DIR))
}

pub fn data_dir() -> PathBuf {
    home_override().unwrap_or_else(|| dirs::data_dir().unwrap_or_else(std::env::temp_dir).join(APP_DIR))
}

pub fn history_dir() -> PathBuf {
    data_dir().join("history")
}

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

pub fn run_dir() -> PathBuf {
    data_dir().join("run")
}

/// Create the config and data folders readable only by this user: they hold screenshots and upload credentials.
pub fn ensure_private_dirs() -> Result<()> {
    for dir in [config_dir(), data_dir()] {
        make_private(&dir)?;
    }
    Ok(())
}

fn make_private(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    // Windows and macOS user folders are already private; a Unix umask usually leaves them world-readable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

fn state_path() -> PathBuf {
    data_dir().join("state.json")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Png,
    Jpeg,
    Webp,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 3] = [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::Webp];

    pub fn ext(self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Webp => "webp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Png => "PNG",
            ExportFormat::Jpeg => "JPEG",
            ExportFormat::Webp => "WebP",
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some(ExportFormat::Png),
            "jpg" | "jpeg" => Some(ExportFormat::Jpeg),
            "webp" => Some(ExportFormat::Webp),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowCaptureStyle {
    /// The window pixels only.
    Plain,
    /// Transparent margin with a drop shadow.
    Shadow,
    /// The window placed on the default background.
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuickAccessSide {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub all_in_one: String,
    pub capture_area: String,
    pub capture_previous_area: String,
    pub capture_fullscreen: String,
    pub capture_window: String,
    pub scrolling_capture: String,
    pub self_timer: String,
    pub capture_text: String,
    pub open_history: String,
    pub restore_recently_closed: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            all_in_one: "Alt+Shift+Digit5".into(),
            capture_area: "Alt+Shift+Digit4".into(),
            capture_previous_area: "Alt+Shift+Digit8".into(),
            capture_fullscreen: "Alt+Shift+Digit3".into(),
            capture_window: "Alt+Shift+Digit6".into(),
            scrolling_capture: "Alt+Shift+Digit7".into(),
            self_timer: String::new(),
            capture_text: "Alt+Shift+Digit2".into(),
            open_history: "Alt+Shift+KeyH".into(),
            restore_recently_closed: String::new(),
        }
    }
}

impl Hotkeys {
    /// (label, CLI subcommand, binding) for every bindable action.
    pub fn entries_mut(&mut self) -> [(&'static str, &'static str, &mut String); 10] {
        [
            ("All-In-One", "all-in-one", &mut self.all_in_one),
            ("Capture Area", "capture-area", &mut self.capture_area),
            ("Capture Previous Area", "capture-previous-area", &mut self.capture_previous_area),
            ("Capture Fullscreen", "capture-fullscreen", &mut self.capture_fullscreen),
            ("Capture Window", "capture-window", &mut self.capture_window),
            ("Scrolling Capture", "scrolling-capture", &mut self.scrolling_capture),
            ("Self-Timer", "self-timer", &mut self.self_timer),
            ("Capture Text (OCR)", "capture-text", &mut self.capture_text),
            ("Open History", "open-history", &mut self.open_history),
            ("Restore Recently Closed", "restore-recently-closed", &mut self.restore_recently_closed),
        ]
    }

    pub fn entries(&self) -> Vec<(&'static str, String)> {
        let mut copy = self.clone();
        copy.entries_mut().into_iter().map(|(_, cmd, key)| (cmd, key.clone())).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UploadSettings {
    /// Endpoint receiving a `multipart/form-data` POST. Empty disables uploads.
    pub url: String,
    pub file_field: String,
    /// Extra request headers, one `Name: value` per entry (e.g. authorization).
    pub headers: Vec<String>,
    /// JSON path to the link in the response (`data.link`); empty means the body is the link.
    pub response_path: String,
}

impl Default for UploadSettings {
    fn default() -> Self {
        Self { url: String::new(), file_field: "file".into(), headers: Vec::new(), response_path: String::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub background: Background,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub save_dir: PathBuf,
    pub export_format: ExportFormat,
    pub jpeg_quality: u8,
    pub show_quick_access: bool,
    pub copy_after_capture: bool,
    pub save_after_capture: bool,
    pub show_crosshair: bool,
    pub show_magnifier: bool,
    pub window_style: WindowCaptureStyle,
    pub quick_access_side: QuickAccessSide,
    /// Seconds before the Quick Access Overlay closes itself; 0 keeps it open.
    pub quick_access_auto_close: u32,
    pub close_quick_access_after_action: bool,
    pub timer_seconds: u32,
    pub ocr_keep_linebreaks: bool,
    pub history_days: u32,
    pub hotkeys: Hotkeys,
    pub upload: UploadSettings,
    pub presets: Vec<Preset>,
    pub palette: Vec<[u8; 4]>,
}

impl Default for Settings {
    fn default() -> Self {
        let save_dir = dirs::desktop_dir().or_else(dirs::picture_dir).or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir);
        Self {
            save_dir,
            export_format: ExportFormat::Png,
            jpeg_quality: 90,
            show_quick_access: true,
            copy_after_capture: false,
            save_after_capture: false,
            show_crosshair: true,
            show_magnifier: true,
            window_style: WindowCaptureStyle::Shadow,
            quick_access_side: QuickAccessSide::Left,
            quick_access_auto_close: 0,
            close_quick_access_after_action: true,
            timer_seconds: 5,
            ocr_keep_linebreaks: true,
            history_days: 30,
            hotkeys: Hotkeys::default(),
            upload: UploadSettings::default(),
            presets: Vec::new(),
            palette: vec![
                [255, 59, 48, 255],
                [255, 149, 0, 255],
                [255, 204, 0, 255],
                [52, 199, 89, 255],
                [10, 132, 255, 255],
                [175, 82, 222, 255],
                [255, 255, 255, 255],
                [28, 28, 30, 255],
            ],
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        read_json(&settings_path()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        write_json(&settings_path(), self)
    }

    /// Change fields on the newest settings on disk, so edits other windows made since we loaded survive.
    pub fn update(f: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let mut settings = Self::load();
        f(&mut settings);
        settings.save()?;
        Ok(settings)
    }

    /// Save the fields that differ from `base` (what this window loaded) over the newest settings on disk.
    pub fn save_changes(&self, base: &Settings) -> Result<Settings> {
        let merged = merge_changes(Self::load(), base, self)?;
        merged.save()?;
        Ok(merged)
    }

    pub fn modified() -> Option<std::time::SystemTime> {
        std::fs::metadata(settings_path()).and_then(|m| m.modified()).ok()
    }
}

/// `disk` with every top-level field that `edited` changed relative to `base`.
fn merge_changes(disk: Settings, base: &Settings, edited: &Settings) -> Result<Settings> {
    let mut out = serde_json::to_value(disk)?;
    if let (Some(out), serde_json::Value::Object(base), serde_json::Value::Object(edited)) =
        (out.as_object_mut(), serde_json::to_value(base)?, serde_json::to_value(edited)?)
    {
        for (key, value) in edited {
            if base.get(&key) != Some(&value) {
                out.insert(key, value);
            }
        }
    }
    Ok(serde_json::from_value(out)?)
}

/// The last selected area, in physical pixels of the monitor it was taken on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LastArea {
    pub monitor_id: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub last_area: Option<LastArea>,
    /// History ids of Quick Access Overlays closed without an action, most recent last.
    pub recently_closed: Vec<String>,
}

impl State {
    pub fn load() -> Self {
        read_json(&state_path()).unwrap_or_default()
    }

    /// Load, modify and save in one step so concurrent processes lose as little as possible.
    pub fn update(f: impl FnOnce(&mut State)) {
        let mut state = Self::load();
        f(&mut state);
        if let Err(err) = write_json(&state_path(), &state) {
            eprintln!("failed to save state: {err:#}");
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(v) => Some(v),
        Err(err) => {
            // Set the file aside rather than let the next save replace it with defaults.
            let mut kept = path.as_os_str().to_owned();
            kept.push(".corrupt");
            let kept = PathBuf::from(kept);
            match std::fs::rename(path, &kept) {
                Ok(()) => {
                    eprintln!("unreadable {} ({err}); moved to {}", path.display(), kept.display());
                    crate::share::warn("Unreadable file set aside", &format!("Using defaults. The old file is now {}", kept.display()));
                }
                Err(e) => eprintln!("ignoring unreadable {}: {err} (could not set it aside: {e})", path.display()),
            }
            None
        }
    }
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::doc::write_atomic(path, &serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_fields_other_windows_changed() {
        let base = Settings::default();
        let mut disk = base.clone();
        disk.palette.push([1, 2, 3, 255]);
        let mut edited = base.clone();
        edited.upload.url = "https://example.test/upload".into();
        let merged = merge_changes(disk, &base, &edited).unwrap();
        assert_eq!(merged.upload.url, "https://example.test/upload");
        assert_eq!(merged.palette.last(), Some(&[1, 2, 3, 255]));
    }

    #[cfg(unix)]
    #[test]
    fn private_dir_is_owner_only_even_if_it_existed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("shotcraft-private-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        make_private(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::remove_dir_all(&dir).ok();
    }
}
