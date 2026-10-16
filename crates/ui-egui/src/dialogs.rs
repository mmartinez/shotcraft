//! Native file dialogs.

use std::path::PathBuf;

use image::RgbaImage;
use shotcraft_engine::config::{ExportFormat, Settings};
use shotcraft_engine::doc::model::PROJECT_EXT;
use shotcraft_engine::share;

/// Ask where to save; the extension picks the format. Returns `None` if cancelled.
pub fn save_as(img: &RgbaImage, settings: &Settings) -> shotcraft_engine::Result<Option<PathBuf>> {
    let mut dialog = rfd::FileDialog::new().set_directory(&settings.save_dir).set_file_name(share::default_file_name(settings.export_format));
    for f in ExportFormat::ALL {
        dialog = dialog.add_filter(f.label(), &[f.ext()]);
    }
    let Some(chosen) = dialog.save_file() else { return Ok(None) };
    let (path, format) = match ExportFormat::from_path(&chosen) {
        Some(format) => (chosen, format),
        None => {
            // Append rather than replace: "Shot 18.25.31" must not become "Shot 18.25.png".
            let mut name = chosen.into_os_string();
            name.push(".");
            name.push(settings.export_format.ext());
            let path = PathBuf::from(name);
            // The dialog only confirmed overwriting the name it showed.
            if path.exists() {
                return Err(shotcraft_engine::Error::Other(format!("{} already exists; choose another name", path.display())));
            }
            (path, settings.export_format)
        }
    };
    share::write_image(img, &path, format, settings)?;
    Ok(Some(path))
}

pub fn pick_image() -> Option<PathBuf> {
    rfd::FileDialog::new().add_filter("Images and projects", &["png", "jpg", "jpeg", "webp", "gif", "bmp", PROJECT_EXT]).pick_file()
}
