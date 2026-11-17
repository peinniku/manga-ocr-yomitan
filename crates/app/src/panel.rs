//! The lookup result panel shared by the screen-lookup popup and the in-app lookup page:
//! sentence with furigana, dictionary entries, Chinese explanation, and "add to word list".

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Disableable, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use image::DynamicImage;
use picyomi_core::cards::Explanation;
use picyomi_core::explain::{ExplainRequest, LlmConfig};
use picyomi_core::service::{AddCardRequest, LookupResponse};
use picyomi_core::tokenize::Ruby;

use crate::glossary;
use crate::state::{rt, svc};

/// Picture to attach to a card: the source image and the region (speech bubble) to crop.
#[derive(Clone)]
pub struct Picture {
    pub image: Arc<DynamicImage>,
    pub bbox: [f32; 4],
}

enum ZhState {
    Off,
    Loading,
    Done(Explanation),
    Failed(String),
}

pub struct LookupPanel {
    sentence: String,
    offset: usize,
    response: Option<LookupResponse>,
    selected: usize,
    zh: ZhState,
    picture: Option<Picture>,
    source: Option<String>,
    status: Option<String>,
    busy: bool,
    /// Bumped for every new word, so late async results are dropped.
    generation: u64,
    /// An LLM is configured, so the 「中文解释」 button is offered.
    llm_ready: bool,
    /// Show the source sentence at the top (off where the page already shows it).
    show_sentence: bool,
}

impl LookupPanel {
    pub fn new() -> Self {
        Self {
            sentence: String::new(),
            offset: 0,
            response: None,
            selected: 0,
            zh: ZhState::Off,
            picture: None,
            source: None,
            status: None,
            busy: false,
            generation: 0,
            llm_ready: false,
            show_sentence: true,
        }
    }

    pub fn without_sentence(mut self) -> Self {
        self.show_sentence = false;
        self
    }

    /// Characters matched by the shown word (0 while looking up or when nothing matched).
    pub fn match_len(&self) -> usize {
        self.response.as_ref().map(|r| r.result.match_length).unwrap_or(0)
    }

    pub fn has_result(&self) -> bool {
        self.response.as_ref().is_some_and(|r| !r.result.entries.is_empty())
    }

