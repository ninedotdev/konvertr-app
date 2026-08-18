//! Video converter: drop files or browse, pick a format + quality, run ffmpeg
//! on the background executor with live per-file progress.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use gpui::{
    Context, ExternalPaths, PathPromptOptions, Task, Window, div, prelude::*, px, relative,
};
use konvrt_core::video::{
    VideoFormat, VideoOutcome, VideoQuality, convert_file, find_ffmpeg, is_supported_input,
};

use crate::theme::Theme;

#[derive(Clone)]
enum FileStatus {
    Pending,
    Converting,
    Done(VideoOutcome),
    Error(String),
}

struct FileEntry {
    path: PathBuf,
    size: u64,
    status: FileStatus,
    progress: Arc<AtomicU8>,
}

pub struct VideoTool {
    files: Vec<FileEntry>,
    format: VideoFormat,
    quality: VideoQuality,
    converting: bool,
    ffmpeg: Option<PathBuf>,
    task: Option<Task<()>>,
    poll_task: Option<Task<()>>,
}

impl VideoTool {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            files: Vec::new(),
            format: VideoFormat::Mp4,
            quality: VideoQuality::Balanced,
            converting: false,
            ffmpeg: find_ffmpeg(),
            task: None,
            poll_task: None,
        }
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            if !is_supported_input(&path) {
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
                progress: Arc::new(AtomicU8::new(0)),
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
        let Some(ffmpeg) = self.ffmpeg.clone() else {
            return;
        };
        let format = self.format;
        let quality = self.quality;
        let jobs: Vec<(usize, PathBuf, Arc<AtomicU8>)> = self
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| !matches!(f.status, FileStatus::Done(_)))
            .map(|(ix, f)| (ix, f.path.clone(), f.progress.clone()))
            .collect();
        if jobs.is_empty() {
            return;
        }
        self.converting = true;
        cx.notify();
        // Repaint on a timer while converting so the progress bars track the
        // AtomicU8 the background thread writes into.
        self.poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let converting = this
                    .update(cx, |tool, cx| {
                        cx.notify();
                        tool.converting
                    })
                    .unwrap_or(false);
                if !converting {
                    break;
                }
            }
        }));
        self.task = Some(cx.spawn(async move |this, cx| {
            for (ix, path, progress) in jobs {
                this.update(cx, |tool, cx| {
                    tool.files[ix].progress.store(0, Ordering::Relaxed);
                    tool.files[ix].status = FileStatus::Converting;
                    cx.notify();
                })
                .ok();
                let ffmpeg = ffmpeg.clone();
                let result = cx
                    .background_spawn(async move {
                        convert_file(&ffmpeg, &path, format, quality, &progress)
                    })
                    .await;
                this.update(cx, |tool, cx| {
                    tool.files[ix].status = match result {
                        Ok(outcome) => {
                            crate::history::push(
                                cx,
                                crate::history::HistoryEntry {
                                    tool: "vid",
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

    fn set_format(&mut self, format: VideoFormat, cx: &mut Context<Self>) {
        self.format = format;
        for file in &mut self.files {
            if matches!(file.status, FileStatus::Done(_) | FileStatus::Error(_)) {
                file.status = FileStatus::Pending;
            }
        }
        cx.notify();
    }

    fn render_missing_ffmpeg(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(Theme::SPACE_SM))
            .rounded(px(Theme::PANEL_RADIUS))
            .border_1()
            .border_dashed()
            .border_color(theme.border_strong)
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme.text_muted)
                    .child("ffmpeg not found — brew install ffmpeg (or bundle it later)"),
            )
            .child(
                div()
                    .id("retry-ffmpeg")
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
                        this.ffmpeg = find_ffmpeg();
                        cx.notify();
                    }))
                    .child("retry"),
            )
    }

    fn render_drop_zone(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        crate::dropzone::drop_zone(
            theme,
            self.files.is_empty(),
            "drag & drop your videos here",
            "or click to browse · mp4 · webm · mov · avi · mkv · flv · wmv · gif · mpeg · ts",
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
            FileStatus::Converting => {
                let pct = file.progress.load(Ordering::Relaxed).min(100);
                div()
                    .flex()
                    .items_center()
                    .gap(px(Theme::SPACE_SM))
                    .child(
                        div()
                            .w(px(120.))
                            .h(px(3.))
                            .rounded(px(2.))
                            .bg(theme.surface_hover)
                            .child(
                                div()
                                    .w(relative(pct as f32 / 100.0))
                                    .h_full()
                                    .rounded(px(2.))
                                    .bg(theme.accent),
                            ),
                    )
                    .child(div().text_color(theme.text_muted).child(format!("{pct}%")))
                    .into_any_element()
            }
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
        for format in VideoFormat::ALL {
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

        let mut controls = div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_MD))
            .child(labeled(
                theme,
                "output format",
                format_chips.into_any_element(),
            ));

        if self.format.supports_quality() {
            let mut quality_chips = div().flex().gap(px(Theme::SPACE_XS));
            for quality in VideoQuality::ALL {
                let selected = self.quality == quality;
                quality_chips = quality_chips.child(
                    div()
                        .id(quality.label())
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.quality = quality;
                            cx.notify();
                        }))
                        .child(quality.label()),
                );
            }
            controls = controls.child(labeled(theme, "quality", quality_chips.into_any_element()));
        }

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
                            "convert 1 video".to_string()
                        } else {
                            format!("convert {pending} videos")
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
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB", b / (1024.0 * 1024.0 * 1024.0))
    } else if b >= 1024.0 * 1024.0 {
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

impl Render for VideoTool {
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
                    .child("video converter"),
            );

        if self.ffmpeg.is_none() {
            return pane.child(self.render_missing_ffmpeg(cx));
        }

        pane = pane.child(self.render_drop_zone(cx));

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
