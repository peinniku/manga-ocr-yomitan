//! Floating lookup popup and the on-screen word highlight, shared by the ⌥⌘J
//! hotkey and hold-Shift scanning (see `scan.rs`).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use picyomi_core::service::LookupResponse;

use crate::native;
use crate::panel::{LookupPanel, Picture};
use crate::screen;
use crate::state::svc;

actions!(popup, [ClosePopup]);

const W: f32 = 440.;
const H: f32 = 480.;
/// Gap between the looked-up word and the popup.
const GAP: f64 = 8.;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", ClosePopup, Some("Popup"))]);
}

/// The open popup and highlight windows, so a new lookup reuses or replaces them.
#[derive(Default)]
struct Windows {
    popup: Option<(AnyWindowHandle, Entity<PopupView>)>,
    /// Popup frame in global points: x, y, w, h.
    popup_rect: Option<[f64; 4]>,
    overlay: Option<(AnyWindowHandle, Entity<Highlight>)>,
}
impl Global for Windows {}

pub struct PopupView {
    focus: FocusHandle,
    panel: Entity<LookupPanel>,
    status: Option<String>,
}

impl PopupView {
    fn close(&mut self, _: &ClosePopup, _: &mut Window, cx: &mut Context<Self>) {
        cx.defer(|cx| {
            close_all(cx);
        });
    }
}

impl Render for PopupView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .id("popup")
            .key_context("Popup")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::close))
            .size_full()
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .px_4()
            .pt_2()
            .pb_3()
            .gap_2()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_flex()
                    .flex_none()
                    .child(div().text_xs().text_color(theme.muted_foreground).child("pic_yomi · 点别处关闭"))
                    .child(div().flex_1())
                    .child(Button::new("close").ghost().xsmall().icon(IconName::Close).on_click(cx.listener(|this, _, w, cx| this.close(&ClosePopup, w, cx)))),
            )
            .when_some(self.status.clone(), |d, s| d.child(div().flex_none().text_sm().text_color(theme.muted_foreground).child(s)))
            .child(div().flex_1().min_h_0().child(self.panel.clone()))
    }
}

/// Translucent marks over the looked-up word on screen, like Yomitan's highlight.
pub struct Highlight {
    rects: Vec<Bounds<Pixels>>,
}

impl Render for Highlight {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let fill = hsla(45. / 360., 1., 0.5, 0.28);
        let line = hsla(32. / 360., 1., 0.5, 0.9);
        div().size_full().relative().children(self.rects.iter().map(|r| {
            div()
                .absolute()
                .left(r.origin.x)
                .top(r.origin.y)
                .w(r.size.width)
                .h(r.size.height)
                .rounded_sm()
                .bg(fill)
                .border_b_2()
                .border_color(line)
        }))
    }
}

/// Close the popup and the highlight. Returns true if anything was open.
pub fn close_all(cx: &mut App) -> bool {
    let w = cx.default_global::<Windows>();
    let popup = w.popup.take();
    let overlay = w.overlay.take();
    w.popup_rect = None;
    let any = popup.is_some() || overlay.is_some();
    for handle in popup.map(|p| p.0).into_iter().chain(overlay.map(|o| o.0)) {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
    any
}

/// Whether the global point is inside the open popup.
pub fn popup_contains(x: f64, y: f64, cx: &mut App) -> bool {
    cx.default_global::<Windows>()
        .popup_rect
        .is_some_and(|[px, py, pw, ph]| x >= px && y >= py && x < px + pw && y < py + ph)
}

fn display_at(x: f64, y: f64, cx: &App) -> Option<Bounds<Pixels>> {
    let p = point(px(x as f32), px(y as f32));
    cx.displays().into_iter().find(|d| d.bounds().contains(&p)).or_else(|| cx.primary_display()).map(|d| d.bounds())
}

/// Top-left (global points) for the popup next to `word` (x0, y0, x1, y1):
/// below it, or above when there is no room, kept on the display.
fn place(word: [f64; 4], cx: &App) -> (f64, f64) {
    let (w, h) = (W as f64, H as f64);
    let Some(b) = display_at(word[0], word[1], cx) else { return (word[0], word[3] + GAP) };
    let (dx, dy) = (f64::from(b.origin.x), f64::from(b.origin.y));
    let (dw, dh) = (f64::from(b.size.width), f64::from(b.size.height));
    let x = word[0].clamp(dx + 4., (dx + dw - w - 4.).max(dx));
    let y = if word[3] + GAP + h <= dy + dh {
        word[3] + GAP
    } else if word[1] - GAP - h >= dy {
        word[1] - GAP - h
    } else {
        (dy + dh - h - 4.).max(dy)
    };
    (x, y)
}

/// Open a popup at a global top-left. `activate` focuses it (⌥⌘J);
/// hover scanning leaves focus with the app being read.
fn open_popup(origin: (f64, f64), activate: bool, status: Option<String>, cx: &mut App) -> Option<Entity<PopupView>> {
    if let Some((old, _)) = cx.default_global::<Windows>().popup.take() {
        let _ = old.update(cx, |_, window, _| window.remove_window());
    }
    let p = point(px(origin.0 as f32), px(origin.1 as f32));
    let display = cx.displays().into_iter().find(|d| d.bounds().contains(&p)).or_else(|| cx.primary_display());
    let (local, display_id) = match &display {
        Some(d) => (point(p.x - d.bounds().origin.x, p.y - d.bounds().origin.y), Some(d.id())),
        None => (p, None),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: local, size: size(px(W), px(H)) })),
        titlebar: None,
        focus: activate,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: true,
        display_id,
        // Transparent so the view's rounded corners show.
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let mut view = None;
    let handle = gpui_kit::open_window(options, cx, |window, cx| {
        let v = cx.new(|cx| PopupView { focus: cx.focus_handle(), panel: cx.new(|_| LookupPanel::new()), status });
        if activate {
            let focus = v.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
        view = Some(v.clone());
        v
    })
    .ok()?
    .0;
    let view = view?;
    let w = cx.default_global::<Windows>();
    w.popup = Some((handle, view.clone()));
    w.popup_rect = Some([origin.0, origin.1, W as f64, H as f64]);
    if activate {
        cx.activate(true);
    }
    Some(view)
}

