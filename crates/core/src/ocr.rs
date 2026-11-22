//! Manga OCR: RT-DETR text/bubble detection + charspot per-character recognition, via ONNX Runtime.
//!
//! The recognizer returns every character with its own box, so overlays can
//! place each glyph exactly and hover hit-testing maps straight to a text offset.

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, anyhow};
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, RgbImage};
use ort::session::Session;
use ort::session::builder::{AutoDevicePolicy, GraphOptimizationLevel};
use ort::value::Tensor;
use serde::{Deserialize, Serialize};

use crate::models;

/// x0, y0, x1, y1 in page pixels.
pub type BBox = [f32; 4];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// Text inside a speech bubble.
    Bubble,
    /// Text drawn outside bubbles (narration, asides, handwritten notes).
    Free,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrChar {
    pub c: String,
    #[serde(rename = "box")]
    pub bbox: BBox,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrBlock {
    #[serde(rename = "box")]
    pub bbox: BBox,
    pub kind: BlockKind,
    /// The speech bubble containing this text, if any (used for card crops).
    pub bubble: Option<BBox>,
    pub score: f32,
    pub vertical: bool,
    pub text: String,
    /// Characters in reading order; `text` is their concatenation.
    pub chars: Vec<OcrChar>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrPage {
    pub width: u32,
    pub height: u32,
    pub blocks: Vec<OcrBlock>,
}

const DET_SIZE: u32 = 640;
const DET_THRESHOLD: f32 = 0.4;
const CELL: u32 = 16;
const BUDGET: f64 = 256.0;
/// tate-chū-yoko glyphs appended after the classifier's 6,818 classes.
fn combos() -> Vec<String> {
    let mut v = Vec::new();
    for a in ['!', '?'] {
        for b in ['!', '?'] {
            v.push(format!("{a}{b}"));
        }
    }
    for a in ['!', '?'] {
        for b in ['!', '?'] {
            for c in ['!', '?'] {
                v.push(format!("{a}{b}{c}"));
            }
        }
    }
    v
}

pub struct OcrEngine {
    det: Mutex<Session>,
    rec: Mutex<Session>,
    vocab: Vec<String>,
}

fn environment() -> Result<&'static ort::environment::Environment> {
    static ENV: OnceLock<ort::environment::Environment> = OnceLock::new();
    if let Some(e) = ENV.get() {
        return Ok(e);
    }
    let env = ort::init().with_name("picyomi").build().map_err(|e| anyhow!("onnxruntime init: {e}"))?;
    Ok(ENV.get_or_init(|| env))
}

fn session(path: &Path, threads: usize) -> Result<Session> {
    let err = |e: ort::Error<ort::session::builder::SessionBuilder>| anyhow!("{e}");
    Session::builder(environment()?)
        .map_err(|e| anyhow!("{e}"))?
        // On macOS the default policy auto-registers CoreML, which fails to compile the
        // int8 detector ("Espresso exception") and is not faster for these small models.
        .with_auto_device(AutoDevicePolicy::PreferCPU)
        .map_err(err)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(err)?
        .with_intra_threads(threads)
        .map_err(err)?
        .commit_from_file(path)
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("load {}", path.display()))
}

impl OcrEngine {
    /// Load from the downloaded model files.
    pub fn load() -> Result<Self> {
        Self::load_from(
            &models::path_of(&models::DETECTOR),
            &models::path_of(&models::RECOGNIZER),
            &models::path_of(&models::RECOGNIZER_VOCAB),
        )
    }

    pub fn load_from(det: &Path, rec: &Path, vocab: &Path) -> Result<Self> {
        #[derive(Deserialize)]
        struct Vocab {
            characters: Vec<String>,
        }
        let v: Vocab = serde_json::from_slice(&std::fs::read(vocab)?)?;
        let mut vocab = v.characters;
        vocab.resize(6818, String::new());
        vocab.extend(combos());
        Ok(Self { det: Mutex::new(session(det, 4)?), rec: Mutex::new(session(rec, 4)?), vocab })
    }

