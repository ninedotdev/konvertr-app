//! Image kit: batch social-size presets, ASCII art, and palette extraction.

use std::path::PathBuf;

use gpui::{
    ClipboardItem, Context, ExternalPaths, PathPromptOptions, Task, Window, div, prelude::*, px,
};
use konvrt_core::imgkit::{self, Fit, ImgOutcome, Preset};

use crate::image_tool::shimmer;
use crate::theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Resize,
    Ascii,
    Palette,
}

impl Mode {
    const ALL: [Mode; 3] = [Mode::Resize, Mode::Ascii, Mode::Palette];

    fn label(self) -> &'static str {
        match self {
            Mode::Resize => "resize",
            Mode::Ascii => "ascii art",
            Mode::Palette => "palette",
        }
    }
}

#[derive(Clone)]
enum Status {
    Pending,
    Working,
    Resized(ImgOutcome),
    Ascii(String),
    Palette(Vec<[u8; 3]>),
    Error(String),
}

struct FileEntry {
    path: PathBuf,
    size: u64,
    status: Status,
}

pub struct ImgKitTool {
    files: Vec<FileEntry>,
    mode: Mode,
    preset: Preset,
    fit: Fit,
    cols: u32,
    invert: bool,
    colors: usize,
    working: bool,
    task: Option<Task<()>>,
}

