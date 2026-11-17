//! In-app lookup: paste text or open an image, then click a character to look it up.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use image::DynamicImage;
use picyomi_core::ocr::OcrPage;

use crate::panel::{LookupPanel, Picture};
use crate::state::svc;
use crate::ui;

pub struct LookupView {
    input: Entity<InputState>,
    panel: Entity<LookupPanel>,
    text: String,
    image: Option<(PathBuf, Arc<DynamicImage>, OcrPage)>,
    /// Picture for the current text (block of the opened image).
    picture: Option<Picture>,
    status: Option<String>,
    /// Character of `text` the current lookup starts at (highlighted with the match).
    picked: Option<usize>,
    _subs: Vec<Subscription>,
}

impl LookupView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("粘贴日语句子，回车后点击任意字查词"));
        let sub = cx.subscribe_in(&input, window, |this, state, ev, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.text = state.read(cx).value().trim().to_string();
                this.picture = None;
                this.picked = (!this.text.is_empty()).then_some(0);
                if !this.text.is_empty() {
                    let text = this.text.clone();
                    this.panel.update(cx, |p, cx| p.query(text, 0, None, None, cx));
                }
                cx.notify();
            }
        });
        let panel = cx.new(|_| LookupPanel::new().without_sentence());
        // Re-render when the lookup finishes, to highlight the matched characters.
        let watch = cx.observe(&panel, |_, _, cx| cx.notify());
        Self {
            input,
            panel,
            text: String::new(),
            image: None,
            picture: None,
            status: None,
            picked: None,
            _subs: vec![sub, watch],
        }
    }

    fn open_image(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("识别图片".into()) });
        let service = svc(cx);
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            this.update(cx, |this, cx| {
                this.status = Some("识别中…".into());
                cx.notify();
            })
            .ok();
            let p2 = path.clone();
            let res = cx
                .background_spawn(async move {
                    let bytes = std::fs::read(&p2)?;
                    let img = Arc::new(picyomi_core::ocr::decode(&bytes)?);
                    // Manga text from the manga models, plus horizontal text Vision found elsewhere
                    // (screenshots, web captures).
                    let pages = crate::screen::analyze(&service, img.clone());
                    let mut blocks = pages.manga.as_ref().map(|p| p.blocks.clone()).unwrap_or_default();
                    let inside = |b: &picyomi_core::ocr::OcrBlock, o: &picyomi_core::ocr::OcrBlock| {
                        let (cx_, cy) = ((b.bbox[0] + b.bbox[2]) / 2., (b.bbox[1] + b.bbox[3]) / 2.);
                        cx_ >= o.bbox[0] && cx_ <= o.bbox[2] && cy >= o.bbox[1] && cy <= o.bbox[3]
                    };
                    for v in pages.vision.map(|p| p.blocks).unwrap_or_default() {
                        if !blocks.iter().any(|m| inside(&v, m) || inside(m, &v)) {
                            blocks.push(v);
                        }
                    }
                    blocks.sort_by(|a, b| a.bbox[1].total_cmp(&b.bbox[1]));
                    let page = OcrPage { width: img.width(), height: img.height(), blocks };
                    anyhow::Ok((img, page))
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok((img, page)) => {
                        this.status = Some(if page.blocks.is_empty() { "图里没有识别到文字".into() } else { format!("识别到 {} 段文字", page.blocks.len()) });
                        this.image = Some((path, img, page));
                    }
                    Err(e) => this.status = Some(format!("识别失败：{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn pick_block(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some((path, img, page)) = &self.image else { return };
        let Some(b) = page.blocks.get(i) else { return };
        self.text = b.text.clone();
        self.picked = Some(0);
        self.picture = Some(Picture { image: img.clone(), bbox: b.bubble.unwrap_or(b.bbox) });
        let (text, picture, src) = (self.text.clone(), self.picture.clone(), Some(format!("file://{}", path.display())));
        self.panel.update(cx, |p, cx| p.query(text, 0, picture, src, cx));
        cx.notify();
    }

    fn pick_char(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.picked = Some(offset);
        let (text, picture) = (self.text.clone(), self.picture.clone());
        let src = self.image.as_ref().map(|(p, _, _)| format!("file://{}", p.display()));
        self.panel.update(cx, |p, cx| p.query(text, offset, picture, src, cx));
    }
}

impl Render for LookupView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let narrow = window.viewport_size().width < px(900.);

        let header = ui::header(
            "查词",
            h_flex()
                .flex_1()
                .gap_2()
                .child(div().flex_1().min_w(px(120.)).child(Input::new(&self.input).small().cleanable(true)))
                .child(Button::new("open-image").small().icon(IconName::FolderOpen).label("识别图片…").on_click(cx.listener(|this, _, _, cx| this.open_image(cx)))),
            cx,
        );

        // The sentence, one clickable character at a time.
        let len = self.panel.read(cx).match_len();
        let picked = self.picked.map(|p| p..p + len.max(1));
        let chars_row = (!self.text.is_empty()).then(|| {
            let mut row = h_flex().flex_wrap().gap_y_1();
            for (i, ch) in self.text.chars().enumerate() {
                let on = picked.as_ref().is_some_and(|r| r.contains(&i));
                row = row.child(
                    div()
                        .id(("ch", i))
                        .px(px(1.))
                        .text_xl()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(on, |d| d.bg(theme.primary.opacity(0.18)).text_color(theme.primary))
                        .hover(|s| s.bg(theme.secondary))
                        .on_click(cx.listener(move |this, _, _, cx| this.pick_char(i, cx)))
                        .child(ch.to_string()),
                );
            }
            v_flex()
                .flex_none()
                .gap_1()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .child(div().text_xs().text_color(theme.muted_foreground).child("点击任意字，从这个字开始查词"))
                .child(row)
        });

        let blocks = self.image.as_ref().map(|(_, _, page)| {
            let list = v_flex().gap_1().children(page.blocks.iter().enumerate().map(|(i, b)| {
                div()
                    .id(("block", i))
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.secondary))
                    .when(b.text == self.text, |d| d.bg(theme.secondary).text_color(theme.primary))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_block(i, cx)))
                    .child(b.text.clone())
            }));
            let frame = v_flex()
                .gap_2()
                .p_2()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .when_some(self.status.clone(), |d, s| d.child(div().px_1().text_xs().text_color(theme.muted_foreground).child(s)))
                .child(div().flex_1().min_h_0().child(ui::scroll("blocks", list)));
            if narrow { frame.flex_none().h(px(170.)) } else { frame.flex_none().w(px(260.)).h_full() }
        });

        let body: AnyElement = if self.text.is_empty() && self.image.is_none() {
            ui::empty_state(
                "查词",
                "在上面粘贴日语句子后回车，再点任意字查词；或者「识别图片…」打开一张漫画或截图，点识别出的文字。",
                cx,
            )
            .when_some(self.status.clone(), |d, s| d.child(div().text_sm().text_color(theme.muted_foreground).child(s)))
            .into_any_element()
        } else {
            let right = v_flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .gap_3()
                .children(chars_row)
                .when(!self.text.is_empty(), |d| d.child(div().flex_1().min_h_0().child(self.panel.clone())));
            let inner = if narrow { v_flex().children(blocks).child(right) } else { h_flex().items_start().children(blocks).child(right) };
            inner.size_full().min_h_0().gap_4().p_5().into_any_element()
        };
        ui::page(header, body)
    }
}