    /// Full page: detect text regions, then read each one.
    pub fn read_page(&self, img: &DynamicImage) -> Result<OcrPage> {
        let rgb = img.to_rgb8();
        let (w, h) = rgb.dimensions();
        let (texts, bubbles) = self.detect(&rgb)?;
        let mut blocks = Vec::with_capacity(texts.len());
        for (bbox, kind, score) in texts {
            let pad = ((bbox[2] - bbox[0]).min(bbox[3] - bbox[1]) * 0.04).clamp(2.0, 8.0);
            let x0 = (bbox[0] - pad).max(0.0).floor() as u32;
            let y0 = (bbox[1] - pad).max(0.0).floor() as u32;
            let x1 = ((bbox[2] + pad).min(w as f32).ceil() as u32).max(x0 + 1);
            let y1 = ((bbox[3] + pad).min(h as f32).ceil() as u32).max(y0 + 1);
            let crop = image::imageops::crop_imm(&rgb, x0, y0, x1 - x0, y1 - y0).to_image();
            let mut chars = self.read_crop(&crop)?;
            if chars.is_empty() {
                continue;
            }
            for ch in &mut chars {
                ch.bbox = [ch.bbox[0] + x0 as f32, ch.bbox[1] + y0 as f32, ch.bbox[2] + x0 as f32, ch.bbox[3] + y0 as f32];
            }
            let bubble = bubbles
                .iter()
                .map(|b| (containment(&bbox, b), *b))
                .filter(|(c, _)| *c > 0.7)
                .max_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, b)| b);
            blocks.push(OcrBlock {
                bbox,
                kind,
                bubble,
                score,
                vertical: is_vertical(&chars, &bbox),
                text: chars.iter().map(|c| c.c.as_str()).collect(),
                chars,
            });
        }
        // Reading order for the page: right-to-left, top-to-bottom (manga convention), by block centre rows.
        blocks.sort_by(|a, b| {
            let (ay, by) = (a.bbox[1], b.bbox[1]);
            let row = (h as f32 * 0.12).max(1.0);
            ((ay / row) as i32).cmp(&((by / row) as i32)).then(b.bbox[2].total_cmp(&a.bbox[2]))
        });
        Ok(OcrPage { width: w, height: h, blocks })
    }

    /// Returns (text boxes with kind and score, bubble boxes).
    fn detect(&self, img: &RgbImage) -> Result<(Vec<(BBox, BlockKind, f32)>, Vec<BBox>)> {
        let (w, h) = img.dimensions();
        let small = image::imageops::resize(img, DET_SIZE, DET_SIZE, FilterType::Triangle);
        let n = (DET_SIZE * DET_SIZE) as usize;
        let mut data = vec![0f32; 3 * n];
        for (i, p) in small.pixels().enumerate() {
            for c in 0..3 {
                data[c * n + i] = p[c] as f32 / 255.0;
            }
        }
        let images = Tensor::from_array(([1usize, 3, DET_SIZE as usize, DET_SIZE as usize], data))?;
        let sizes = Tensor::from_array(([1usize, 2], vec![w as i64, h as i64]))?;
        let mut sess = self.det.lock().unwrap_or_else(|e| e.into_inner());
        let out = sess.run(ort::inputs!["images" => images, "orig_target_sizes" => sizes])?;
        let (_, labels) = out["labels"].try_extract_tensor::<i64>()?;
        let (_, boxes) = out["boxes"].try_extract_tensor::<f32>()?;
        let (_, scores) = out["scores"].try_extract_tensor::<f32>()?;

        let mut texts: Vec<(BBox, BlockKind, f32)> = Vec::new();
        let mut bubbles = Vec::new();
        for i in 0..scores.len() {
            if scores[i] < DET_THRESHOLD {
                continue;
            }
            let b = [
                boxes[i * 4].clamp(0.0, w as f32),
                boxes[i * 4 + 1].clamp(0.0, h as f32),
                boxes[i * 4 + 2].clamp(0.0, w as f32),
                boxes[i * 4 + 3].clamp(0.0, h as f32),
            ];
            if b[2] - b[0] < 4.0 || b[3] - b[1] < 4.0 {
                continue;
            }
            match labels[i] {
                0 => bubbles.push(b),
                1 => texts.push((b, BlockKind::Bubble, scores[i])),
                _ => texts.push((b, BlockKind::Free, scores[i])),
            }
        }
        // The detector sometimes reports the same text as both bubble and free text: keep the more confident box.
        texts.sort_by(|a, b| b.2.total_cmp(&a.2));
        let mut kept: Vec<(BBox, BlockKind, f32)> = Vec::new();
        for t in texts {
            if kept.iter().any(|k| iou(&k.0, &t.0) > 0.5 || containment(&t.0, &k.0) > 0.8 || containment(&k.0, &t.0) > 0.8) {
                continue;
            }
            kept.push(t);
        }
        Ok((kept, bubbles))
    }

    /// Read one text crop; boxes are in crop pixels, characters in reading order.
    pub fn read_crop(&self, crop: &RgbImage) -> Result<Vec<OcrChar>> {
        let (cw, ch) = crop.dimensions();
        let (scale, gh, gw) = pad_grid(ch as f64, cw as f64);
        let rw = ((cw as f64 * scale).round() as u32).clamp(1, gw * CELL);
        let rh = ((ch as f64 * scale).round() as u32).clamp(1, gh * CELL);
        let small = image::imageops::resize(crop, rw, rh, FilterType::CatmullRom);
        let fill = median_border(&small);
        let (iw, ih) = ((gw * CELL) as usize, (gh * CELL) as usize);
        let n = iw * ih;
        let mut data = vec![0f32; 3 * n];
        for c in 0..3 {
            data[c * n..(c + 1) * n].fill(fill[c] as f32);
        }
        for (x, y, p) in small.enumerate_pixels() {
            let i = y as usize * iw + x as usize;
            for c in 0..3 {
                data[c * n + i] = p[c] as f32;
            }
        }
        let input = Tensor::from_array(([1usize, 3, ih, iw], data))?;
        let mut sess = self.rec.lock().unwrap_or_else(|e| e.into_inner());
        let out = sess.run(ort::inputs!["img" => input])?;
        let (_, boxes) = out["boxes"].try_extract_tensor::<f32>()?;
        let (_, ids) = out["ids"].try_extract_tensor::<i64>()?;
        let (_, order) = out["order"].try_extract_tensor::<i64>()?;
        let (fx, fy) = (cw as f32 / rw as f32, ch as f32 / rh as f32);
        Ok(order
            .iter()
            .filter_map(|&k| {
                let k = k as usize;
                let c = self.vocab.get(*ids.get(k)? as usize)?.clone();
                if c.is_empty() {
                    return None;
                }
                let b = &boxes[k * 4..k * 4 + 4];
                Some(OcrChar { c, bbox: [b[0] * fx, b[1] * fy, b[2] * fx, b[3] * fy] })
            })
            .collect())
    }
}