impl ImgKitTool {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            files: Vec::new(),
            mode: Mode::Resize,
            preset: Preset::OgImage,
            fit: Fit::Crop,
            cols: 100,
            invert: false,
            colors: 6,
            working: false,
            task: None,
        }
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if !konvrt_core::is_supported_input(&path) || self.files.iter().any(|f| f.path == path)
            {
                continue;
            }
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            self.files.push(FileEntry {
                path,
                size,
                status: Status::Pending,
            });
        }
        cx.notify();
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |tool, cx| tool.add_paths(paths, cx)).ok();
            }
        })
        .detach();
    }

    fn reset_statuses(&mut self, cx: &mut Context<Self>) {
        for file in &mut self.files {
            file.status = Status::Pending;
        }
        cx.notify();
    }

    fn run_all(&mut self, cx: &mut Context<Self>) {
        if self.working || self.files.is_empty() {
            return;
        }
        let (mode, preset, fit, cols, invert, colors) = (
            self.mode,
            self.preset,
            self.fit,
            self.cols,
            self.invert,
            self.colors,
        );
        let jobs: Vec<(usize, PathBuf)> = self
            .files
            .iter()
            .enumerate()
            .map(|(ix, f)| (ix, f.path.clone()))
            .collect();
        self.working = true;
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            for (ix, path) in jobs {
                this.update(cx, |tool, cx| {
                    tool.files[ix].status = Status::Working;
                    cx.notify();
                })
                .ok();
                let status = cx
                    .background_spawn(async move {
                        match mode {
                            Mode::Resize => imgkit::resize_file(&path, preset, fit)
                                .map(Status::Resized)
                                .unwrap_or_else(|e| Status::Error(format!("{e:#}"))),
                            Mode::Ascii => imgkit::ascii_from_file(&path, cols, invert)
                                .map(Status::Ascii)
                                .unwrap_or_else(|e| Status::Error(format!("{e:#}"))),
                            Mode::Palette => imgkit::palette_from_file(&path, colors)
                                .map(Status::Palette)
                                .unwrap_or_else(|e| Status::Error(format!("{e:#}"))),
                        }
                    })
                    .await;
                this.update(cx, |tool, cx| {
                    if let Status::Resized(outcome) = &status {
                        crate::history::push(
                            cx,
                            crate::history::HistoryEntry {
                                tool: "img",
                                name: outcome
                                    .out_path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                                out_path: outcome.out_path.clone(),
                                in_size: outcome.in_size,
                                out_size: outcome.out_size,
                            },
                        );
                    }
                    tool.files[ix].status = status;
                    cx.notify();
                })
                .ok();
            }
            this.update(cx, |tool, cx| {
                tool.working = false;
                cx.notify();
            })
            .ok();
        }));
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let file = &self.files[ix];
        let name = file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut row = div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_SM))
            .px(px(Theme::SPACE_MD))
            .py(px(Theme::SPACE_SM))
            .rounded(px(Theme::CONTROL_RADIUS))
            .bg(theme.surface);

        let head: gpui::AnyElement = match &file.status {
            Status::Pending => div()
                .text_size(px(11.))
                .text_color(theme.text_faint)
                .child(format_size(file.size))
                .into_any_element(),
            Status::Working => shimmer(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_muted)
                    .child("working…"),
                ("imgkit-shimmer", ix),
            )
            .into_any_element(),
            Status::Resized(outcome) => {
                let out_path = outcome.out_path.clone();
                div()
                    .flex()
                    .gap(px(Theme::SPACE_SM))
                    .text_size(px(11.))
                    .child(
                        div()
                            .text_color(theme.success)
                            .child(format!("{}×{}", outcome.width, outcome.height)),
                    )
                    .child(
                        div()
                            .text_color(theme.text_faint)
                            .child(format_size(outcome.out_size)),
                    )
                    .child(
                        div()
                            .id(("reveal", ix))
                            .text_color(theme.text_faint)
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.text))
                            .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&out_path)))
                            .child("reveal"),
                    )
                    .into_any_element()
            }
            Status::Ascii(art) => {
                let copy = art.clone();
                let save_path = file.path.with_extension("txt");
                let save = art.clone();
                div()
                    .flex()
                    .gap(px(Theme::SPACE_SM))
                    .text_size(px(11.))
                    .child(
                        div()
                            .id(("copy-ascii", ix))
                            .text_color(theme.text_muted)
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.text))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            }))
                            .child("copy"),
                    )
                    .child(
                        div()
                            .id(("save-ascii", ix))
                            .text_color(theme.text_faint)
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.text))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if std::fs::write(&save_path, &save).is_ok() {
                                    cx.reveal_path(&save_path);
                                }
                            }))
                            .child("save .txt"),
                    )
                    .into_any_element()
            }
            Status::Palette(colors) => div()
                .text_size(px(11.))
                .text_color(theme.text_faint)
                .child(format!("{} colors", colors.len()))
                .into_any_element(),
            Status::Error(e) => div()
                .text_size(px(11.))
                .text_color(theme.danger)
                .max_w(px(360.))
                .truncate()
                .child(e.clone())
                .into_any_element(),
        };

        row = row.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(Theme::SPACE_MD))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(theme.text)
                        .child(name),
                )
                .child(head)
                .child(
                    div()
                        .id(("remove", ix))
                        .px(px(Theme::SPACE_XS))
                        .text_color(theme.text_faint)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.danger))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.working {
                                this.files.remove(ix);
                                cx.notify();
                            }
                        }))
                        .child("×"),
                ),
        );

        // Inline results: the ascii preview and the palette swatches.
        match &file.status {
            Status::Ascii(art) => {
                let preview: String = art.lines().take(14).collect::<Vec<_>>().join("\n");
                row = row.child(
                    div()
                        .p(px(Theme::SPACE_SM))
                        .rounded(px(4.))
                        .bg(theme.surface_hover)
                        .text_size(px(7.))
                        .line_height(px(7.))
                        .text_color(theme.text_muted)
                        .children(
                            preview
                                .lines()
                                .map(|l| div().child(l.to_string()))
                                .collect::<Vec<_>>(),
                        ),
                );
            }
            Status::Palette(colors) => {
                let mut swatches = div().flex().flex_wrap().gap(px(Theme::SPACE_XS));
                for (ci, color) in colors.iter().enumerate() {
                    let hex = imgkit::hex(*color);
                    let copy = hex.clone();
                    swatches = swatches.child(
                        div()
                            .id(("swatch", ix * 100 + ci))
                            .flex()
                            .items_center()
                            .gap(px(Theme::SPACE_XS))
                            .px(px(Theme::SPACE_SM))
                            .py(px(3.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme.border)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.surface_hover))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            }))
                            .child(div().size(px(12.)).rounded(px(3.)).bg(gpui::Rgba {
                                r: color[0] as f32 / 255.0,
                                g: color[1] as f32 / 255.0,
                                b: color[2] as f32 / 255.0,
                                a: 1.0,
                            }))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(theme.text_muted)
                                    .child(hex),
                            ),
                    );
                }
                row = row.child(swatches);
            }
            _ => {}
        }

        row
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let mut controls = div().flex().flex_col().gap(px(Theme::SPACE_MD));

        match self.mode {
            Mode::Resize => {
                let mut presets = div().flex().flex_wrap().gap(px(Theme::SPACE_XS));
                for preset in Preset::ALL {
                    let selected = self.preset == preset;
                    presets = presets.child(
                        chip(theme, preset.slug(), preset.label(), selected).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.preset = preset;
                                this.reset_statuses(cx);
                            },
                        )),
                    );
                }
                let mut fits = div().flex().gap(px(Theme::SPACE_XS));
                for fit in Fit::ALL {
                    let selected = self.fit == fit;
                    fits = fits.child(chip(theme, fit.label(), fit.label(), selected).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.fit = fit;
                            this.reset_statuses(cx);
                        }),
                    ));
                }
                controls = controls
                    .child(labeled(theme, "size", presets.into_any_element()))
                    .child(labeled(theme, "framing", fits.into_any_element()));
            }
            Mode::Ascii => {
                let mut widths = div().flex().gap(px(Theme::SPACE_XS));
                for cols in [60u32, 100, 160, 240] {
                    let selected = self.cols == cols;
                    widths = widths.child(
                        chip(
                            theme,
                            ("cols", cols as usize),
                            format!("{cols} cols"),
                            selected,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.cols = cols;
                            this.reset_statuses(cx);
                        })),
                    );
                }
                controls = controls
                    .child(labeled(theme, "width", widths.into_any_element()))
                    .child(labeled(
                        theme,
                        "tone",
                        chip(
                            theme,
                            "invert",
                            "invert (for dark backgrounds)",
                            self.invert,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.invert = !this.invert;
                            this.reset_statuses(cx);
                        }))
                        .into_any_element(),
                    ));
            }
            Mode::Palette => {
                let mut counts = div().flex().gap(px(Theme::SPACE_XS));
                for k in [3usize, 5, 6, 8, 10] {
                    let selected = self.colors == k;
                    counts =
                        counts.child(chip(theme, ("k", k), format!("{k}"), selected).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.colors = k;
                                this.reset_statuses(cx);
                            }),
                        ));
                }
                controls = controls.child(labeled(theme, "colors", counts.into_any_element()));
            }
        }

        let count = self.files.len();
        controls.child(
            div()
                .flex()
                .gap(px(Theme::SPACE_SM))
                .child(
                    div()
                        .id("run")
                        .px(px(Theme::SPACE_LG))
                        .py(px(6.))
                        .rounded(px(Theme::CONTROL_RADIUS))
                        .bg(if self.working {
                            theme.surface
                        } else {
                            theme.accent
                        })
                        .text_color(if self.working {
                            theme.text_faint
                        } else {
                            theme.on_accent
                        })
                        .text_size(px(12.))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.run_all(cx)))
                        .child(if self.working {
                            shimmer(div().child("working…"), "imgkit-run-shimmer")
                                .into_any_element()
                        } else {
                            div()
                                .child(match self.mode {
                                    Mode::Resize => format!("resize {count}"),
                                    Mode::Ascii => format!("asciify {count}"),
                                    Mode::Palette => format!("extract from {count}"),
                                })
                                .into_any_element()
                        }),
                )
                .child(
                    div()
                        .id("clear")
                        .px(px(Theme::SPACE_MD))
                        .py(px(6.))
                        .rounded(px(Theme::CONTROL_RADIUS))
                        .border_1()
                        .border_color(theme.border)
                        .text_color(theme.text_muted)
                        .text_size(px(12.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.surface_hover))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.working {
                                this.files.clear();
                                cx.notify();
                            }
                        }))
                        .child("clear all"),
                ),
        )
    }
}

