//! Global screen lookup: cursor position, screen capture around it, and hit-testing OCR output.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use image::{DynamicImage, RgbaImage};
use picyomi_core::ocr::{OcrBlock, OcrPage};
use picyomi_core::service::Service;

/// Cursor position in global display points (origin top-left of the main display).
pub fn cursor_position() -> Option<(f64, f64)> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    let src = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
    let ev = CGEvent::new(src).ok()?;
    let p = ev.location();
    Some((p.x, p.y))
}

/// A capture of the area around the cursor.
pub struct Capture {
    pub image: DynamicImage,
    /// Cursor inside `image`, in image pixels.
    pub cursor_px: (f32, f32),
    /// Image pixels per point.
    pub scale: f32,
    /// Top-left of the captured region in global points.
    pub origin: (f64, f64),
    /// Size of the captured region in points.
    pub size: (f64, f64),
}

impl Capture {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.origin.0 && y >= self.origin.1 && x < self.origin.0 + self.size.0 && y < self.origin.1 + self.size.1
    }

    /// Global point → image pixel.
    pub fn to_px(&self, x: f64, y: f64) -> (f32, f32) {
        (((x - self.origin.0) as f32) * self.scale, ((y - self.origin.1) as f32) * self.scale)
    }
}

/// Region (in points) captured around the cursor.
const HALF_W: i32 = 520;
const HALF_H: i32 = 380;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Without Screen Recording permission macOS still returns an image, but with
/// only the wallpaper and no window contents, so check explicitly first.
pub fn ensure_capture_access() -> Result<()> {
    if unsafe { CGPreflightScreenCaptureAccess() } {
        return Ok(());
    }
    unsafe { CGRequestScreenCaptureAccess() };
    bail!(
        "pic_yomi 还没有「屏幕录制」权限。\n打开 系统设置 → 隐私与安全性 → 屏幕录制，打开 pic_yomi 的开关（列表里已有 pic_yomi 的话，先关再开，或用「−」删掉后重新添加），然后退出并重新打开 pic_yomi。"
    )
}

pub fn capture_around(x: f64, y: f64) -> Result<Capture> {
    ensure_capture_access()?;
    let monitor = xcap::Monitor::from_point(x as i32, y as i32).context("no display under the cursor")?;
    let (mx, my) = (monitor.x()?, monitor.y()?);
    let (mw, mh) = (monitor.width()? as i32, monitor.height()? as i32);
    let (lx, ly) = (x as i32 - mx, y as i32 - my);
    let w = (HALF_W * 2).min(mw);
    let h = (HALF_H * 2).min(mh);
    let rx = (lx - HALF_W).clamp(0, mw - w);
    let ry = (ly - HALF_H).clamp(0, mh - h);
    let img: RgbaImage = monitor
        .capture_region(rx as u32, ry as u32, w as u32, h as u32)
        .context("screen capture failed (is Screen Recording allowed for pic_yomi?)")?;
    let scale = img.width() as f32 / w as f32;
    Ok(Capture {
        cursor_px: ((lx - rx) as f32 * scale, (ly - ry) as f32 * scale),
        scale,
        origin: ((mx + rx) as f64, (my + ry) as f64),
        size: (w as f64, h as f64),
        image: DynamicImage::ImageRgba8(img),
    })
}

fn is_japanese(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '\u{ff66}'..='\u{ff9f}')
}

/// Recognized text of one capture, from both recognizers.
///
/// Apple Vision reads clean horizontal text (tweets, web pages, app UI) almost
/// perfectly but cannot read vertical text; the manga models read speech
/// bubbles and vertical text.
pub struct Pages {
    pub vision: Option<OcrPage>,
    pub manga: Option<OcrPage>,
    pub manga_missing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Vision,
    Manga,
}

impl Pages {
    pub fn block(&self, src: Source, i: usize) -> Option<&OcrBlock> {
        match src {
            Source::Vision => self.vision.as_ref()?.blocks.get(i),
            Source::Manga => self.manga.as_ref()?.blocks.get(i),
        }
    }
}

/// Run both recognizers side by side.
pub fn analyze(service: &Service, image: Arc<DynamicImage>) -> Pages {
    let (vision, manga) = std::thread::scope(|s| {
        let manga = s.spawn(|| {
            service.ocr_ready().then(|| service.ocr(&format!("screen:{}", picyomi_core::now_ms()), image.clone()))
        });
        let vision = picyomi_core::vision::read_page(&image);
        (vision, manga.join().unwrap_or(None))
    });
    Pages {
        vision: vision.map_err(|e| eprintln!("vision: {e:#}")).ok(),
        manga_missing: manga.is_none(),
        manga: manga.and_then(|r| r.map_err(|e| eprintln!("manga ocr: {e:#}")).ok()),
    }
}

