//! Text kit: section chips switching between case conversions, line tools,
//! diff, markdown, regex tester, text stats and lorem ipsum. Inputs are
//! single-line for now: multi-line pastes collapse to one line.

use gpui::{ClipboardItem, Context, Entity, Subscription, Window, div, prelude::*, px};
use konvrt_core::textkit;

use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Case,
    Lines,
    Diff,
    Markdown,
    Regex,
    Stats,
    Lorem,
}

impl Section {
    const ALL: [Section; 7] = [
        Section::Case,
        Section::Lines,
        Section::Diff,
        Section::Markdown,
        Section::Regex,
        Section::Stats,
        Section::Lorem,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::Case => "case",
            Section::Lines => "lines",
            Section::Diff => "diff",
            Section::Markdown => "markdown",
            Section::Regex => "regex",
            Section::Stats => "stats",
            Section::Lorem => "lorem",
        }
    }
}

pub struct TextKitTool {
    section: Section,
    case_input: Entity<TextInput>,
    lines_input: Entity<TextInput>,
    line_op: textkit::LineOp,
    diff_a: Entity<TextInput>,
    diff_b: Entity<TextInput>,
    md_input: Entity<TextInput>,
    regex_pattern: Entity<TextInput>,
    regex_haystack: Entity<TextInput>,
    regex_i: bool,
    regex_m: bool,
    regex_s: bool,
    stats_input: Entity<TextInput>,
    lorem_paragraphs: usize,
    lorem_words: usize,
    _subscriptions: Vec<Subscription>,
}

impl TextKitTool {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let case_input = cx.new(|cx| TextInput::new(cx, "helloWorld · hello_world · Hello World"));
        let lines_input = cx.new(|cx| TextInput::new(cx, "some lines of text"));
        let diff_a = cx.new(|cx| TextInput::new(cx, "before"));
        let diff_b = cx.new(|cx| TextInput::new(cx, "after"));
        let md_input = cx.new(|cx| TextInput::new(cx, "# markdown source"));
        let regex_pattern = cx.new(|cx| TextInput::new(cx, r"pattern, e.g. (\w+)@(\w+)"));
        let regex_haystack = cx.new(|cx| TextInput::new(cx, "text to match against"));
        let stats_input = cx.new(|cx| TextInput::new(cx, "paste text to count"));

        let subscriptions = [
            &case_input,
            &lines_input,
            &diff_a,
            &diff_b,
            &md_input,
            &regex_pattern,
            &regex_haystack,
            &stats_input,
        ]
        .into_iter()
        .map(|input| {
            cx.subscribe(input, |_: &mut TextKitTool, _, event, cx| {
                let TextInputEvent::Edited = event;
                cx.notify();
            })
        })
        .collect();

        Self {
            section: Section::Case,
            case_input,
            lines_input,
            line_op: textkit::LineOp::Sort,
            diff_a,
            diff_b,
            md_input,
            regex_pattern,
            regex_haystack,
            regex_i: false,
            regex_m: false,
            regex_s: false,
            stats_input,
            lorem_paragraphs: 2,
            lorem_words: 30,
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
                    .w(px(72.))
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

    fn chip(
        &self,
        id: (&'static str, usize),
        label: &'static str,
        selected: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .id(id)
            .px(px(Theme::SPACE_SM))
            .py(px(3.))
            .rounded(px(Theme::CONTROL_RADIUS))
            .border_1()
            .border_color(if selected {
                theme.border_strong
            } else {
                theme.border
            })
            .when(selected, |d| {
                d.bg(theme.surface_hover).text_color(theme.text)
            })
            .when(!selected, |d| d.text_color(theme.text_muted))
            .text_size(px(11.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_hover))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .child(label)
    }

    fn render_case(&self, cx: &mut Context<Self>) -> gpui::Div {
        let text = self.case_input.read(cx).text().trim().to_string();
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_XS))
            .child(self.case_input.clone());
        if !text.is_empty() {
            let outputs = [
                ("camel", textkit::to_camel(&text)),
                ("pascal", textkit::to_pascal(&text)),
                ("snake", textkit::to_snake(&text)),
                ("kebab", textkit::to_kebab(&text)),
                ("constant", textkit::to_constant(&text)),
                ("title", textkit::to_title(&text)),
                ("slug", textkit::slugify(&text)),
            ];
            for (ix, (label, value)) in outputs.into_iter().enumerate() {
                body = body.child(self.out_row(("case", ix), label, value, cx));
            }
        }
        body
    }

