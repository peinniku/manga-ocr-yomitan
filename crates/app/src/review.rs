//! Review page: FSRS queue with reveal / rate (space, 1–4).

use std::sync::Arc;
use std::time::Instant;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use picyomi_core::cards::{Card, Stats};
use picyomi_core::explain::{ExplainRequest, LlmConfig};
use picyomi_core::service::QueueItem;

use crate::glossary;
use crate::panel::{explanation_view, ruby_line};
use crate::ui;
use crate::state::{DEVICE_ID, rt, svc};

actions!(review, [Reveal, Again, Hard, Good, Easy]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("space", Reveal, Some("Review")),
        KeyBinding::new("enter", Reveal, Some("Review")),
        KeyBinding::new("1", Again, Some("Review")),
        KeyBinding::new("2", Hard, Some("Review")),
        KeyBinding::new("3", Good, Some("Review")),
        KeyBinding::new("4", Easy, Some("Review")),
    ]);
}

pub struct ReviewView {
    focus: FocusHandle,
    queue: Vec<QueueItem>,
    stats: Option<Stats>,
    revealed: bool,
    shown_at: Instant,
    loading: bool,
    image: Option<Arc<Image>>,
    explaining: bool,
    /// 重来/困难/良好/简单 instead of 忘了/记得.
    four_buttons: bool,
}