/// The text under the cursor: which page, block index and character offset.
/// Vertical manga text under the cursor wins; otherwise the nearest Japanese
/// text Vision found; otherwise the manga result.
pub fn pick(pages: &Pages, cursor: (f32, f32), scale: f32) -> Option<(Source, usize, usize)> {
    let near = 40.0 * scale;
    let manga_hit = pages.manga.as_ref().and_then(|p| hit(p, cursor, near));
    if let Some((bi, offset, d)) = manga_hit {
        if pages.manga.as_ref()?.blocks[bi].vertical && d <= 4.0 * scale {
            return Some((Source::Manga, bi, offset));
        }
    }
    let vision_hit = pages.vision.as_ref().and_then(|p| {
        hit(p, cursor, near).filter(|(bi, off, _)| {
            let b = &p.blocks[*bi];
            b.text.chars().nth(*off).is_some_and(is_japanese) && b.text.chars().filter(|c| is_japanese(*c)).count() >= 2
        })
    });
    if let Some((bi, offset, _)) = vision_hit {
        return Some((Source::Vision, bi, offset));
    }
    manga_hit.map(|(bi, offset, _)| (Source::Manga, bi, offset))
}

/// One-shot lookup (⌥⌘J): the block and character offset under the cursor.
pub fn read_at(service: &Service, image: Arc<DynamicImage>, cursor: (f32, f32), scale: f32) -> Result<Option<(OcrBlock, usize)>> {
    let pages = analyze(service, image.clone());
    if let Some((src, bi, offset)) = pick(&pages, cursor, scale) {
        return Ok(pages.block(src, bi).map(|b| (b.clone(), offset)));
    }
    // Keep the capture that found nothing, to diagnose what was on screen.
    let _ = image.to_rgb8().save(picyomi_core::db::data_dir().join("last-capture.jpg"));
    if pages.manga_missing {
        bail!("鼠标附近没有识别到横排日语。漫画竖排文字需要 OCR 模型：打开 pic_yomi → 设置 → OCR 模型 → 下载模型。");
    }
    Ok(None)
}

/// Boxes (image pixels) covering `len` characters from `offset`, one per text line.
pub fn word_rects(block: &OcrBlock, offset: usize, len: usize) -> Vec<[f32; 4]> {
    let first = char_index(block, offset);
    let mut rects: Vec<[f32; 4]> = Vec::new();
    let mut taken = 0;
    for ch in block.chars.iter().skip(first) {
        if taken >= len.max(1) {
            break;
        }
        taken += ch.c.chars().count();
        let b = ch.bbox;
        if b[0] > 1e8 {
            continue; // synthetic line break
        }
        // Same line (horizontal) or column (vertical) as the previous glyph: extend it.
        let same = rects.last().is_some_and(|r| {
            if block.vertical {
                r[0].max(b[0]) < r[2].min(b[2]) && b[1] >= r[1] - 2.0
            } else {
                r[1].max(b[1]) < r[3].min(b[3]) && b[0] >= r[0] - 2.0
            }
        });
        match rects.last_mut() {
            Some(r) if same => *r = [r[0].min(b[0]), r[1].min(b[1]), r[2].max(b[2]), r[3].max(b[3])],
            _ => rects.push(b),
        }
    }
    rects
}

/// Nearest block within `max_dist`, the offset of the glyph nearest the point, and the distance to that glyph.
fn hit(page: &OcrPage, p: (f32, f32), max_dist: f32) -> Option<(usize, usize, f32)> {
    let (bi, offset) = hit_test(page, p, max_dist)?;
    let block = &page.blocks[bi];
    let glyph = block.chars.get(char_index(block, offset))?;
    Some((bi, offset, dist_to_box(p, &glyph.bbox)))
}

/// Index into `block.chars` of the glyph that starts at text offset `offset`.
fn char_index(block: &OcrBlock, offset: usize) -> usize {
    let mut pos = 0;
    for (i, ch) in block.chars.iter().enumerate() {
        let n = ch.c.chars().count();
        if offset < pos + n {
            return i;
        }
        pos += n;
    }
    block.chars.len().saturating_sub(1)
}

