//! Capture history: every capture is kept on disk with its editable project.
//!
//! Layout: `<data>/history/<id>/{capture.png, project.shotcraft, meta.json, thumb.png}`.

use std::path::PathBuf;

use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::config::{history_dir, run_dir, write_json};
use crate::doc::model::Doc;
use crate::doc::render;
use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    /// Unix seconds.
    pub created: i64,
    pub kind: String,
    pub width: u32,
    pub height: u32,
}

pub fn entry_dir(id: &str) -> PathBuf {
    history_dir().join(id)
}

pub fn image_path(id: &str) -> PathBuf {
    entry_dir(id).join("capture.png")
}

pub fn project_path(id: &str) -> PathBuf {
    entry_dir(id).join("project.shotcraft")
}

fn new_id() -> String {
    let now = chrono::Local::now();
    format!("{}-{:05}", now.format("%Y%m%d-%H%M%S%3f"), std::process::id() % 100_000)
}

/// Store a new capture and return its entry.
pub fn add(doc: &Doc, kind: &str) -> Result<Entry> {
    let id = new_id();
    std::fs::create_dir_all(entry_dir(&id))?;
    let flat = render::flatten(doc)?;
    flat.save(image_path(&id))?;
    doc.save_project(&project_path(&id))?;
    let entry = Entry { id, created: chrono::Utc::now().timestamp(), kind: kind.into(), width: flat.width(), height: flat.height() };
    write_json(&entry_dir(&entry.id).join("meta.json"), &entry)?;
    Ok(entry)
}

/// Replace an entry's project and image after editing.
pub fn update(id: &str, doc: &Doc) -> Result<()> {
    let flat = render::flatten(doc)?;
    flat.save(image_path(id))?;
    doc.save_project(&project_path(id))?;
    let _ = std::fs::remove_file(entry_dir(id).join("thumb.png"));
    if let Some(mut entry) = get(id) {
        (entry.width, entry.height) = flat.dimensions();
        write_json(&entry_dir(id).join("meta.json"), &entry)?;
    }
    Ok(())
}

pub fn get(id: &str) -> Option<Entry> {
    serde_json::from_slice(&std::fs::read(entry_dir(id).join("meta.json")).ok()?).ok()
}

/// Newest first.
pub fn list() -> Vec<Entry> {
    let Ok(dir) = std::fs::read_dir(history_dir()) else { return Vec::new() };
    let mut v: Vec<Entry> = dir.flatten().filter_map(|e| get(e.file_name().to_str()?)).collect();
    v.sort_by(|a, b| b.id.cmp(&a.id));
    v
}

pub fn load_doc(id: &str) -> Result<Doc> {
    let project = project_path(id);
    Ok(if project.exists() { Doc::load_project(&project)? } else { Doc::open(&image_path(id))? })
}

pub fn load_image(id: &str) -> Result<RgbaImage> {
    Ok(image::open(image_path(id))?.into_rgba8())
}

pub fn delete(id: &str) -> Result<()> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return Err(Error::InvalidHistoryId(id.into()));
    }
    std::fs::remove_dir_all(entry_dir(id))?;
    let _ = std::fs::remove_dir_all(drag_dir(id));
    Ok(())
}

/// Where Quick Access stages a copy of the capture for drag and drop; it goes when the entry goes.
pub fn drag_dir(id: &str) -> PathBuf {
    run_dir().join(format!("drag-{id}"))
}

/// Drop entries older than `days`.
pub fn prune(days: u32) {
    let cutoff = chrono::Utc::now().timestamp() - days as i64 * 86_400;
    for e in list().into_iter().filter(|e| e.created < cutoff) {
        let _ = delete(&e.id);
    }
}

/// Cached thumbnail fitting in `max`x`max`.
pub fn thumbnail(id: &str, max: u32) -> Result<RgbaImage> {
    let path = entry_dir(id).join("thumb.png");
    if let Ok(img) = image::open(&path) {
        let img = img.into_rgba8();
        if img.width().max(img.height()) == max {
            return Ok(img);
        }
    }
    let full = load_image(id)?;
    let k = max as f32 / full.width().max(full.height()) as f32;
    let thumb = image::imageops::thumbnail(&full, ((full.width() as f32 * k) as u32).max(1), ((full.height() as f32 * k) as u32).max(1));
    let _ = thumb.save(&path);
    Ok(thumb)
}