    /// Look up the word at `offset` in `sentence`.
    pub fn query(&mut self, sentence: String, offset: usize, picture: Option<Picture>, source: Option<String>, cx: &mut Context<Self>) {
        // Only the sentence around the word: for display, the card and the LLM.
        let (sentence, offset) = picyomi_core::lookup::sentence_at(&sentence, offset);
        self.sentence = sentence.clone();
        self.offset = offset;
        self.picture = picture;
        self.source = source;
        self.status = None;
        self.busy = true;
        self.generation += 1;
        let generation = self.generation;
        cx.notify();
        let service = svc(cx);
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { service.lookup(&sentence, offset) }).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.busy = false;
                match result {
                    Ok(r) => {
                        this.response = Some(r);
                        this.selected = 0;
                        if !this.has_result() {
                            this.status = Some("没有找到这个词".into());
                        }
                        this.reset_explanation(cx);
                    }
                    Err(e) => this.status = Some(format!("查词失败：{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Show a lookup already done by the caller (hover scanning looks up on every move).
    pub fn show(&mut self, sentence: String, offset: usize, response: LookupResponse, picture: Option<Picture>, cx: &mut Context<Self>) {
        self.sentence = sentence;
        self.offset = offset;
        self.picture = picture;
        self.source = None;
        self.status = None;
        self.busy = false;
        self.response = Some(response);
        self.selected = 0;
        self.generation += 1;
        self.reset_explanation(cx);
        cx.notify();
    }

    /// A new word: no Chinese explanation until asked for (or until the word is
    /// added to the word list, which generates one in the background).
    fn reset_explanation(&mut self, cx: &mut Context<Self>) {
        self.zh = ZhState::Off;
        self.llm_ready = LlmConfig::load(&svc(cx).db).map(|c| c.is_configured()).unwrap_or(false);
    }

    fn fetch_explanation(&mut self, cx: &mut Context<Self>) {
        let service = svc(cx);
        let Some(entry) = self.response.as_ref().and_then(|r| r.result.entries.get(self.selected)).cloned() else {
            self.zh = ZhState::Off;
            return;
        };
        if !LlmConfig::load(&service.db).map(|c| c.is_configured()).unwrap_or(false) {
            self.zh = ZhState::Off;
            return;
        }
        let req = ExplainRequest {
            expression: entry.expression.clone(),
            reading: entry.reading.clone(),
            sentence: self.sentence.clone(),
            definition_hint: glossary::summary(&entry.glossary, 200),
        };
        self.zh = ZhState::Loading;
        let handle = rt(cx).spawn(async move { picyomi_core::explain::explain(&service.db, &req).await });
        cx.spawn(async move |this, cx| {
            let res = handle.await;
            this.update(cx, |this, cx| {
                // Ignore late results for an entry that is no longer selected.
                let still = this.response.as_ref().and_then(|r| r.result.entries.get(this.selected)).map(|e| e.id) == Some(entry.id);
                if still {
                    this.zh = match res {
                        Ok(Ok(e)) => ZhState::Done(e),
                        Ok(Err(e)) => ZhState::Failed(format!("{e:#}")),
                        Err(e) => ZhState::Failed(e.to_string()),
                    };
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn select(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.selected != i {
            self.selected = i;
            self.zh = ZhState::Off;
            cx.notify();
        }
    }

    fn add_selected(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &self.response else { return };
        let Some(entry) = r.result.entries.get(self.selected).cloned() else { return };
        let explanation = match &self.zh {
            ZhState::Done(e) => Some(e.clone()),
            _ => None,
        };
        let sentence = self.sentence.clone();
        let picture = self.picture.clone();
        let source = self.source.clone();
        let service = svc(cx);
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let image_b64 = picture.map(|p| encode_crop(&p)).transpose()?;
                    service.add_card(&AddCardRequest {
                        expression: entry.expression.clone(),
                        reading: entry.reading.clone(),
                        sentence,
                        source_url: source,
                        image_b64,
                        explanation,
                        frequency: entry.frequencies.first().and_then(|f| f.display.clone()),
                        ..Default::default()
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match res {
                    Ok(card) => {
                        if let Some(r) = this.response.as_mut() {
                            if let Some(slot) = r.saved.get_mut(this.selected) {
                                *slot = Some(card.id.clone());
                            }
                        }
                        this.status = Some(format!("已加入词单：{}", card.expression));
                        if card.explanation.is_none() && this.llm_ready {
                            this.explain_saved(card.id.clone(), cx);
                        }
                    }
                    Err(e) => this.status = Some(format!("加入失败：{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl LookupPanel {
    /// After "add to word list": generate the card's Chinese explanation and show it.
    fn explain_saved(&mut self, card_id: String, cx: &mut Context<Self>) {
        let service = svc(cx);
        let generation = self.generation;
        self.zh = ZhState::Loading;
        let handle = rt(cx).spawn(async move { service.fill_explanation(&card_id).await });
        cx.spawn(async move |this, cx| {
            let res = handle.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return; // another word is up now; the card is updated anyway
                }
                this.zh = match res {
                    Ok(Ok(Some(card))) => card.explanation.map(ZhState::Done).unwrap_or(ZhState::Off),
                    Ok(Ok(None)) => ZhState::Off,
                    Ok(Err(e)) => ZhState::Failed(format!("{e:#}")),
                    Err(e) => ZhState::Failed(e.to_string()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Crop the picture region to a small JPEG, base64-encoded.
fn encode_crop(p: &Picture) -> anyhow::Result<String> {
    use base64_engine::encode;
    let (w, h) = (p.image.width() as f32, p.image.height() as f32);
    let m = 12.0;
    let x0 = (p.bbox[0] - m).max(0.0) as u32;
    let y0 = (p.bbox[1] - m).max(0.0) as u32;
    let x1 = ((p.bbox[2] + m).min(w) as u32).max(x0 + 1);
    let y1 = ((p.bbox[3] + m).min(h) as u32).max(y0 + 1);
    let mut crop = p.image.crop_imm(x0, y0, x1 - x0, y1 - y0);
    if crop.width() > 720 || crop.height() > 720 {
        crop = crop.resize(720, 720, image::imageops::FilterType::Triangle);
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 82);
    crop.to_rgb8().write_with_encoder(enc)?;
    Ok(encode(&buf.into_inner()))
}

/// Minimal standard base64 (avoids another dependency in the app crate).
mod base64_engine {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    pub fn encode(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            out.push(T[(n >> 18) as usize & 63] as char);
            out.push(T[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
        }
        out
    }
}

/// The source sentence with the looked-up word highlighted.
pub fn sentence_view(sentence: &str, offset: usize, len: usize, cx: &App) -> Div {
    let byte = |n: usize| sentence.char_indices().nth(n).map(|(i, _)| i).unwrap_or(sentence.len());
    let (a, b) = (byte(offset), byte(offset + len));
    let primary = cx.theme().primary;
    let text = StyledText::new(sentence.to_string()).with_highlights((a < b).then(|| {
        (
            a..b,
            HighlightStyle {
                color: Some(primary),
                font_weight: Some(FontWeight::BOLD),
                background_color: Some(primary.opacity(0.12)),
                ..Default::default()
            },
        )
    }));
    div().flex_none().text_base().line_height(relative(1.6)).child(text)
}

/// Furigana sentence; segments overlapping [hl_start, hl_end) are highlighted.
///
/// Readings float above their base text (they don't widen it), so the sentence
/// keeps normal spacing; a long reading may overhang its neighbours a little.
pub fn ruby_line(segments: &[Ruby], hl: Option<(usize, usize)>, cx: &App) -> Div {
    let theme = cx.theme();
    let mut row = h_flex().flex_wrap().items_end().gap_y_1();
    let mut pos = 0usize;
    for (i, seg) in segments.iter().enumerate() {
        let len = seg.t.chars().count();
        // Highlight exactly the characters of [a, b) that fall in this segment.
        let range = hl.and_then(|(a, b)| {
            let (from, to) = (a.max(pos) - pos, b.min(pos + len).saturating_sub(pos));
            (from < to).then(|| {
                let byte = |n: usize| seg.t.char_indices().nth(n).map(|(i, _)| i).unwrap_or(seg.t.len());
                byte(from)..byte(to)
            })
        });
        pos += len;
        let style = HighlightStyle {
            color: Some(theme.primary),
            font_weight: Some(FontWeight::SEMIBOLD),
            background_color: Some(theme.primary.opacity(0.12)),
            ..Default::default()
        };
        let base = div().text_lg().child(StyledText::new(seg.t.clone()).with_highlights(range.map(|r| (r, style))));
        let mut cell = div().id(("ruby", i)).relative().pt(px(13.));
        if let Some(r) = seg.r.clone().filter(|r| !r.is_empty()) {
            cell = cell.child(
                h_flex()
                    .absolute()
                    .top_0()
                    .left(px(-24.))
                    .right(px(-24.))
                    .justify_center()
                    .text_size(px(10.))
                    .line_height(px(13.))
                    .text_color(theme.muted_foreground)
                    .whitespace_nowrap()
                    .child(r),
            );
        }
        row = row.child(cell.child(base));
    }
    row
}

pub fn explanation_view(e: &Explanation, cx: &App) -> Div {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .gap_1()
        .when(!e.meaning_zh.is_empty(), |d| d.child(div().child(e.meaning_zh.clone())))
        .when(!e.gloss_zh.is_empty(), |d| d.child(div().text_sm().text_color(muted).child(format!("义项：{}", e.gloss_zh))))
        .when(!e.sentence_zh.is_empty(), |d| d.child(div().text_sm().child(format!("译：{}", e.sentence_zh))))
        .children(e.examples.iter().map(|ex| {
            v_flex()
                .pt_1()
                .child(div().text_sm().child(ex.ja.clone()))
                .child(div().text_xs().text_color(muted).child(ex.zh.clone()))
        }))
}

impl Render for LookupPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut root = v_flex().size_full().min_h_0().gap_3();

        if let Some(r) = self.response.as_ref().filter(|_| self.show_sentence) {
            root = root.child(sentence_view(&self.sentence, self.offset, r.result.match_length, cx));
        } else if self.busy {
            root = root.child(div().text_color(theme.muted_foreground).child("查词中…"));
        }

        if let Some(r) = &self.response {
            if !r.result.entries.is_empty() {
                // Entry chooser (only when there is a choice).
                let mut tabs = h_flex().flex_none().flex_wrap().gap_1();
                for (i, e) in r.result.entries.iter().enumerate() {
                    let label = format!("{}【{}】", e.expression, e.reading);
                    tabs = tabs.child(
                        Button::new(("entry", e.id as usize))
                            .small()
                            .label(label)
                            .when(i == self.selected, |b| b.primary())
                            .when(i != self.selected, |b| b.ghost())
                            .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx))),
                    );
                }
                if r.result.entries.len() > 1 {
                    root = root.child(tabs);
                }

                let e = &r.result.entries[self.selected.min(r.result.entries.len() - 1)];
                let saved = r.saved.get(self.selected).cloned().flatten().is_some();
                let freq = e.frequencies.iter().filter_map(|f| f.display.clone()).collect::<Vec<_>>().join(" · ");
                let header = h_flex()
                    .flex_none()
                    .gap_3()
                    .items_center()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .flex_wrap()
                            .gap_x_3()
                            .items_baseline()
                            .child(div().text_2xl().font_weight(FontWeight::BOLD).child(e.expression.clone()))
                            .child(div().text_color(theme.muted_foreground).child(e.reading.clone()))
                            .when(!freq.is_empty(), |d| d.child(div().text_xs().text_color(theme.muted_foreground).child(format!("频率 {freq}"))))
                            .when(!e.reasons.is_empty(), |d| d.child(div().text_xs().text_color(theme.muted_foreground).child(e.reasons.join(" ← ")))),
                    )
                    .child(
                        Button::new("add-card")
                            .flex_none()
                            .small()
                            .icon(if saved { IconName::Check } else { IconName::Plus })
                            .label(if saved { "已在词单" } else { "加入词单" })
                            .when(!saved, |b| b.primary())
                            .disabled(saved || self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.add_selected(cx))),
                    );

                let defs = glossary::view(&e.glossary, None, cx);
                // Chinese explanation: on request (button), or after adding to the word list.
                let zh_body: Option<AnyElement> = match &self.zh {
                    ZhState::Off => None,
                    ZhState::Loading => Some(div().text_sm().text_color(theme.muted_foreground).child("中文解释生成中…").into_any_element()),
                    ZhState::Done(x) => Some(explanation_view(x, cx).into_any_element()),
                    ZhState::Failed(err) => Some(div().text_xs().text_color(theme.danger).child(format!("中文解释失败：{err}")).into_any_element()),
                };
                let zh: Option<AnyElement> = match zh_body {
                    Some(body) => Some(
                        v_flex()
                            .gap_1()
                            .p_3()
                            .rounded_md()
                            .bg(theme.secondary)
                            .child(div().text_xs().text_color(theme.muted_foreground).child("中文解释"))
                            .child(body)
                            .into_any_element(),
                    ),
                    None if self.llm_ready => Some(
                        h_flex()
                            .child(
                                Button::new("ask-zh")
                                    .small()
                                    .outline()
                                    .label("中文解释")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.fetch_explanation(cx);
                                        cx.notify();
                                    })),
                            )
                            .into_any_element(),
                    ),
                    None => None,
                };
                root = root.child(header).child(
                    div().flex_1().min_h_0().child(crate::ui::scroll(
                        ("entry-body", e.id as usize),
                        v_flex()
                            .gap_4()
                            .pr_3()
                            .pb_2()
                            .child(v_flex().gap_1().child(div().text_xs().text_color(theme.muted_foreground).child(e.dict.clone())).child(defs))
                            .children(zh),
                    )),
                );
            }
        }
        if let Some(s) = &self.status {
            root = root.child(div().flex_none().text_sm().text_color(theme.muted_foreground).child(s.clone()));
        }
        root
    }
}
