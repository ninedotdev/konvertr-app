//! Shared shadcn-style dropzone: upload glyph in a circle, title, hint line.
//! Callers chain `.on_click(...)` / `.on_drop(...)` on the returned element.

use gpui::{SharedString, div, prelude::*, px};

use crate::theme::Theme;

pub fn drop_zone(
    theme: &Theme,
    empty: bool,
    title: &'static str,
    hint: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    let zone = div()
        .id("drop-zone")
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(Theme::PANEL_RADIUS))
        .border_1()
        .border_dashed()
        .border_color(theme.border_strong)
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface).border_color(theme.text_faint));

    if empty {
        zone.flex_col()
            .flex_1()
            .gap(px(Theme::SPACE_SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(44.))
                    .rounded_full()
                    .bg(theme.surface_hover)
                    .text_size(px(18.))
                    .text_color(theme.text_muted)
                    .child("↑"),
            )
            .child(div().text_size(px(13.)).text_color(theme.text).child(title))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.text_faint)
                    .child(hint.into()),
            )
    } else {
        zone.flex_row()
            .py(px(Theme::SPACE_MD))
            .gap(px(Theme::SPACE_SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(22.))
                    .rounded_full()
                    .bg(theme.surface_hover)
                    .text_size(px(11.))
                    .text_color(theme.text_muted)
                    .child("↑"),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_muted)
                    .child("drop more, or click to browse"),
            )
    }
}
