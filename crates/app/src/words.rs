//! Word list: search, inspect, delete.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use picyomi_core::cards::CardWithSchedule;
use picyomi_core::srs::{Phase, format_interval};

use crate::glossary;
use crate::panel::{explanation_view, ruby_line};
use crate::state::svc;
use crate::ui;

pub struct WordsView {
    search: Entity<InputState>,
    cards: Vec<CardWithSchedule>,
    expanded: Option<String>,
    /// Card whose 「删除」 was clicked once; a second click deletes.
    confirm_delete: Option<String>,
    loaded: bool,
    _subs: Vec<Subscription>,
}

impl WordsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索单词、读音或句子"));
        let sub = cx.subscribe_in(&search, window, |_, _, ev, _, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        });
        let mut this = Self { search, cards: vec![], expanded: None, confirm_delete: None, loaded: false, _subs: vec![sub] };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let service = svc(cx);
        cx.spawn(async move |this, cx| {
            let cards = cx.background_spawn(async move { service.cards() }).await;
            this.update(cx, |this, cx| {
                this.cards = cards.unwrap_or_default();
                // Newest first.
                this.cards.sort_by(|a, b| b.card.created_at.cmp(&a.card.created_at));
                this.loaded = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        let service = svc(cx);
        let _ = service.delete_card(&id);
        self.cards.retain(|c| c.card.id != id);
        cx.notify();
    }
}

fn due_label(c: &CardWithSchedule, now: i64) -> String {
    match c.schedule.phase {
        Phase::New => "新词".into(),
        _ if c.schedule.due <= now => "待复习".into(),
        _ => format!("{}后", format_interval(c.schedule.due - now)),
    }
}

impl WordsView {
    fn detail(&self, c: &CardWithSchedule, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let id = c.card.id.clone();
        let confirming = self.confirm_delete.as_deref() == Some(id.as_str());
        let mut detail = v_flex().gap_4().mx_3().mb_2().p_4().rounded_lg().border_1().border_color(theme.border).bg(theme.background);
        if !c.card.sentence_furigana.is_empty() {
            let hl = crate::review::word_span(&c.card.sentence, &c.card.expression)
                .map(|r| (c.card.sentence[..r.start].chars().count(), c.card.sentence[..r.end].chars().count()));
            detail = detail.child(ruby_line(&c.card.sentence_furigana, hl, cx));
        }
        for d in &c.card.definitions {
            detail = detail.child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(d.dict.clone()))
                    .child(glossary::view(&d.glossary, Some(14), cx)),
            );
        }
        if let Some(e) = &c.card.explanation {
            detail = detail.child(v_flex().p_3().rounded_md().bg(theme.secondary).child(explanation_view(e, cx)));
        }
        let source = c.card.source_title.clone().or_else(|| c.card.source_url.clone());
        detail.child(
            h_flex()
                .gap_3()
                .items_center()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!("复习 {} 次 · 遗忘 {} 次", c.schedule.reps, c.schedule.lapses))
                .when_some(source, |d, s| d.child(div().min_w_0().truncate().child(s)))
                .child(div().flex_1())
                .when(confirming, |d| {
                    d.child(Button::new(SharedString::from(format!("cancel-{id}"))).xsmall().ghost().label("取消").on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_delete = None;
                        cx.notify();
                    })))
                })
                .child({
                    let del_id = id.clone();
                    Button::new(SharedString::from(format!("del-{id}")))
                        .xsmall()
                        .when(confirming, |b| b.danger())
                        .when(!confirming, |b| b.ghost())
                        .icon(IconName::Delete)
                        .label(if confirming { "确认删除" } else { "删除" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.confirm_delete.as_deref() == Some(del_id.as_str()) {
                                this.confirm_delete = None;
                                this.delete(del_id.clone(), cx);
                            } else {
                                this.confirm_delete = Some(del_id.clone());
                                cx.notify();
                            }
                        }))
                }),
        )
    }
}

impl Render for WordsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let q = self.search.read(cx).value().trim().to_string();
        let now = picyomi_core::now_ms();
        let shown: Vec<CardWithSchedule> = self
            .cards
            .iter()
            .filter(|c| {
                q.is_empty()
                    || c.card.expression.contains(&q)
                    || c.card.reading.contains(&q)
                    || c.card.sentence.contains(&q)
                    || c.card.explanation.as_ref().is_some_and(|e| e.meaning_zh.contains(&q))
            })
            .cloned()
            .collect();

        let header = ui::header(
            "词单",
            h_flex()
                .gap_2()
                .child(div().flex_none().text_sm().text_color(theme.muted_foreground).child(if q.is_empty() {
                    format!("{} 词", self.cards.len())
                } else {
                    format!("{} / {}", shown.len(), self.cards.len())
                }))
                .child(div().w(px(240.)).min_w(px(120.)).flex_shrink(1.).child(Input::new(&self.search).small().cleanable(true)))
                .child(Button::new("reload-words").ghost().small().icon(IconName::Redo).tooltip("刷新").on_click(cx.listener(|this, _, _, cx| this.reload(cx)))),
            cx,
        );

        let body: AnyElement = if !self.loaded {
            ui::empty_state("载入中…", "", cx).into_any_element()
        } else if self.cards.is_empty() {
            ui::empty_state("词单还是空的", "在任意 App 里按住 Shift 指向日语，或按 ⌥⌘J，在弹窗里点「加入词单」。", cx).into_any_element()
        } else if shown.is_empty() {
            ui::empty_state("没有匹配的词", "换个关键词试试：单词、读音、例句或中文意思都能搜。", cx).into_any_element()
        } else {
            let mut list = v_flex().gap_px();
            for c in &shown {
                let id = c.card.id.clone();
                let open = self.expanded.as_deref() == Some(id.as_str());
                let summary = c
                    .card
                    .explanation
                    .as_ref()
                    .map(|e| e.meaning_zh.clone())
                    .filter(|s| !s.is_empty())
                    .or_else(|| c.card.definitions.first().map(|d| glossary::summary(&d.glossary, 80)))
                    .unwrap_or_default();
                let due = due_label(c, now);
                let is_due = c.schedule.phase != Phase::New && c.schedule.due <= now;
                let toggle_id = id.clone();
                let row = h_flex()
                    .id(SharedString::from(format!("row-{id}")))
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(open, |d| d.bg(theme.secondary))
                    .hover(|s| s.bg(theme.secondary))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.expanded = if this.expanded.as_deref() == Some(toggle_id.as_str()) { None } else { Some(toggle_id.clone()) };
                        this.confirm_delete = None;
                        cx.notify();
                    }))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_baseline()
                                    .child(div().flex_none().text_base().font_weight(FontWeight::SEMIBOLD).child(c.card.expression.clone()))
                                    .child(div().min_w_0().truncate().text_sm().text_color(theme.muted_foreground).child(c.card.reading.clone())),
                            )
                            .child(div().w_full().truncate().text_sm().text_color(theme.muted_foreground).child(summary)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .text_xs()
                            .when(is_due, |d| d.bg(theme.primary.opacity(0.15)).text_color(theme.primary))
                            .when(!is_due, |d| d.text_color(theme.muted_foreground))
                            .child(due),
                    );
                list = list.child(row);
                if open {
                    list = list.child(self.detail(c, cx));
                }
            }
            ui::scroll("words-scroll", ui::column(860.).py_3().child(list)).into_any_element()
        };
        ui::page(header, body)
    }
}
