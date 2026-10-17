//! Everything below the UI: screen capture, settings and state, capture history, OCR, and
//! sharing (clipboard, save, upload, launching other modes). No UI toolkit or dialogs here.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod capture;
pub mod config;
pub mod history;
pub mod ocr;
pub mod ops;
pub mod share;

pub use shotcraft_doc as doc;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Doc(#[from] doc::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("screen capture: {0}")]
    Capture(#[from] xcap::XCapError),
    #[error("screen capture failed (on macOS, grant Screen Recording permission): {0}")]
    ScreenRecording(xcap::XCapError),
    #[error("no monitors found")]
    NoMonitors,
    #[error("the display used for that capture (id {0}) is no longer connected")]
    MonitorGone(u32),
    #[error("there is no display {0} (displays are numbered from 1, primary first)")]
    NoSuchDisplay(usize),
    #[error("area is outside the image ({width}×{height})")]
    AreaOutside { width: u32, height: u32 },
    #[error("clipboard: {0}")]
    Clipboard(#[from] arboard::Error),
    #[error("network: {0}")]
    Http(#[from] ureq::Error),
    #[error("text recognition failed: {0}")]
    Ocr(String),
    #[error("invalid history id `{0}`")]
    InvalidHistoryId(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
