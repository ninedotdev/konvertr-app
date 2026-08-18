//! Base64 tool: drop any files, pick encode/decode, run on the background
//! executor, outputs saved next to the inputs.

use std::path::PathBuf;

use gpui::{Context, ExternalPaths, PathPromptOptions, Task, Window, div, prelude::*, px};
use konvrt_core::b64::B64Outcome;

use crate::theme::Theme;

#[derive(Clone)]
enum FileStatus {
    Pending,
    Converting,
    Done(B64Outcome),
    Error(String),
}

struct FileEntry {
    path: PathBuf,
    size: u64,
    status: FileStatus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Encode,
    Decode,
}

impl Mode {
    const ALL: [Mode; 2] = [Mode::Encode, Mode::Decode];

    fn label(self) -> &'static str {
        match self {
            Mode::Encode => "encode",
            Mode::Decode => "decode",
        }
    }
}

pub struct B64Tool {
    files: Vec<FileEntry>,
    mode: Mode,
    converting: bool,
    task: Option<Task<()>>,
}

impl B64Tool {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            files: Vec::new(),
            mode: Mode::Encode,
            converting: false,
            task: None,
        }
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if !path.is_file() {
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
        let mode = self.mode;
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
                    .background_spawn(async move {
                        match mode {
                            Mode::Encode => konvrt_core::b64::encode_file(&path),
                            Mode::Decode => konvrt_core::b64::decode_file(&path),
                        }
                    })
                    .await;
                this.update(cx, |tool, cx| {
                    tool.files[ix].status = match result {
                        Ok(outcome) => {
                            crate::history::push(
                                cx,
                                crate::history::HistoryEntry {
                                    tool: "b64",
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

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
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
            "drag & drop any files here",
            "or click to browse · any file → data URL · base64 text → file",
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
                let out_name = out_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                div()
                    .id(("reveal", ix))
                    .flex()
                    .gap(px(Theme::SPACE_SM))
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&out_path)))
                    .child(
                        div()
                            .text_color(theme.success)
                            .max_w(px(320.))
                            .truncate()
                            .child(format!("{out_name} · {}", format_size(outcome.out_size))),
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

        let mut mode_chips = div().flex().gap(px(Theme::SPACE_XS));
        for mode in Mode::ALL {
            let selected = self.mode == mode;
            mode_chips = mode_chips.child(
                div()
                    .id(mode.label())
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
                    .on_click(cx.listener(move |this, _, _, cx| this.set_mode(mode, cx)))
                    .child(mode.label()),
            );
        }

        let can_convert = pending > 0 && !self.converting;
        let verb = self.mode.label();
        div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_MD))
            .child(labeled(theme, "mode", mode_chips.into_any_element()))
            .child(
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
                                "working…".to_string()
                            } else if pending == 1 {
                                format!("{verb} 1 file")
                            } else {
                                format!("{verb} {pending} files")
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

impl Render for B64Tool {
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
                    .child("base64 encoder / decoder"),
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