/// Largest scale whose 14-px grid fits the cell budget; returns (scale for 16-px cells, rows, cols).
fn pad_grid(h: f64, w: f64) -> (f64, u32, u32) {
    let (mut lo, mut hi) = (1e-4f64, 1000.0f64);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if (h * mid / 14.0).ceil() * (w * mid / 14.0).ceil() <= BUDGET {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo * CELL as f64 / 14.0, (h * lo / 14.0).ceil() as u32, (w * lo / 14.0).ceil() as u32)
}

fn median_border(img: &RgbImage) -> [u8; 3] {
    let (w, h) = img.dimensions();
    let mut ch: [Vec<u8>; 3] = Default::default();
    let mut push = |x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        for c in 0..3 {
            ch[c].push(p[c]);
        }
    };
    for x in 0..w {
        push(x, 0);
        push(x, h - 1);
    }
    for y in 0..h {
        push(0, y);
        push(w - 1, y);
    }
    let mut out = [0u8; 3];
    for c in 0..3 {
        let v = &mut ch[c];
        v.sort_unstable();
        let m = v.len();
        out[c] = if m % 2 == 1 { v[m / 2] } else { ((v[m / 2 - 1] as u16 + v[m / 2] as u16) / 2) as u8 };
    }
    out
}

fn area(b: &BBox) -> f32 {
    (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0)
}

fn intersection(a: &BBox, b: &BBox) -> f32 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    w * h
}

fn iou(a: &BBox, b: &BBox) -> f32 {
    let i = intersection(a, b);
    i / (area(a) + area(b) - i + 1e-6)
}

/// Share of `inner` covered by `outer`.
fn containment(inner: &BBox, outer: &BBox) -> f32 {
    intersection(inner, outer) / (area(inner) + 1e-6)
}

/// Vertical if consecutive characters mostly advance downwards rather than rightwards.
fn is_vertical(chars: &[OcrChar], bbox: &BBox) -> bool {
    let (mut v, mut hz) = (0, 0);
    for w in chars.windows(2) {
        let (a, b) = (&w[0].bbox, &w[1].bbox);
        let dx = ((b[0] + b[2]) - (a[0] + a[2])).abs() / 2.0;
        let dy = ((b[1] + b[3]) - (a[1] + a[3])).abs() / 2.0;
        if dy > dx { v += 1 } else { hz += 1 }
    }
    if v + hz == 0 {
        return bbox[3] - bbox[1] > bbox[2] - bbox[0];
    }
    v >= hz
}

/// Decode an image from bytes (jpeg / png / webp / gif / bmp).
pub fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    Ok(image::load_from_memory(bytes)?)
}

pub fn dims(img: &DynamicImage) -> (u32, u32) {
    img.dimensions()
}
