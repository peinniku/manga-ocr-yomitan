//! Hold Shift and move the mouse to look up words anywhere on screen, like Yomitan.
//!
//! The screen around the cursor is captured and recognized once per Shift
//! hold; after that, moving the mouse only hit-tests the cached text, so the
//! highlight and popup follow the pointer instantly.

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use image::DynamicImage;
use picyomi_core::lookup::sentence_at;

use crate::panel::Picture;
use crate::state::svc;
use crate::{native, popup, screen};

/// Setting: "0" turns hold-Shift scanning off.
pub const SETTING_ENABLED: &str = "scan.shift";
/// Setting: "1" also scans in web browsers (where the extension normally does it).
pub const SETTING_BROWSERS: &str = "scan.browsers";

/// Browsers get the pic_yomi extension, which reads page text exactly; by
/// default the desktop scanner stays out of their way.
const BROWSERS: &[&str] = &[
    "app.zen-browser.zen",
    "org.mozilla.firefox",
    "org.mozilla.firefoxdeveloperedition",
    "org.mozilla.nightly",
    "com.google.Chrome",
    "com.google.Chrome.canary",
    "com.apple.Safari",
    "company.thebrowser.Browser",
    "com.microsoft.edgemac",
    "com.brave.Browser",
    "com.vivaldi.Vivaldi",
    "com.operasoftware.Opera",
];

const TICK: Duration = Duration::from_millis(25);
/// Shift alone starts a scan once the mouse moves this far, or after this long.
const MOVE_PT: f64 = 3.0;
const HOLD: Duration = Duration::from_millis(450);

struct Hold {
    start: Instant,
    pos: (f64, f64),
    keys: u32,
    armed: bool,
    /// Typing (Shift + letter) or a disallowed app: ignore the rest of this hold.
    dead: bool,
}

struct Scanned {
    capture: screen::Capture,
    image: Arc<DynamicImage>,
    pages: screen::Pages,
}

#[derive(Default)]
struct Scanner {
    hold: Option<Hold>,
    scanned: Option<Rc<Scanned>>,
    shown: Option<(screen::Source, usize, usize)>,
    button_was_down: bool,
}

pub fn setting_on(key: &str, default: bool, cx: &App) -> bool {
    match svc(cx).db.get_setting(key).ok().flatten().as_deref() {
        Some("1") => true,
        Some("0") => false,
        _ => default,
    }
}

fn allowed_here(cx: &App) -> bool {
    if !setting_on(SETTING_ENABLED, true, cx) {
        return false;
    }
    if setting_on(SETTING_BROWSERS, false, cx) {
        return true;
    }
    !native::frontmost_bundle_id().is_some_and(|id| BROWSERS.contains(&id.as_str()))
}

pub fn start(cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut s = Scanner::default();
        loop {
            cx.background_executor().timer(TICK).await;
            let request = cx.update(|cx| tick(&mut s, cx));
            if let Some((x, y)) = request {
                scan_at(&mut s, x, y, cx).await;
            }
        }
    })
    .detach();
}

/// One poll of keyboard and mouse. Returns a point to capture around.
fn tick(s: &mut Scanner, cx: &mut App) -> Option<(f64, f64)> {
    let (x, y) = screen::cursor_position()?;

    // A click outside the popup closes it.
    let down = native::left_button_down();
    if down && !s.button_was_down && !popup::popup_contains(x, y, cx) && popup::close_all(cx) {
        s.shown = None;
    }
    s.button_was_down = down;

    let mods = native::modifiers();
    let shift_only = mods & native::SHIFT != 0 && mods & (native::CONTROL | native::OPTION | native::COMMAND) == 0;
    if !shift_only {
        s.hold = None;
        return None;
    }
    let keys = native::key_down_count();
    let hold = s.hold.get_or_insert_with(|| Hold { start: Instant::now(), pos: (x, y), keys, armed: false, dead: false });
    if hold.dead {
        return None;
    }
    if keys != hold.keys {
        hold.dead = true; // Shift is being used to type capitals
        return None;
    }
    if popup::popup_contains(x, y, cx) {
        return None; // reading the popup, not the screen
    }
    if !hold.armed {
        let moved = (x - hold.pos.0).hypot(y - hold.pos.1) >= MOVE_PT;
        if !moved && hold.start.elapsed() < HOLD {
            return None;
        }
        if !allowed_here(cx) {
            hold.dead = true;
            return None;
        }
        hold.armed = true;
        return Some((x, y)); // a fresh capture for every new hold
    }
    let scanned = s.scanned.clone()?;
    if !scanned.capture.contains(x, y) {
        return Some((x, y));
    }
    show_word(s, &scanned, x, y, cx);
    None
}

/// Capture and recognize around the point, then show the word under the cursor.
async fn scan_at(s: &mut Scanner, x: f64, y: f64, cx: &mut AsyncApp) {
    s.scanned = None;
    s.shown = None;
    // The popup and highlight must not be in the screenshot.
    if cx.update(|cx| popup::close_all(cx)) {
        cx.background_executor().timer(Duration::from_millis(120)).await;
    }
    let mut capture = match cx.update(|_| screen::capture_around(x, y)) {
        Ok(c) => c,
        Err(e) => {
            let _ = cx.update(|cx| popup::show_message(x, y, format!("{e:#}"), cx));
            if let Some(h) = s.hold.as_mut() {
                h.dead = true;
            }
            return;
        }
    };
    let image = Arc::new(std::mem::replace(&mut capture.image, DynamicImage::new_rgb8(1, 1)));
    let service = cx.update(|cx| svc(cx));
    let img = image.clone();
    let pages = cx.background_spawn(async move { screen::analyze(&service, img) }).await;
    let scanned = Rc::new(Scanned { capture, image, pages });
    s.scanned = Some(scanned.clone());
    // Show the word where the pointer is now, even if Shift was let go meanwhile.
    let _ = cx.update(|cx| {
        if let Some((x, y)) = screen::cursor_position() {
            if scanned.capture.contains(x, y) {
                show_word(s, &scanned, x, y, cx);
            }
        }
    });
}

fn show_word(s: &mut Scanner, sc: &Scanned, x: f64, y: f64, cx: &mut App) {
    let cap = &sc.capture;
    let Some(hit) = screen::pick(&sc.pages, cap.to_px(x, y), cap.scale) else { return };
    if s.shown == Some(hit) {
        return;
    }
    s.shown = Some(hit);
    let (src, bi, offset) = hit;
    let Some(block) = sc.pages.block(src, bi) else { return };
    let (sentence, sentence_offset) = sentence_at(&block.text, offset);
    let Ok(response) = svc(cx).lookup(&sentence, sentence_offset) else { return };
    if response.result.entries.is_empty() {
        return; // keep showing the last word, like Yomitan
    }
    let scale = f64::from(cap.scale);
    let rects: Vec<[f64; 4]> = screen::word_rects(block, offset, response.result.match_length)
        .into_iter()
        .map(|r| [f64::from(r[0]) / scale, f64::from(r[1]) / scale, f64::from(r[2]) / scale, f64::from(r[3]) / scale])
        .collect();
    let Some(word) = rects.iter().copied().reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]) else {
        return;
    };
    let (ox, oy) = cap.origin;
    popup::show_highlight((ox, oy, cap.size.0, cap.size.1), rects, cx);
    let picture = Picture { image: sc.image.clone(), bbox: block.bubble.unwrap_or(block.bbox) };
    popup::show_hover([word[0] + ox, word[1] + oy, word[2] + ox, word[3] + oy], sentence, sentence_offset, response, Some(picture), cx);
}
