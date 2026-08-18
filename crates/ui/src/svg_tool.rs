//! SVG optimizer: drop .svg files, optimize on the background executor,
//! outputs saved next to the inputs with savings shown per file.

use std::path::{Path, PathBuf};

use gpui::{Context, ExternalPaths, PathPromptOptions, Task, Window, div, prelude::*, px};
use konvrt_core::svg::SvgOutcome;

use crate::theme::Theme;

#[derive(Clone)]
enum FileStatus {
    Pending,
    Optimizing,
    Done(SvgOutcome),
    Error(String),
}

struct FileEntry {
    path: PathBuf,
    size: u64,
    status: FileStatus,
}

fn is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
}

pub struct SvgTool {
    files: Vec<FileEntry>,
    optimizing: bool,
    task: Option<Task<()>>,
}

impl SvgTool {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            files: Vec::new(),
            optimizing: false,
            task: None,
        }
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if !is_svg(&path) {
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

    fn optimize_all(&mut self, cx: &mut Context<Self>) {
        if self.optimizing {
            return;
        }
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
        self.optimizing = true;
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            for (ix, path) in jobs {
                this.update(cx, |tool, cx| {
                    tool.files[ix].status = FileStatus::Optimizing;
                    cx.notify();
                })
                .ok();
                let result = cx
                    .background_spawn(async move { konvrt_core::svg::optimize_file(&path) })
                    .await;
                this.update(cx, |tool, cx| {
                    tool.files[ix].status = match result {
                        Ok(outcome) => {
                            crate::history::push(
                                cx,
                                crate::history::HistoryEntry {
                                    tool: "svg",
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
                tool.optimizing = false;
                cx.notify();
            })
            .ok();
        }));
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        if self.optimizing {
            return;
        }
        self.files.clear();
        cx.notify();
    }

    fn render_drop_zone(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        crate::dropzone::drop_zone(
            theme,
            self.files.is_empty(),
            "drag & drop your svg files here",
            "or click to browse · strips comments, metadata, editor cruft",
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
            FileStatus::Optimizing => div()
                .text_color(theme.text_muted)
                .child("optimizing…")
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
                        if !this.optimizing {
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
        let can_optimize = pending > 0 && !self.optimizing;

        div()
            .flex()
            .gap(px(Theme::SPACE_SM))
            .child(
                div()
                    .id("optimize")
                    .px(px(Theme::SPACE_LG))
                    .py(px(6.))
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .bg(if can_optimize {
                        theme.accent
                    } else {
                        theme.surface
                    })
                    .text_color(if can_optimize {
                        theme.on_accent
                    } else {
                        theme.text_faint
                    })
                    .text_size(px(12.))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.optimize_all(cx)))
                    .child(if self.optimizing {
                        "optimizing…".to_string()
                    } else if pending == 1 {
                        "optimize 1 file".to_string()
                    } else {
                        format!("optimize {pending} files")
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
            )
    }
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

impl Render for SvgTool {
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
                    .child("svg optimizer"),
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
