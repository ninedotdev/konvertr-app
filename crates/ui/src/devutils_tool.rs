//! Dev utils: four stacked mini-sections — epoch converter, url
//! encode/decode, uuid v4 generator, jwt decoder — each a live TextInput with
//! copyable output rows.

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{ClipboardItem, Context, Entity, Subscription, Window, div, prelude::*, px};
use konvrt_core::devutils;

use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;

pub struct DevUtilsTool {
    epoch_input: Entity<TextInput>,
    url_input: Entity<TextInput>,
    jwt_input: Entity<TextInput>,
    /// Most recent first, capped at 5.
    uuids: Vec<String>,
    _subscriptions: Vec<Subscription>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl DevUtilsTool {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let epoch_input = cx.new(|cx| TextInput::new(cx, "1712345678 · 2024-04-05T12:00:00Z"));
        let url_input = cx.new(|cx| TextInput::new(cx, "text or percent-encoded url"));
        let jwt_input = cx.new(|cx| TextInput::new(cx, "paste a jwt (header.payload.signature)"));
        let subscriptions = [&epoch_input, &url_input, &jwt_input]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |_: &mut DevUtilsTool, _, event, cx| {
                    let TextInputEvent::Edited = event;
                    cx.notify();
                })
            })
            .collect();
        Self {
            epoch_input,
            url_input,
            jwt_input,
            uuids: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    fn out_row(
        &self,
        id: (&'static str, usize),
        label: &'static str,
        value: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::of(cx);
        let copy_value = value.clone();
        div()
            .flex()
            .items_start()
            .gap(px(Theme::SPACE_MD))
            .px(px(Theme::SPACE_MD))
            .py(px(Theme::SPACE_SM))
            .rounded(px(Theme::CONTROL_RADIUS))
            .bg(theme.surface)
            .child(
                div()
                    .w(px(64.))
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(theme.text_faint)
                    .child(label.to_uppercase()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(11.))
                    .text_color(theme.text)
                    .child(value),
            )
            .child(
                div()
                    .id(id)
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(theme.text_faint)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.text))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy_value.clone()));
                    }))
                    .child("copy"),
            )
    }

    fn hint(&self, text: impl Into<gpui::SharedString>, cx: &Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .text_size(px(11.))
            .text_color(theme.text_faint)
            .child(text.into())
    }

    fn section(&self, label: &'static str, cx: &Context<Self>) -> gpui::Div {
        let theme = Theme::of(cx);
        div().flex().flex_col().gap(px(Theme::SPACE_XS)).child(
            div()
                .text_size(px(10.))
                .text_color(theme.text_faint)
                .child(label.to_uppercase()),
        )
    }

    fn render_epoch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.epoch_input.read(cx).text().trim().to_string();
        let mut section = self
            .section("epoch converter", cx)
            .child(self.epoch_input.clone());

        if let Some(secs) = devutils::parse_epoch(&text) {
            section = section
                .child(self.out_row(("epoch-utc", 0), "utc", devutils::utc_iso(secs), cx))
                .child(self.out_row(
                    ("epoch-rel", 1),
                    "relative",
                    devutils::relative(secs, now_secs()),
                    cx,
                ));
        } else if let Some(secs) = devutils::iso_to_epoch(&text) {
            section = section
                .child(self.out_row(("epoch-s", 2), "seconds", secs.to_string(), cx))
                .child(self.out_row(("epoch-ms", 3), "millis", (secs * 1000).to_string(), cx))
                .child(self.out_row(
                    ("epoch-rel2", 4),
                    "relative",
                    devutils::relative(secs, now_secs()),
                    cx,
                ));
        } else if !text.is_empty() {
            section = section.child(self.hint("enter an epoch integer or an ISO date (UTC)", cx));
        }
        section
    }

    fn render_url(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.url_input.read(cx).text().to_string();
        let mut section = self
            .section("url encode / decode", cx)
            .child(self.url_input.clone());
        if !text.trim().is_empty() {
            section = section.child(self.out_row(
                ("url-enc", 0),
                "encoded",
                devutils::url_encode(&text),
                cx,
            ));
            match devutils::url_decode(&text) {
                Ok(decoded) => {
                    section = section.child(self.out_row(("url-dec", 1), "decoded", decoded, cx))
                }
                Err(e) => section = section.child(self.hint(format!("can't decode: {e:#}"), cx)),
            }
        }
        section
    }

    fn render_uuid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let mut section = self.section("uuid v4", cx).child(
            div()
                .id("uuid-generate")
                .w(px(96.))
                .px(px(Theme::SPACE_MD))
                .py(px(6.))
                .rounded(px(Theme::CONTROL_RADIUS))
                .bg(theme.accent)
                .text_color(theme.on_accent)
                .text_size(px(12.))
                .text_center()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.uuids.insert(0, devutils::uuid_v4());
                    this.uuids.truncate(5);
                    cx.notify();
                }))
                .child("generate"),
        );
        for (ix, uuid) in self.uuids.iter().enumerate() {
            section = section.child(self.out_row(("uuid", ix), "uuid", uuid.clone(), cx));
        }
        section
    }

    fn render_jwt(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.jwt_input.read(cx).text().trim().to_string();
        let mut section = self
            .section("jwt decoder", cx)
            .child(self.jwt_input.clone())
            .child(self.hint("decoded, not verified — the signature is never checked", cx));
        if !text.is_empty() {
            match devutils::jwt_decode(&text) {
                Ok(parts) => {
                    section = section
                        .child(self.out_row(("jwt-header", 0), "header", parts.header, cx))
                        .child(self.out_row(("jwt-payload", 1), "payload", parts.payload, cx));
                }
                Err(e) => section = section.child(self.hint(format!("{e:#}"), cx)),
            }
        }
        section
    }
}

impl Render for DevUtilsTool {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .id("devutils")
            .flex()
            .flex_col()
            .size_full()
            .p(px(Theme::SPACE_LG))
            .gap(px(Theme::SPACE_LG))
            .overflow_y_scroll()
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme.text)
                    .child("dev utils"),
            )
            .child(self.render_epoch(cx))
            .child(self.render_url(cx))
            .child(self.render_uuid(cx))
            .child(self.render_jwt(cx))
    }
}