fn chip(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
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
        .child(label.into())
}

fn labeled(theme: &Theme, label: &'static str, content: gpui::AnyElement) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(Theme::SPACE_XS))
        .child(
            div()
                .text_size(px(10.))
                .text_color(theme.text_faint)
                .child(label.to_uppercase()),
        )
        .child(content)
}

fn format_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

impl Render for ImgKitTool {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let has_files = !self.files.is_empty();

        let mut modes = div().flex().gap(px(Theme::SPACE_XS));
        for mode in Mode::ALL {
            let selected = self.mode == mode;
            modes = modes.child(chip(theme, mode.label(), mode.label(), selected).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.mode = mode;
                    this.reset_statuses(cx);
                }),
            ));
        }

        let mut pane = div()
            .flex()
            .flex_col()
            .size_full()
            .p(px(Theme::SPACE_LG))
            .gap(px(Theme::SPACE_MD))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.text)
                            .child("image kit"),
                    )
                    .child(modes),
            )
            .child(
                crate::dropzone::drop_zone(
                    theme,
                    self.files.is_empty(),
                    "drag & drop your images here",
                    "or click to browse · social presets · ascii art · color palettes",
                )
                .on_click(cx.listener(|this, _, _, cx| this.browse(cx)))
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    this.add_paths(paths.paths().to_vec(), cx);
                })),
            );

        if has_files {
            let mut rows = div()
                .id("imgkit-list")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.))
                .gap(px(Theme::SPACE_XS))
                .overflow_y_scroll();
            for ix in 0..self.files.len() {
                rows = rows.child(self.render_row(ix, cx));
            }
            pane = pane.child(rows).child(self.render_controls(cx));
        }

        pane
    }
}
