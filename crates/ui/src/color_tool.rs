//! Color converter: live-parses hex/rgb/hsl/oklch as you type and shows all
//! four representations with per-format copy, plus a swatch.

use gpui::{ClipboardItem, Context, Entity, Subscription, Window, div, prelude::*, px};
use konvrt_core::color::{Rgba, parse};

use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;

pub struct ColorTool {
    input: Entity<TextInput>,
    parsed: Option<Rgba>,
    _subscription: Subscription,
}

impl ColorTool {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new(cx, "#7c9cff · rgb(…) · hsl(…) · oklch(…)"));
        let subscription = cx.subscribe(&input, |this: &mut ColorTool, input, event, cx| {
            let TextInputEvent::Edited = event;
            this.parsed = parse(input.read(cx).text().trim());
            cx.notify();
        });
        Self {
            input,
            parsed: None,
            _subscription: subscription,
        }
    }
}

fn swatch_color(c: &Rgba) -> gpui::Hsla {
    gpui::Rgba {
        r: (c.r / 255.0) as f32,
        g: (c.g / 255.0) as f32,
        b: (c.b / 255.0) as f32,
        a: c.a as f32,
    }
    .into()
}

impl Render for ColorTool {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let mut pane = div()
            .flex()
            .flex_col()
            .size_full()
            .p(px(Theme::SPACE_LG))
            .gap(px(Theme::SPACE_MD))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme.text)
                    .child("color converter"),
            )
            .child(self.input.clone());

        if let Some(color) = &self.parsed {
            let outputs = [
                ("hex", color.to_hex()),
                ("rgb", color.to_rgb()),
                ("hsl", color.to_hsl()),
                ("oklch", color.to_oklch()),
            ];
            pane = pane.child(
                div()
                    .h(px(56.))
                    .rounded(px(Theme::PANEL_RADIUS))
                    .border_1()
                    .border_color(theme.border_strong)
                    .bg(swatch_color(color)),
            );
            for (ix, (label, value)) in outputs.into_iter().enumerate() {
                let copy_value = value.clone();
                pane = pane.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(Theme::SPACE_MD))
                        .px(px(Theme::SPACE_MD))
                        .py(px(Theme::SPACE_SM))
                        .rounded(px(Theme::CONTROL_RADIUS))
                        .bg(theme.surface)
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(theme.text_faint)
                                .child(label.to_uppercase()),
                        )
                        .child(div().flex_1().text_color(theme.text).child(value))
                        .child(
                            div()
                                .id(("copy", ix))
                                .text_size(px(11.))
                                .text_color(theme.text_faint)
                                .cursor_pointer()
                                .hover(|s| s.text_color(theme.text))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_value.clone(),
                                    ));
                                }))
                                .child("copy"),
                        ),
                );
            }
        } else if !self.input.read(cx).text().trim().is_empty() {
            pane = pane.child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_faint)
                    .child("can't parse that — try #hex, rgb(), hsl() or oklch()"),
            );
        }

        pane
    }
}
