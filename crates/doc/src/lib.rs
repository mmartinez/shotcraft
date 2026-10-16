//! The screenshot document: model, rendering, pixel effects and scrolling-capture stitching.
//! Pure data and pixels; nothing here touches the screen, the file system layout or a UI toolkit.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod effects;
pub mod fonts;
pub mod model;
pub mod render;
pub mod stitch;

use std::io::Write;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid project file: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
    #[error("not a ShotCraft project")]
    NotAProject,
    #[error("project version {0} is newer than this app")]
    TooNew(u32),
    #[error("image too large ({0}x{1})")]
    TooLarge(u32, u32),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Replace `path` with `bytes` in one step: write a fresh temp file next to it, then rename it over.
/// The temp file is created exclusively, so a file or symlink planted at its name is never written through.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}-{nanos}.tmp", std::process::id()));
    let result = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_replaces_the_target_and_leaves_no_temp_files() {
        let dir = std::env::temp_dir().join(format!("shotcraft-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.json");
        std::fs::write(&path, b"old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
