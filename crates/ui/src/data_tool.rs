//! Data converter: drop JSON/CSV/YAML files or browse, pick a target format,
//! convert on the background executor, outputs saved next to the inputs.

use std::path::PathBuf;

use gpui::{Context, ExternalPaths, PathPromptOptions, Task, Window, div, prelude::*, px};
use konvrt_core::data::{DataFormat, DataOutcome};

use crate::theme::Theme;

#[derive(Clone)]
enum FileStatus {
    Pending,
    Converting,
    Done(DataOutcome),
    Error(String),
}

struct FileEntry {
    path: PathBuf,
    size: u64,
    status: FileStatus,
}

pub struct DataTool {
    files: Vec<FileEntry>,
    format: DataFormat,
    converting: bool,
    task: Option<Task<()>>,
}

impl DataTool {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            files: Vec::new(),
            format: DataFormat::Json,
            converting: false,
            task: None,
        }
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if !konvrt_core::data::is_supported_input(&path) {
                continue;
            }
            if self.files.iter().any(|f| f.path == path) {
                continue;
            }
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            self.files.push(FileEntry {
                path,
                size,
                status: FileStatus::Pending,
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

    fn convert_all(&mut self, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let to = self.format;
        let jobs: Vec<(usize, PathBuf)> = self
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| !matches!(f.status, FileStatus::Done(_)))
            .map(|(ix, f)| (ix, f.path.clone()))
            .collect();
        if jobs.is_empty() {
            return;
        }
        self.converting = true;
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            for (ix, path) in jobs {
                this.update(cx, |tool, cx| {
                    tool.files[ix].status = FileStatus::Converting;
                    cx.notify();
                })
                .ok();
                let result = cx
                    .background_spawn(async move { konvrt_core::data::convert_file(&path, to) })
                    .await;
                this.update(cx, |tool, cx| {
                    tool.files[ix].status = match result {
                        Ok(outcome) => {
                            crate::history::push(
                                cx,
                                crate::history::HistoryEntry {
                                    tool: "data",
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
                            FileStatus::Done(outcome)
                        }
                        Err(e) => FileStatus::Error(format!("{e:#}")),
                    };
                    cx.notify();
                })
                .ok();
            }
            this.update(cx, |tool, cx| {
                tool.converting = false;
                cx.notify();
            })
            .ok();
        }));
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        self.files.clear();
        cx.notify();
    }

    fn set_format(&mut self, format: DataFormat, cx: &mut Context<Self>) {
        self.format = format;
        for file in &mut self.files {
            if matches!(file.status, FileStatus::Done(_) | FileStatus::Error(_)) {
                file.status = FileStatus::Pending;
            }
        }
        cx.notify();
    }

    fn render_drop_zone(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        crate::dropzone::drop_zone(
            theme,
            self.files.is_empty(),
            "drag & drop your data files here",
            "or click to browse · json · csv · yaml · toml",
        )
        .on_click(cx.listener(|this, _, _, cx| this.browse(cx)))
        .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
            this.add_paths(paths.paths().to_vec(), cx);
        }))
    }

    fn render_file_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let file = &self.files[ix];
        let name = file
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let status: gpui::AnyElement = match &file.status {
            FileStatus::Pending => div()
                .text_color(theme.text_faint)
                .child(format_size(file.size))
                .into_any_element(),
            FileStatus::Converting => div()
                .text_color(theme.text_muted)
                .child("converting…")
                .into_any_element(),
            FileStatus::Done(outcome) => {
                let out_path = outcome.out_path.clone();
                let delta = savings(outcome.in_size, outcome.out_size);
                div()
                    .id(("reveal", ix))
                    .flex()
                    .gap(px(Theme::SPACE_SM))
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&out_path)))
                    .child(
                        div()
                            .text_color(theme.success)
                            .child(format!("{delta} · {}", format_size(outcome.out_size))),
                    )
                    .child(div().text_color(theme.text_faint).child("reveal"))
                    .into_any_element()
            }
            FileStatus::Error(e) => div()
                .text_color(theme.danger)
                .max_w(px(320.))
                .truncate()
                .child(e.clone())
                .into_any_element(),
        };

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
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(theme.text)
                    .child(name),
            )
            .child(div().text_size(px(11.)).child(status))
            .child(
                div()
                    .id(("remove", ix))
                    .px(px(Theme::SPACE_XS))
                    .text_color(theme.text_faint)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.danger))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.converting {
                            this.files.remove(ix);
                            cx.notify();
                        }
                    }))
                    .child("×"),
            )
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let pending = self
            .files
            .iter()
            .filter(|f| !matches!(f.status, FileStatus::Done(_)))
            .count();

        let mut format_chips = div().flex().flex_wrap().gap(px(Theme::SPACE_XS));
        for format in DataFormat::ALL {
            let selected = self.format == format;
            format_chips = format_chips.child(
                div()
                    .id(format.label())
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
                    .on_click(cx.listener(move |this, _, _, cx| this.set_format(format, cx)))
                    .child(format.label()),
            );
        }

        let controls = div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_MD))
            .child(labeled(
                theme,
                "output format",
                format_chips.into_any_element(),
            ));

        let can_convert = pending > 0 && !self.converting;
        controls.child(
            div()
                .flex()
                .gap(px(Theme::SPACE_SM))
                .child(
                    div()
                        .id("convert")
                        .px(px(Theme::SPACE_LG))
                        .py(px(6.))
                        .rounded(px(Theme::CONTROL_RADIUS))
                        .bg(if can_convert {
                            theme.accent
                        } else {
                            theme.surface
                        })
                        .text_color(if can_convert {
                            theme.on_accent
                        } else {
                            theme.text_faint
                        })
                        .text_size(px(12.))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.convert_all(cx)))
                        .child(if self.converting {
                            "converting…".to_string()
                        } else if pending == 1 {
                            "convert 1 file".to_string()
                        } else {
                            format!("convert {pending} files")
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
                        .on_click(cx.listener(|this, _, _, cx| this.clear(cx)))
                        .child("clear all"),
                ),
        )
    }
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

fn savings(input: u64, output: u64) -> String {
    if input == 0 {
        return "±0%".to_string();
    }
    let pct = 100.0 - (output as f64 / input as f64) * 100.0;
    if pct >= 0.0 {
        format!("-{pct:.0}%")
    } else {
        format!("+{:.0}%", -pct)
    }
}

impl Render for DataTool {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let has_files = !self.files.is_empty();

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
                    .child("data converter"),
            )
            .child(self.render_drop_zone(cx));

        if has_files {
            let mut rows = div()
                .id("file-list")
                .flex()
                .flex_col()
                .flex_1()
                .gap(px(Theme::SPACE_XS))
                .overflow_y_scroll();
            for ix in 0..self.files.len() {
                rows = rows.child(self.render_file_row(ix, cx));
            }
            pane = pane.child(rows).child(self.render_controls(cx));
        }

        pane
    }
}