    fn render_lines(&self, cx: &mut Context<Self>) -> gpui::Div {
        let text = self.lines_input.read(cx).text().to_string();
        let mut chips = div().flex().flex_wrap().gap(px(Theme::SPACE_XS));
        for (ix, op) in textkit::LineOp::ALL.into_iter().enumerate() {
            chips = chips.child(self.chip(
                ("line-op", ix),
                op.label(),
                self.line_op == op,
                move |this, cx| {
                    this.line_op = op;
                    cx.notify();
                },
                cx,
            ));
        }
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_XS))
            .child(self.lines_input.clone())
            .child(chips)
            .child(self.hint(
                "single-line input for now — multi-line pastes collapse to one line",
                cx,
            ));
        if !text.is_empty() {
            body = body.child(self.out_row(
                ("lines-out", 0),
                "result",
                textkit::apply_lines(&text, self.line_op),
                cx,
            ));
        }
        body
    }

    fn render_diff(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = Theme::of(cx);
        let a = self.diff_a.read(cx).text().to_string();
        let b = self.diff_b.read(cx).text().to_string();
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_XS))
            .child(self.diff_a.clone())
            .child(self.diff_b.clone())
            .child(self.hint(
                "line diff — single-line inputs for now, so this compares one line each",
                cx,
            ));
        if !a.is_empty() || !b.is_empty() {
            let lines = textkit::diff_lines(&a, &b);
            let (added, removed) = textkit::diff_stats(&lines);
            body = body.child(
                div()
                    .flex()
                    .gap(px(Theme::SPACE_MD))
                    .text_size(px(11.))
                    .child(div().text_color(theme.success).child(format!("+{added}")))
                    .child(div().text_color(theme.danger).child(format!("-{removed}"))),
            );
            let mut list = div()
                .flex()
                .flex_col()
                .rounded(px(Theme::CONTROL_RADIUS))
                .bg(theme.surface)
                .px(px(Theme::SPACE_MD))
                .py(px(Theme::SPACE_SM));
            for line in lines {
                let (prefix, color) = match line.kind {
                    textkit::DiffKind::Equal => ("  ", theme.text_muted),
                    textkit::DiffKind::Insert => ("+ ", theme.success),
                    textkit::DiffKind::Delete => ("- ", theme.danger),
                };
                list = list.child(
                    div()
                        .text_size(px(11.))
                        .text_color(color)
                        .child(format!("{prefix}{}", line.text)),
                );
            }
            body = body.child(list);
        }
        body
    }

    fn render_markdown(&self, cx: &mut Context<Self>) -> gpui::Div {
        let text = self.md_input.read(cx).text().to_string();
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_XS))
            .child(self.md_input.clone())
            .child(self.hint("tables · strikethrough · footnotes enabled", cx));
        if !text.trim().is_empty() {
            body = body.child(self.out_row(
                ("md-html", 0),
                "html",
                textkit::markdown_to_html(&text),
                cx,
            ));
        }
        body
    }

    fn render_regex(&self, cx: &mut Context<Self>) -> gpui::Div {
        // Copy the color out so the theme borrow doesn't overlap &mut cx uses.
        let danger = Theme::of(cx).danger;
        let pattern = self.regex_pattern.read(cx).text().to_string();
        let haystack = self.regex_haystack.read(cx).text().to_string();
        let mut flags = String::new();
        if self.regex_i {
            flags.push('i');
        }
        if self.regex_m {
            flags.push('m');
        }
        if self.regex_s {
            flags.push('s');
        }

        type FlagDef = (&'static str, bool, fn(&mut TextKitTool));
        let flag_defs: [FlagDef; 3] = [
            ("i — ignore case", self.regex_i, |t| {
                t.regex_i = !t.regex_i
            }),
            ("m — multiline ^$", self.regex_m, |t| {
                t.regex_m = !t.regex_m
            }),
            ("s — dot matches \\n", self.regex_s, |t| {
                t.regex_s = !t.regex_s
            }),
        ];
        let mut chips = div().flex().flex_wrap().gap(px(Theme::SPACE_XS));
        for (ix, (label, selected, toggle)) in flag_defs.into_iter().enumerate() {
            chips = chips.child(self.chip(
                ("regex-flag", ix),
                label,
                selected,
                move |this, cx| {
                    toggle(this);
                    cx.notify();
                },
                cx,
            ));
        }

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_XS))
            .child(self.regex_pattern.clone())
            .child(self.regex_haystack.clone())
            .child(chips);

        if !pattern.is_empty() {
            match textkit::regex_test(&pattern, &flags, &haystack) {
                Ok(matches) if matches.is_empty() => {
                    body = body.child(self.hint("no matches", cx));
                }
                Ok(matches) => {
                    body = body.child(self.hint(
                        format!(
                            "{} match{}",
                            matches.len(),
                            if matches.len() == 1 { "" } else { "es" }
                        ),
                        cx,
                    ));
                    for (ix, m) in matches.into_iter().enumerate().take(50) {
                        let mut desc = format!("[{}..{}] {}", m.start, m.end, m.text);
                        for (g, group) in m.groups.iter().enumerate() {
                            match group {
                                Some(text) => desc.push_str(&format!("  ${}={text}", g + 1)),
                                None => desc.push_str(&format!("  ${}=∅", g + 1)),
                            }
                        }
                        body = body.child(self.out_row(("regex-match", ix), "match", desc, cx));
                    }
                }
                Err(e) => {
                    body = body.child(
                        div()
                            .text_size(px(11.))
                            .text_color(danger)
                            .child(format!("{e:#}")),
                    );
                }
            }
        }
        body
    }

    fn render_stats(&self, cx: &mut Context<Self>) -> gpui::Div {
        let text = self.stats_input.read(cx).text().to_string();
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_XS))
            .child(self.stats_input.clone());
        if !text.is_empty() {
            let stats = textkit::count_stats(&text);
            let reading = if stats.reading_secs >= 60 {
                format!("{}m {}s", stats.reading_secs / 60, stats.reading_secs % 60)
            } else {
                format!("{}s", stats.reading_secs)
            };
            let rows = [
                ("chars", stats.chars.to_string()),
                ("no spaces", stats.chars_no_spaces.to_string()),
                ("words", stats.words.to_string()),
                ("lines", stats.lines.to_string()),
                ("reading", reading),
            ];
            for (ix, (label, value)) in rows.into_iter().enumerate() {
                body = body.child(self.out_row(("stat", ix), label, value, cx));
            }
        }
        body
    }

    fn render_lorem(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut para_chips = div().flex().gap(px(Theme::SPACE_XS));
        for (ix, n) in [1usize, 2, 3, 5].into_iter().enumerate() {
            para_chips = para_chips.child(self.chip(
                ("lorem-para", ix),
                match n {
                    1 => "1 para",
                    2 => "2 paras",
                    3 => "3 paras",
                    _ => "5 paras",
                },
                self.lorem_paragraphs == n,
                move |this, cx| {
                    this.lorem_paragraphs = n;
                    cx.notify();
                },
                cx,
            ));
        }
        let mut word_chips = div().flex().gap(px(Theme::SPACE_XS));
        for (ix, n) in [10usize, 30, 60].into_iter().enumerate() {
            word_chips = word_chips.child(self.chip(
                ("lorem-words", ix),
                match n {
                    10 => "10 words",
                    30 => "30 words",
                    _ => "60 words",
                },
                self.lorem_words == n,
                move |this, cx| {
                    this.lorem_words = n;
                    cx.notify();
                },
                cx,
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_XS))
            .child(para_chips)
            .child(word_chips)
            .child(self.out_row(
                ("lorem-out", 0),
                "lorem",
                textkit::lorem(self.lorem_paragraphs, self.lorem_words),
                cx,
            ))
    }
}

impl Render for TextKitTool {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let title = theme.text;

        let mut section_chips = div().flex().flex_wrap().gap(px(Theme::SPACE_XS));
        for (ix, section) in Section::ALL.into_iter().enumerate() {
            section_chips = section_chips.child(self.chip(
                ("section", ix),
                section.label(),
                self.section == section,
                move |this, cx| {
                    this.section = section;
                    cx.notify();
                },
                cx,
            ));
        }

        let body = match self.section {
            Section::Case => self.render_case(cx),
            Section::Lines => self.render_lines(cx),
            Section::Diff => self.render_diff(cx),
            Section::Markdown => self.render_markdown(cx),
            Section::Regex => self.render_regex(cx),
            Section::Stats => self.render_stats(cx),
            Section::Lorem => self.render_lorem(cx),
        };

        div()
            .id("textkit")
            .flex()
            .flex_col()
            .size_full()
            .p(px(Theme::SPACE_LG))
            .gap(px(Theme::SPACE_MD))
            .overflow_y_scroll()
            .child(div().text_size(px(13.)).text_color(title).child("text kit"))
            .child(section_chips)
            .child(body)
    }
}
