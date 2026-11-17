//! On-screen text recognition with Apple's Vision framework (macOS only).
//!
//! The manga models are tuned for speech bubbles and vertical text; for clean
//! horizontal UI text (tweets, web pages, app windows) Vision is far more
//! accurate. Lines are grouped into paragraphs so a lookup sees the whole
//! sentence, with `\n` kept where a line ends early (a real line break).

use anyhow::Result;
use image::DynamicImage;

use crate::ocr::{BBox, BlockKind, OcrBlock, OcrChar, OcrPage};

/// One recognized line in page pixels.
#[derive(Debug, Clone)]
pub struct Line {
    pub bbox: BBox,
    pub confidence: f32,
    pub chars: Vec<OcrChar>,
}

/// Box used for synthetic `\n` characters so hit-testing never lands on them.
const NOWHERE: BBox = [1e9, 1e9, 1e9, 1e9];

/// Recognize text in `img` and group it into paragraph blocks.
pub fn read_page(img: &DynamicImage) -> Result<OcrPage> {
    let lines = recognize_lines(img)?;
    Ok(OcrPage { width: img.width(), height: img.height(), blocks: group(lines) })
}

#[cfg(target_os = "macos")]
pub fn recognize_lines(img: &DynamicImage) -> Result<Vec<Line>> {
    use anyhow::anyhow;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::{AnyThread, msg_send};
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSError, NSRange, NSString};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRectangleObservation, VNRequest, VNRequestTextRecognitionLevel,
    };

    let (w, h) = (img.width() as f32, img.height() as f32);
    let mut png = Vec::new();
    img.to_rgb8()
        .write_with_encoder(PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Sub))?;

    // Vision rects are normalized with the origin at the bottom left.
    let to_px = |r: objc2_core_foundation::CGRect| -> BBox {
        let x0 = r.origin.x as f32 * w;
        let y1 = (1.0 - r.origin.y as f32) * h;
        [x0, y1 - r.size.height as f32 * h, x0 + r.size.width as f32 * w, y1]
    };

    autoreleasepool(|_| {
        let data = NSData::with_bytes(&png);
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setRecognitionLanguages(&NSArray::from_retained_slice(&[
            NSString::from_str("ja-JP"),
            NSString::from_str("en-US"),
        ]));
        request.setUsesLanguageCorrection(true);
        let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &NSDictionary::new());
        let as_request: &VNRequest = &request;
        handler
            .performRequests_error(&NSArray::from_slice(&[as_request]))
            .map_err(|e| anyhow!("Vision text recognition failed: {}", e.localizedDescription()))?;

        let mut lines = Vec::new();
        for obs in request.results().unwrap_or_default().iter() {
            let Some(top) = obs.topCandidates(1).firstObject() else { continue };
            let text = top.string().to_string();
            if text.trim().is_empty() {
                continue;
            }
            let line_box = to_px(unsafe { obs.boundingBox() });
            // Per-character boxes; Vision may only resolve word-sized ranges.
            let mut chars = Vec::new();
            let mut i = 0usize;
            for c in text.chars() {
                let len = c.len_utf16();
                let range = NSRange::new(i, len);
                let res: Result<Retained<VNRectangleObservation>, Retained<NSError>> =
                    unsafe { msg_send![&*top, boundingBoxForRange: range, error: _] };
                let bbox = res.ok().map(|o| to_px(unsafe { o.boundingBox() })).unwrap_or(line_box);
                chars.push(OcrChar { c: c.to_string(), bbox });
                i += len;
            }
            refine_char_boxes(&mut chars, line_box);
            lines.push(Line { bbox: line_box, confidence: top.confidence(), chars });
        }
        Ok(lines)
    })
}

#[cfg(not(target_os = "macos"))]
pub fn recognize_lines(_img: &DynamicImage) -> Result<Vec<Line>> {
    anyhow::bail!("screen text recognition needs macOS")
}

fn is_wide(c: &str) -> bool {
    c.chars().next().is_some_and(|c| !c.is_ascii() && !('\u{ff61}'..='\u{ff9f}').contains(&c))
}

/// Vision often returns one box for a whole word. Where several characters
/// share a box, split it in proportion to their widths (CJK ≈ 2× ASCII).
fn refine_char_boxes(chars: &mut [OcrChar], line: BBox) {
    let mut i = 0;
    while i < chars.len() {
        let mut j = i + 1;
        while j < chars.len() && chars[j].bbox == chars[i].bbox {
            j += 1;
        }
        if j - i > 1 {
            let b = chars[i].bbox;
            let weights: Vec<f32> = chars[i..j].iter().map(|c| if is_wide(&c.c) { 1.0 } else { 0.55 }).collect();
            let total: f32 = weights.iter().sum();
            let mut x = b[0];
            for (k, wgt) in weights.iter().enumerate() {
                let dx = (b[2] - b[0]) * wgt / total;
                chars[i + k].bbox = [x, line[1], x + dx, line[3]];
                x += dx;
            }
        }
        i = j;
    }
}

