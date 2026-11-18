//! Shared layout pieces, so every page has the same frame: a header bar,
//! a body that fills the rest of the window, and scrolling with a scrollbar.
//!
//! Flex children must be allowed to shrink below their content (`min_h_0` /
//! `min_w_0`), otherwise long content pushes everything below it out of the
//! window instead of scrolling.

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::*;

/// Readable width for text-heavy pages.
pub const READ_WIDTH: f32 = 720.;

/// Header bar: title on the left, `right` (actions, search…) after it.
pub fn header(title: &str, right: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .h(px(52.))
        .px_5()
        .gap_3()
        .items_center()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(div().flex_none().text_lg().font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
        .child(h_flex().flex_1().min_w_0().gap_2().items_center().justify_end().child(right))
}

/// A page: header on top, `body` filling (and clipped to) the rest.
pub fn page(header: impl IntoElement, body: impl IntoElement) -> Div {
    v_flex()
        .size_full()
        .min_h_0()
        .overflow_hidden()
        .child(header)
        .child(div().flex_1().min_h_0().overflow_hidden().child(body))
}

/// Vertical scroll area with a scrollbar, filling its parent.
pub fn scroll(id: impl Into<ElementId>, content: impl IntoElement) -> impl IntoElement {
    div().id(id.into()).size_full().overflow_y_scrollbar().child(content)
}

/// Centered column of readable width with page padding.
pub fn column(max_width: f32) -> Div {
    v_flex().w_full().max_w(px(max_width)).mx_auto().px_5().py_5()
}

/// Centered message for empty states.
pub fn empty_state(title: &str, hint: &str, cx: &App) -> Div {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .p_6()
        .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
        .child(div().max_w(px(420.)).text_sm().text_center().text_color(cx.theme().muted_foreground).child(hint.to_string()))
}
