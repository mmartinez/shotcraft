//! On-device text recognition (ocrs) and QR code reading (rqrr).
//! The OCR models are fetched once on first use and then run fully offline.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use image::RgbaImage;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};

use crate::config::models_dir;
use crate::{Error, Result};

/// (file, URL, SHA-256). Upstream publishes no checksums; these are the files the URLs served when
/// pinned (identical on 2026-10-15 and 2026-10-16). Updating a model means updating its hash.
const MODELS: [(&str, &str, &str); 2] = [
    (
        "text-detection.onnx",
        "https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.onnx",
        "a917b23dbd9524b465df7e922641b3ff2981623df4ded5a0234004ef2fee7cfe",
    ),
    (
        "text-recognition.onnx",
        "https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.onnx",
        "86c145c2edb96c8caed5b1ebb8f44d706408922211451c309c625157dd6061c5",
    ),
];

/// The models are 2.5 and 9.7 MB; a much larger download is not them.
const MAX_MODEL_BYTES: u64 = 64 << 20;

fn ocr_err(e: impl std::fmt::Display) -> Error {
    Error::Ocr(format!("{e:#}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn ensure_models() -> Result<[PathBuf; 2]> {
    let dir = models_dir();
    std::fs::create_dir_all(&dir)?;
    let paths = MODELS.map(|(name, ..)| dir.join(name));
    for ((name, url, sha), path) in MODELS.iter().zip(&paths) {
        // Checked on every use: a damaged or replaced file is fetched again instead of trusted.
        if std::fs::read(path).is_ok_and(|b| sha256_hex(&b) == *sha) {
            continue;
        }
        let mut bytes = Vec::new();
        ureq::get(*url)
            .config()
            .timeout_global(Some(Duration::from_secs(300)))
            .build()
            .call()
            .map_err(|e| Error::Other(format!("downloading OCR model {name}: {e}")))?
            .into_body()
            .into_reader()
            .take(MAX_MODEL_BYTES)
            .read_to_end(&mut bytes)?;
        if sha256_hex(&bytes) != *sha {
            return Err(Error::Other(format!("the downloaded OCR model {name} does not match its pinned checksum")));
        }
        // Unique temp file per process, then an atomic rename: concurrent first runs can't corrupt it.
        crate::doc::write_atomic(path, &bytes)?;
    }
    Ok(paths)
}

/// Text in reading order; lines joined with `\n` or, without line breaks, with spaces.
pub fn recognize(img: &RgbaImage, keep_linebreaks: bool) -> Result<String> {
    let [det, rec] = ensure_models()?;
    let engine = OcrEngine::new(OcrEngineParams {
        detection_model: Some(rten::Model::load_file(det).map_err(ocr_err)?),
        recognition_model: Some(rten::Model::load_file(rec).map_err(ocr_err)?),
        ..Default::default()
    })
    .map_err(ocr_err)?;
    // Screen text at 1x is small for the detector; upscaling small captures helps a lot.
    let rgb = image::DynamicImage::ImageRgba8(img.clone());
    let rgb = if img.width().max(img.height()) < 1600 {
        rgb.resize(img.width() * 2, img.height() * 2, image::imageops::FilterType::CatmullRom).into_rgb8()
    } else {
        rgb.into_rgb8()
    };
    let input = engine.prepare_input(ImageSource::from_bytes(rgb.as_raw(), rgb.dimensions()).map_err(ocr_err)?).map_err(ocr_err)?;
    let words = engine.detect_words(&input).map_err(ocr_err)?;
    let lines = engine.find_text_lines(&input, &words);
    let text: Vec<String> = engine
        .recognize_text(&input, &lines)
        .map_err(ocr_err)?
        .into_iter()
        .flatten()
        .map(|l| l.to_string().trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect();
    Ok(text.join(if keep_linebreaks { "\n" } else { " " }))
}

pub fn read_qr(img: &RgbaImage) -> Vec<String> {
    let gray = image::DynamicImage::ImageRgba8(img.clone()).into_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(gray);
    prepared.detect_grids().into_iter().filter_map(|g| g.decode().ok().map(|(_, s)| s)).collect()
}

/// QR payloads first (they are exact), otherwise recognised text.
pub fn extract(img: &RgbaImage, keep_linebreaks: bool) -> Result<String> {
    let codes = read_qr(img);
    if !codes.is_empty() {
        return Ok(codes.join("\n"));
    }
    recognize(img, keep_linebreaks)
}