/// Mark `rects` (points, relative to `region`'s top-left) on screen. The
/// window covers the captured region and lets clicks through.
pub fn show_highlight(region: (f64, f64, f64, f64), rects: Vec<[f64; 4]>, cx: &mut App) {
    let rects: Vec<Bounds<Pixels>> = rects
        .into_iter()
        .map(|r| {
            let pad = 1.5;
            Bounds {
                origin: point(px((r[0] - pad) as f32), px((r[1] - pad) as f32)),
                size: size(px((r[2] - r[0] + 2. * pad) as f32), px((r[3] - r[1] + 2. * pad) as f32)),
            }
        })
        .collect();
    if let Some((_, view)) = cx.default_global::<Windows>().overlay.clone() {
        view.update(cx, |v, cx| {
            v.rects = rects;
            cx.notify();
        });
        return;
    }
    let p = point(px(region.0 as f32), px(region.1 as f32));
    let display = cx.displays().into_iter().find(|d| d.bounds().contains(&p)).or_else(|| cx.primary_display());
    let (local, display_id) = match &display {
        Some(d) => (point(p.x - d.bounds().origin.x, p.y - d.bounds().origin.y), Some(d.id())),
        None => (p, None),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin: local, size: size(px(region.2 as f32), px(region.3 as f32)) })),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        display_id,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let mut view = None;
    if let Ok(handle) = gpui_kit::open_window(options, cx, |window, cx| {
        native::set_click_through(window);
        let v = cx.new(|_| Highlight { rects });
        view = Some(v.clone());
        v
    }) {
        if let Some(view) = view {
            cx.default_global::<Windows>().overlay = Some((handle.0, view));
        }
    }
}

/// Hover scanning: show this lookup next to `word` (global points), reusing
/// and moving the popup if one is already up.
pub fn show_hover(word: [f64; 4], sentence: String, offset: usize, response: LookupResponse, picture: Option<Picture>, cx: &mut App) {
    let origin = place(word, cx);
    let existing = cx.default_global::<Windows>().popup.clone();
    let view = match existing {
        Some((handle, view)) if handle.update(cx, |_, window, _| native::move_window(window, origin.0, origin.1)).is_ok() => {
            cx.default_global::<Windows>().popup_rect = Some([origin.0, origin.1, W as f64, H as f64]);
            view
        }
        _ => match open_popup(origin, false, None, cx) {
            Some(v) => v,
            None => return,
        },
    };
    view.update(cx, |v, cx| {
        v.status = None;
        v.panel.update(cx, |p, cx| p.show(sentence, offset, response, picture, cx));
        cx.notify();
    });
}

/// Hover scanning: show a message (e.g. a missing permission) near the cursor.
pub fn show_message(x: f64, y: f64, message: String, cx: &mut App) {
    let origin = place([x, y, x, y + 12.], cx);
    open_popup(origin, false, Some(message), cx);
}

/// ⌥⌘J: capture around the cursor, recognize, and open a focused popup.
pub fn screen_lookup(cx: &mut App) {
    let Some((x, y)) = screen::cursor_position() else { return };
    // Close what is open first so it is not in the screenshot.
    let had = close_all(cx);
    cx.spawn(async move |cx| {
        if had {
            cx.background_executor().timer(Duration::from_millis(150)).await;
        }
        let _ = cx.update(|cx| show_lookup(x, y, cx));
    })
    .detach();
}

fn show_lookup(x: f64, y: f64, cx: &mut App) {
    let capture = screen::capture_around(x, y);
    let Some(view) = open_popup(place([x, y, x, y + 12.], cx), true, Some("识别中…".into()), cx) else { return };
    let capture = match capture {
        Ok(c) => c,
        Err(e) => {
            view.update(cx, |v, cx| {
                v.status = Some(format!("{e:#}"));
                cx.notify();
            });
            return;
        }
    };
    let service = svc(cx);
    let view = view.downgrade();
    cx.spawn(async move |cx| {
        let image = Arc::new(capture.image);
        let img2 = image.clone();
        let (cursor, scale) = (capture.cursor_px, capture.scale);
        let res = cx.background_spawn(async move { screen::read_at(&service, img2, cursor, scale) }).await;
        view.update(cx, |v, cx| {
            match res {
                Ok(Some((block, offset))) => {
                    v.status = None;
                    let picture = Picture { image, bbox: block.bubble.unwrap_or(block.bbox) };
                    v.panel.update(cx, |p, cx| p.query(block.text.clone(), offset, Some(picture), None, cx));
                }
                Ok(None) => v.status = Some("鼠标附近没有识别到日语文字。把鼠标放在文字上再按 ⌥⌘J。".into()),
                Err(e) => v.status = Some(format!("{e:#}")),
            }
            cx.notify();
        })
        .ok();
    })
    .detach();
}