impl ReviewView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus: cx.focus_handle(),
            queue: vec![],
            stats: None,
            revealed: false,
            shown_at: Instant::now(),
            loading: true,
            image: None,
            explaining: false,
            four_buttons: false,
        };
        this.reload(cx);
        this
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Cards waiting now (for the sidebar badge).
    pub fn due_count(&self) -> usize {
        self.queue.len()
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let service = svc(cx);
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let (queue, stats, four) = cx.background_spawn(async move { (service.queue(), service.stats(), service.four_buttons()) }).await;
            this.update(cx, |this, cx| {
                this.loading = false;
                this.four_buttons = four;
                this.queue = queue.unwrap_or_default();
                this.stats = stats.ok();
                this.show_current(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn current(&self) -> Option<&Card> {
        self.queue.first().map(|q| &q.card.card)
    }

    fn show_current(&mut self, cx: &mut Context<Self>) {
        self.revealed = false;
        self.shown_at = Instant::now();
        self.image = None;
        let Some(media) = self.current().and_then(|c| c.media_id.clone()) else { return };
        let service = svc(cx);
        cx.spawn(async move |this, cx| {
            let bytes = cx.background_spawn(async move { picyomi_core::cards::load_media(&service.db, &media) }).await;
            if let Ok(Some((bytes, _))) = bytes {
                let img = Arc::new(Image::from_bytes(ImageFormat::Jpeg, bytes));
                this.update(cx, |this, cx| {
                    this.image = Some(img);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn reveal(&mut self, _: &Reveal, _: &mut Window, cx: &mut Context<Self>) {
        if self.current().is_none() {
            return;
        }
        // Space / Enter on the answer side means "remembered".
        if self.revealed {
            self.rate(3, cx);
            return;
        }
        self.revealed = true;
        self.ensure_explanation(cx);
        cx.notify();
    }

    /// Cards saved before an LLM was configured get their Chinese explanation on first reveal.
    fn ensure_explanation(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.current().cloned() else { return };
        if card.explanation.is_some() || self.explaining {
            return;
        }
        let service = svc(cx);
        if !LlmConfig::load(&service.db).map(|c| c.is_configured()).unwrap_or(false) {
            return;
        }
        self.explaining = true;
        let req = ExplainRequest {
            expression: card.expression.clone(),
            reading: card.reading.clone(),
            sentence: card.sentence.clone(),
            definition_hint: card.definitions.first().map(|d| glossary::summary(&d.glossary, 200)).unwrap_or_default(),
        };
        let svc2 = service.clone();
        let handle = rt(cx).spawn(async move { picyomi_core::explain::explain(&svc2.db, &req).await });
        cx.spawn(async move |this, cx| {
            let res = handle.await;
            this.update(cx, |this, cx| {
                this.explaining = false;
                if let Ok(Ok(e)) = res {
                    let mut updated = card.clone();
                    updated.explanation = Some(e);
                    if let Ok(saved) = service.update_card(updated) {
                        if let Some(q) = this.queue.iter_mut().find(|q| q.card.card.id == saved.id) {
                            q.card.card = saved;
                        }
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn rate(&mut self, rating: u8, cx: &mut Context<Self>) {
        if !self.revealed {
            return;
        }
        let Some(item) = (!self.queue.is_empty()).then(|| self.queue.remove(0)) else { return };
        let ms = self.shown_at.elapsed().as_millis() as i64;
        let service = svc(cx);
        let id = item.card.card.id.clone();
        cx.spawn(async move |this, cx| {
            let _ = cx.background_spawn(async move { service.review(&id, rating, DEVICE_ID, Some(ms)) }).await;
            // Refresh: "again" cards come back after a minute, so rebuild the queue from the store.
            this.update(cx, |this, cx| this.reload(cx)).ok();
        })
        .detach();
        self.show_current(cx);
        cx.notify();
    }

    fn render_front(&self, card: &Card, cx: &App) -> Div {
        let theme = cx.theme();
        v_flex()
            .w_full()
            .min_h_full()
            .items_center()
            .justify_center()
            .gap_5()
            .py_8()
            .child(div().text_size(px(44.)).font_weight(FontWeight::BOLD).child(card.expression.clone()))
            .when(!card.sentence.is_empty(), |d| d.child(sentence_with_word(&card.sentence, &card.expression, cx)))
            .when_some(self.image.clone(), |d, img| d.child(img_view(img)))
            .child(div().text_xs().text_color(theme.muted_foreground).child("想一想读音和意思，再看答案"))
    }

    fn render_back(&self, card: &Card, cx: &App) -> Div {
        let theme = cx.theme();
        let hl = word_span(&card.sentence, &card.expression)
            .map(|r| (card.sentence[..r.start].chars().count(), card.sentence[..r.end].chars().count()));
        let mut back = v_flex()
            .w_full()
            .gap_5()
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .child(div().text_size(px(36.)).font_weight(FontWeight::BOLD).child(card.expression.clone()))
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(div().text_xl().text_color(theme.muted_foreground).child(card.reading.clone()))
                            .when_some(card.frequency.clone(), |d, f| d.child(div().text_xs().text_color(theme.muted_foreground).child(format!("频率 {f}")))),
                    ),
            )
            .when(!card.sentence_furigana.is_empty(), |d| d.child(h_flex().w_full().justify_center().child(ruby_line(&card.sentence_furigana, hl, cx))))
            .when_some(self.image.clone(), |d, img| d.child(h_flex().w_full().justify_center().child(img_view(img))));
        for d in &card.definitions {
            back = back.child(
                v_flex()
                    .gap_2()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(d.dict.clone()))
                    .child(glossary::view(&d.glossary, None, cx)),
            );
        }
        if let Some(e) = &card.explanation {
            back = back.child(
                v_flex()
                    .gap_1()
                    .p_4()
                    .rounded_lg()
                    .bg(theme.secondary)
                    .child(div().text_xs().text_color(theme.muted_foreground).child("中文解释"))
                    .child(explanation_view(e, cx)),
            );
        } else if self.explaining {
            back = back.child(div().text_sm().text_color(theme.muted_foreground).child("中文解释生成中…"));
        }
        back
    }
}

/// Byte range of `word` in `sentence`, also when it appears inflected
/// (催す in 催していた): the kanji stem plus the kana right after it.
pub fn word_span(sentence: &str, word: &str) -> Option<std::ops::Range<usize>> {
    if word.is_empty() {
        return None;
    }
    if let Some(a) = sentence.find(word) {
        return Some(a..a + word.len());
    }
    let is_kana = |c: char| ('\u{3041}'..='\u{309f}').contains(&c);
    let stem: String = {
        let mut chars: Vec<char> = word.chars().collect();
        while chars.len() > 1 && chars.last().is_some_and(|c| is_kana(*c)) {
            chars.pop();
        }
        chars.into_iter().collect()
    };
    if stem == word || stem.is_empty() {
        return None;
    }
    let a = sentence.find(&stem)?;
    let mut end = a + stem.len();
    let okurigana = word.chars().count() - stem.chars().count();
    for c in sentence[end..].chars().take(okurigana.max(1)) {
        if !is_kana(c) {
            break;
        }
        end += c.len_utf8();
    }
    Some(a..end)
}

/// The context sentence with the card's word in bold.
fn sentence_with_word(sentence: &str, word: &str, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let mut text = StyledText::new(sentence.to_string());
    if let Some(r) = word_span(sentence, word) {
        text = text.with_highlights([(
            r,
            HighlightStyle { color: Some(theme.foreground), font_weight: Some(FontWeight::BOLD), ..Default::default() },
        )]);
    }
    div().max_w(px(640.)).text_lg().text_center().text_color(theme.muted_foreground).line_height(relative(1.6)).child(text)
}

fn img_view(image: Arc<Image>) -> impl IntoElement {
    img(image).max_h(px(240.)).max_w_full().rounded_md().object_fit(ObjectFit::Contain)
}

impl Render for ReviewView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let stats = self.stats.clone().unwrap_or_default();
        let stat = |label: &str, value: String| {
            h_flex()
                .gap_1()
                .items_baseline()
                .child(div().text_xs().text_color(theme.muted_foreground).child(label.to_string()))
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(value))
        };
        let header = ui::header(
            "复习",
            h_flex()
                .gap_4()
                .overflow_hidden()
                .child(stat("待复习", self.queue.len().to_string()))
                .child(stat("今日", stats.reviewed_today.to_string()))
                .child(stat("词单", stats.total.to_string()))
                .child(stat("连续", format!("{} 天", stats.streak_days)))
                .child(Button::new("reload").ghost().small().icon(IconName::Redo).tooltip("刷新").on_click(cx.listener(|this, _, _, cx| this.reload(cx)))),
            cx,
        );

        let body: AnyElement = match self.queue.first() {
            None if self.loading => ui::empty_state("载入中…", "", cx).into_any_element(),
            None if stats.total == 0 => ui::empty_state(
                "词单还是空的",
                "在任意 App 里按住 Shift 指向日语，或按 ⌥⌘J，在弹窗里点「加入词单」。浏览器里用 pic_yomi 扩展。",
                cx,
            )
            .into_any_element(),
            None => ui::empty_state("今天的复习完成了 🎉", "新的复习到期后会出现在这里。", cx).into_any_element(),
            Some(item) => {
                let card = &item.card.card;
                let content = if self.revealed { self.render_back(card, cx) } else { self.render_front(card, cx) };
                let bar: AnyElement = if self.revealed && !self.four_buttons {
                    // Default: just "forgot" / "remembered" — the only judgement FSRS really needs.
                    let labels = item.preview_labels.clone();
                    let mk = |id: &'static str, key: &'static str, name: &'static str, note: &'static str, interval: String, rating: u8| {
                        v_flex()
                            .flex_1()
                            .min_w(px(130.))
                            .max_w(px(220.))
                            .gap_1()
                            .items_center()
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{interval}后再见")))
                            .child(
                                Button::new(id)
                                    .w_full()
                                    .label(name)
                                    .when(rating == 3, |b| b.primary())
                                    .when(rating == 1, |b| b.danger())
                                    .tooltip(format!("{note}（快捷键 {key}）"))
                                    .on_click(cx.listener(move |this, _, _, cx| this.rate(rating, cx))),
                            )
                    };
                    h_flex()
                        .w_full()
                        .justify_center()
                        .gap_3()
                        .child(mk("forgot", "1", "忘了", "读音或意思没想起来，或想错了", labels[0].clone(), 1))
                        .child(mk("remembered", "空格", "记得", "读音和意思都想对了，慢一点也算", labels[2].clone(), 3))
                        .into_any_element()
                } else if self.revealed {
                    let labels = item.preview_labels.clone();
                    let mk = |id: &'static str, key: &'static str, name: &'static str, interval: String, rating: u8| {
                        v_flex()
                            .flex_1()
                            .min_w(px(88.))
                            .max_w(px(150.))
                            .gap_1()
                            .items_center()
                            .child(div().text_xs().text_color(theme.muted_foreground).child(interval))
                            .child(
                                Button::new(id)
                                    .w_full()
                                    .label(name)
                                    .when(rating == 3, |b| b.primary())
                                    .when(rating == 1, |b| b.danger())
                                    .tooltip(format!("快捷键 {key}"))
                                    .on_click(cx.listener(move |this, _, _, cx| this.rate(rating, cx))),
                            )
                    };
                    h_flex()
                        .w_full()
                        .flex_wrap()
                        .justify_center()
                        .gap_2()
                        .child(mk("again", "1", "重来", labels[0].clone(), 1))
                        .child(mk("hard", "2", "困难", labels[1].clone(), 2))
                        .child(mk("good", "3", "良好", labels[2].clone(), 3))
                        .child(mk("easy", "4", "简单", labels[3].clone(), 4))
                        .into_any_element()
                } else {
                    h_flex()
                        .justify_center()
                        .gap_2()
                        .child(Button::new("reveal").primary().w(px(200.)).label("显示答案").on_click(cx.listener(|this, _, w, cx| this.reveal(&Reveal, w, cx))))
                        .into_any_element()
                };
                let hint = match (self.revealed, self.four_buttons) {
                    (false, _) => "空格 显示答案",
                    (true, false) => "1 忘了 · 空格 记得 —— 读音和意思都想对了才算记得",
                    (true, true) => "1 重来 · 2 困难 · 3 良好 · 4 简单 · 空格 良好",
                };
                v_flex()
                    .size_full()
                    .min_h_0()
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(ui::scroll(SharedString::from(format!("card-{}-{}", card.id, self.revealed)), ui::column(ui::READ_WIDTH).min_h_full().child(content))),
                    )
                    .child(
                        v_flex()
                            .flex_none()
                            .gap_2()
                            .px_5()
                            .pt_3()
                            .pb_4()
                            .border_t_1()
                            .border_color(theme.border)
                            .child(bar)
                            .child(div().text_center().text_xs().text_color(theme.muted_foreground).child(hint)),
                    )
                    .into_any_element()
            }
        };

        v_flex()
            .id("review")
            .key_context("Review")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::reveal))
            .on_action(cx.listener(|this, _: &Again, _, cx| this.rate(1, cx)))
            .on_action(cx.listener(|this, _: &Hard, _, cx| this.rate(if this.four_buttons { 2 } else { 3 }, cx)))
            .on_action(cx.listener(|this, _: &Good, _, cx| this.rate(3, cx)))
            .on_action(cx.listener(|this, _: &Easy, _, cx| this.rate(if this.four_buttons { 4 } else { 3 }, cx)))
            .size_full()
            .child(ui::page(header, body))
    }
}

#[cfg(test)]
mod tests {
    use super::word_span;

    #[test]
    fn finds_inflected_words() {
        let s = "尿意を催していたため";
        assert_eq!(word_span(s, "催す").map(|r| &s[r]), Some("催し"));
        assert_eq!(word_span(s, "尿意").map(|r| &s[r]), Some("尿意"));
        assert_eq!(word_span(s, "ため息"), None);
    }
}