fn dist_to_box(p: (f32, f32), b: &[f32; 4]) -> f32 {
    let dx = (b[0] - p.0).max(0.0).max(p.0 - b[2]);
    let dy = (b[1] - p.1).max(0.0).max(p.1 - b[3]);
    (dx * dx + dy * dy).sqrt()
}

/// The text block under (or nearest to) the point, and the character offset to look up from.
pub fn hit_test(page: &OcrPage, p: (f32, f32), max_dist: f32) -> Option<(usize, usize)> {
    let (bi, block, d) = page
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (i, b, dist_to_box(p, &b.bbox)))
        .min_by(|a, b| a.2.total_cmp(&b.2))?;
    if d > max_dist {
        return None;
    }
    Some((bi, char_offset(block, p)))
}

/// Offset (in characters of `block.text`) of the glyph nearest to `p`.
pub fn char_offset(block: &OcrBlock, p: (f32, f32)) -> usize {
    let mut best = (0usize, f32::MAX);
    let mut offset = 0usize;
    for ch in &block.chars {
        let d = dist_to_box(p, &ch.bbox);
        if d < best.1 {
            best = (offset, d);
        }
        offset += ch.c.chars().count();
    }
    best.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use picyomi_core::ocr::{BlockKind, OcrChar};

    #[test]
    fn hit_test_picks_nearest_glyph() {
        let chars = vec![
            OcrChar { c: "ヤ".into(), bbox: [0.0, 0.0, 10.0, 10.0] },
            OcrChar { c: "モ".into(), bbox: [0.0, 10.0, 10.0, 20.0] },
            OcrChar { c: "リ".into(), bbox: [0.0, 20.0, 10.0, 30.0] },
        ];
        let page = OcrPage {
            width: 100,
            height: 100,
            blocks: vec![OcrBlock {
                bbox: [0.0, 0.0, 10.0, 30.0],
                kind: BlockKind::Bubble,
                bubble: None,
                score: 1.0,
                vertical: true,
                text: "ヤモリ".into(),
                chars,
            }],
        };
        assert_eq!(hit_test(&page, (5.0, 15.0), 20.0), Some((0, 1)));
        assert_eq!(hit_test(&page, (80.0, 80.0), 20.0), None);
    }

    #[test]
    fn word_rects_merge_per_line() {
        let ch = |c: &str, b: [f32; 4]| OcrChar { c: c.into(), bbox: b };
        let block = OcrBlock {
            bbox: [0.0, 0.0, 40.0, 40.0],
            kind: BlockKind::Free,
            bubble: None,
            score: 1.0,
            vertical: false,
            text: "あ劇伴\nに".into(),
            chars: vec![
                ch("あ", [0.0, 0.0, 10.0, 10.0]),
                ch("劇", [10.0, 0.0, 20.0, 10.0]),
                ch("伴", [20.0, 0.0, 30.0, 10.0]),
                ch("\n", [1e9, 1e9, 1e9, 1e9]),
                ch("に", [0.0, 12.0, 10.0, 22.0]),
            ],
        };
        assert_eq!(word_rects(&block, 1, 2), vec![[10.0, 0.0, 30.0, 10.0]]);
        assert_eq!(word_rects(&block, 2, 3), vec![[20.0, 0.0, 30.0, 10.0], [0.0, 12.0, 10.0, 22.0]]);
    }

    /// Real screenshot end to end: `PICYOMI_TEST_SHOT=shot.png PICYOMI_TEST_AT=x,y cargo test -p picyomi -- --ignored`.
    #[test]
    #[ignore]
    fn screenshot_lookup() {
        let path = std::env::var("PICYOMI_TEST_SHOT").unwrap();
        let at: Vec<f32> = std::env::var("PICYOMI_TEST_AT").unwrap().split(',').map(|v| v.parse().unwrap()).collect();
        let svc = Service::open().unwrap();
        let img = Arc::new(picyomi_core::ocr::decode(&std::fs::read(path).unwrap()).unwrap());
        let (block, offset) = read_at(&svc, img, (at[0], at[1]), 2.0).unwrap().unwrap();
        let (sentence, off) = picyomi_core::lookup::sentence_at(&block.text, offset);
        let r = svc.lookup(&sentence, off).unwrap();
        let e = &r.result.entries[0];
        println!("sentence: {sentence}\noffset: {off}\nword: {}【{}】 entries: {}", e.expression, e.reading, r.result.entries.len());
    }
}