fn height(b: &BBox) -> f32 {
    b[3] - b[1]
}

fn union(a: BBox, b: BBox) -> BBox {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

/// Merge pieces of the same visual line, then stack lines into paragraphs.
pub fn group(mut lines: Vec<Line>) -> Vec<OcrBlock> {
    // 1. Pieces on the same baseline that nearly touch are one line.
    lines.sort_by(|a, b| a.bbox[0].total_cmp(&b.bbox[0]));
    let mut rows: Vec<Line> = Vec::new();
    for l in lines {
        let h = height(&l.bbox);
        let found = rows.iter_mut().find(|r| {
            let overlap = r.bbox[3].min(l.bbox[3]) - r.bbox[1].max(l.bbox[1]);
            let gap = l.bbox[0] - r.bbox[2];
            overlap > 0.6 * h.min(height(&r.bbox)) && gap < 1.5 * h && gap > -0.5 * h
        });
        match found {
            Some(r) => {
                r.bbox = union(r.bbox, l.bbox);
                r.confidence = r.confidence.min(l.confidence);
                r.chars.extend(l.chars);
            }
            None => rows.push(l),
        }
    }

    // 2. Lines of similar height, left-aligned and closely stacked form a paragraph.
    rows.sort_by(|a, b| a.bbox[1].total_cmp(&b.bbox[1]));
    let mut paras: Vec<Vec<Line>> = Vec::new();
    for l in rows {
        let h = height(&l.bbox);
        let found = paras.iter_mut().rev().find(|p| {
            let last = p.last().unwrap();
            let lh = height(&last.bbox);
            let left = p.iter().map(|x| x.bbox[0]).fold(f32::MAX, f32::min);
            let gap = l.bbox[1] - last.bbox[3];
            (0.7..=1.4).contains(&(h / lh)) && gap > -0.3 * lh && gap < 0.8 * lh && (l.bbox[0] - left).abs() < 1.5 * lh
        });
        match found {
            Some(p) => p.push(l),
            None => paras.push(vec![l]),
        }
    }

    paras
        .into_iter()
        .map(|p| {
            let right = p.iter().map(|l| l.bbox[2]).fold(f32::MIN, f32::max);
            let mut bbox = p[0].bbox;
            let mut chars = Vec::new();
            let mut score = 1.0f32;
            let n = p.len();
            for (i, l) in p.into_iter().enumerate() {
                bbox = union(bbox, l.bbox);
                score = score.min(l.confidence);
                let ends_early = l.bbox[2] < right - 2.0 * height(&l.bbox);
                chars.extend(l.chars);
                if i + 1 < n && ends_early {
                    chars.push(OcrChar { c: "\n".into(), bbox: NOWHERE });
                }
            }
            OcrBlock {
                bbox,
                kind: BlockKind::Free,
                bubble: None,
                score,
                vertical: false,
                text: chars.iter().map(|c| c.c.as_str()).collect(),
                chars,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x0: f32, y0: f32, x1: f32) -> Line {
        let n = text.chars().count() as f32;
        let step = (x1 - x0) / n;
        let chars = text
            .chars()
            .enumerate()
            .map(|(i, c)| OcrChar { c: c.to_string(), bbox: [x0 + i as f32 * step, y0, x0 + (i + 1) as f32 * step, y0 + 30.0] })
            .collect();
        Line { bbox: [x0, y0, x1, y0 + 30.0], confidence: 0.9, chars }
    }

    #[test]
    fn groups_wrapped_and_broken_lines() {
        let blocks = group(vec![
            line("Isla @Isla · 4時間", 100.0, 0.0, 400.0),
            line("ゴジラの話は山崎貴なので酷いなぁと思ったけど、劇伴には", 100.0, 45.0, 1000.0),
            line("ムカついてきたな", 100.0, 84.0, 360.0),
            line("ジョンが宇宙人と作った旋律", 100.0, 123.0, 980.0),
            line("別の欄", 1200.0, 45.0, 1300.0),
        ]);
        let texts: Vec<_> = blocks.iter().map(|b| b.text.as_str()).collect();
        assert!(texts.contains(&"Isla @Isla · 4時間\nゴジラの話は山崎貴なので酷いなぁと思ったけど、劇伴にはムカついてきたな\nジョンが宇宙人と作った旋律"));
        assert!(texts.contains(&"別の欄"));
    }

    #[test]
    fn splits_shared_word_boxes() {
        let mut chars: Vec<OcrChar> = "劇伴ab".chars().map(|c| OcrChar { c: c.to_string(), bbox: [0.0, 0.0, 31.0, 10.0] }).collect();
        refine_char_boxes(&mut chars, [0.0, 0.0, 31.0, 10.0]);
        assert!((chars[0].bbox[2] - 10.0).abs() < 0.01);
        assert!((chars[3].bbox[2] - 31.0).abs() < 0.01);
    }
}
