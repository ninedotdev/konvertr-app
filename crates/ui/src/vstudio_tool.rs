//! Video studio: compress to a target size, lossless trim, GIF studio, and
//! frame extraction — one input video, mode chips, live ffmpeg progress.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use gpui::{
    Context, Entity, ExternalPaths, PathPromptOptions, Subscription, Task, Window, div, prelude::*,
    px, relative,
};
use konvrt_core::video::{find_ffmpeg, is_supported_input, probe_duration_secs};
use konvrt_core::vstudio::{
    COMPRESS_PRESETS, GifOptions, StudioOutcome, compress_file, contact_sheet_file,
    format_timestamp, frame_file, gif_file, parse_timestamp, trim_file, video_bitrate_kbps,
};

use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;

const AUDIO_KBPS: u32 = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Compress,
    Trim,
    Gif,
    Frame,
}

impl Mode {
    const ALL: [Mode; 4] = [Mode::Compress, Mode::Trim, Mode::Gif, Mode::Frame];

    fn label(self) -> &'static str {
        match self {
            Mode::Compress => "compress",
            Mode::Trim => "trim",
            Mode::Gif => "gif",
            Mode::Frame => "frame",
        }
    }

    fn verb(self) -> &'static str {
        match self {
            Mode::Compress => "compress",
            Mode::Trim => "trim",
            Mode::Gif => "make gif",
            Mode::Frame => "extract",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Preset(usize),
    Custom,
}

#[derive(Clone, Copy)]
enum Job {
    Compress(f64),
    Trim(f64, Option<f64>),
    Gif(GifOptions),
    Frame(f64),
    Sheet,
}

enum Status {
    Idle,
    Running,
    Done(StudioOutcome),
    Error(String),
}

struct InputFile {
    path: PathBuf,
    size: u64,
    duration: Option<f64>,
}

pub struct VStudioTool {
    ffmpeg: Option<PathBuf>,
    input: Option<InputFile>,
    mode: Mode,
    target: Target,
    custom_mb: Entity<TextInput>,
    start_input: Entity<TextInput>,
    end_input: Entity<TextInput>,
    gif_fps: u32,
    gif_width: u32,
    gif_loop: bool,
    contact_sheet: bool,
    status: Status,
    progress: Arc<AtomicU8>,
    task: Option<Task<()>>,
    poll_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl VStudioTool {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let custom_mb = cx.new(|cx| TextInput::new(cx, "MB"));
        let start_input = cx.new(|cx| TextInput::new(cx, "0:00"));
        let end_input = cx.new(|cx| TextInput::new(cx, "end"));
        let subs = [&custom_mb, &start_input, &end_input]
            .map(|input| cx.subscribe(input, |_: &mut Self, _, _: &TextInputEvent, cx| cx.notify()))
            .into_iter()
            .collect();
        Self {
            ffmpeg: find_ffmpeg(),
            input: None,
            mode: Mode::Compress,
            target: Target::Preset(0),
            custom_mb,
            start_input,
            end_input,
            gif_fps: 12,
            gif_width: 480,
            gif_loop: true,
            contact_sheet: false,
            status: Status::Idle,
            progress: Arc::new(AtomicU8::new(0)),
            task: None,
            poll_task: None,
            _subs: subs,
        }
    }

    fn set_input(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !is_supported_input(&path) {
            return;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        self.input = Some(InputFile {
            path: path.clone(),
            size,
            duration: None,
        });
        self.status = Status::Idle;
        cx.notify();
        let Some(ffmpeg) = self.ffmpeg.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let duration = cx
                .background_spawn(async move { probe_duration_secs(&ffmpeg, &path) })
                .await;
            this.update(cx, |tool, cx| {
                if let Some(input) = &mut tool.input {
                    input.duration = duration;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |tool, cx| tool.set_input(path, cx)).ok();
            }
        })
        .detach();
    }

    /// The start/end fields, parsed; empty start means 0, empty end means None.
    fn parsed_range(&self, cx: &Context<Self>) -> Result<(f64, Option<f64>), String> {
        let start_text = self.start_input.read(cx).text().trim().to_string();
        let end_text = self.end_input.read(cx).text().trim().to_string();
        let start = if start_text.is_empty() {
            0.0
        } else {
            parse_timestamp(&start_text).ok_or("invalid start time")?
        };
        let end = if end_text.is_empty() {
            None
        } else {
            Some(parse_timestamp(&end_text).ok_or("invalid end time")?)
        };
        if let Some(end) = end
            && end <= start
        {
            return Err("end must be after start".into());
        }
        Ok((start, end))
    }

    fn target_mb(&self, cx: &Context<Self>) -> Result<f64, String> {
        match self.target {
            Target::Preset(ix) => Ok(COMPRESS_PRESETS[ix].1),
            Target::Custom => {
                let text = self.custom_mb.read(cx).text().trim().to_string();
                let mb: f64 = text.parse().map_err(|_| "enter a size in MB")?;
                if mb <= 0.0 {
                    return Err("enter a size in MB".into());
                }
                Ok(mb)
            }
        }
    }

    fn current_job(&self, cx: &Context<Self>) -> Result<Job, String> {
        match self.mode {
            Mode::Compress => Ok(Job::Compress(self.target_mb(cx)?)),
            Mode::Trim => {
                let (start, end) = self.parsed_range(cx)?;
                Ok(Job::Trim(start, end))
            }
            Mode::Gif => {
                let (start, end) = self.parsed_range(cx)?;
                Ok(Job::Gif(GifOptions {
                    start,
                    end,
                    fps: self.gif_fps,
                    width: self.gif_width,
                    loop_forever: self.gif_loop,
                    dither: true,
                }))
            }
            Mode::Frame => {
                if self.contact_sheet {
                    Ok(Job::Sheet)
                } else {
                    let (start, _) = self.parsed_range(cx)?;
                    Ok(Job::Frame(start))
                }
            }
        }
    }

    fn run_job(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, Status::Running) {
            return;
        }
        let Some(ffmpeg) = self.ffmpeg.clone() else {
            return;
        };
        let Some(input) = &self.input else {
            return;
        };
        let path = input.path.clone();
        let job = match self.current_job(cx) {
            Ok(job) => job,
            Err(e) => {
                self.status = Status::Error(e);
                cx.notify();
                return;
            }
        };
        self.status = Status::Running;
        self.progress.store(0, Ordering::Relaxed);
        cx.notify();
        // Repaint on a timer while running so the progress bar tracks the
        // AtomicU8 the background thread writes into.
        self.poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let running = this
                    .update(cx, |tool, cx| {
                        cx.notify();
                        matches!(tool.status, Status::Running)
                    })
                    .unwrap_or(false);
                if !running {
                    break;
                }
            }
        }));
        let progress = self.progress.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match job {
                        Job::Compress(mb) => {
                            compress_file(&ffmpeg, &path, mb, AUDIO_KBPS, &progress)
                        }
                        Job::Trim(start, end) => trim_file(&ffmpeg, &path, start, end, &progress),
                        Job::Gif(opts) => gif_file(&ffmpeg, &path, &opts, &progress),
                        Job::Frame(at) => frame_file(&ffmpeg, &path, at, &progress),
                        Job::Sheet => contact_sheet_file(&ffmpeg, &path, 4, 4, 320, &progress),
                    }
                })
                .await;
            this.update(cx, |tool, cx| {
                tool.status = match result {
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
                        Status::Done(outcome)
                    }
                    Err(e) => Status::Error(format!("{e:#}")),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn chip(
        &self,
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

    fn render_file_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let input = self.input.as_ref().expect("input present");
        let name = input
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let meta = match input.duration {
            Some(d) => format!("{} · {}", format_timestamp(d), format_size(input.size)),
            None => format_size(input.size),
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
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_faint)
                    .child(meta),
            )
            .child(
                div()
                    .id("remove-input")
                    .px(px(Theme::SPACE_XS))
                    .text_color(theme.text_faint)
                    .cursor_pointer()
                    .hover(|s| s.text_color(theme.danger))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !matches!(this.status, Status::Running) {
                            this.input = None;
                            this.status = Status::Idle;
                            cx.notify();
                        }
                    }))
                    .child("×"),
            )
    }

    fn render_mode_chips(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let mut chips = div().flex().gap(px(Theme::SPACE_XS));
        for mode in Mode::ALL {
            chips = chips.child(
                self.chip(theme, mode.label(), mode.label(), self.mode == mode)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.mode = mode;
                        if !matches!(this.status, Status::Running) {
                            this.status = Status::Idle;
                        }
                        cx.notify();
                    })),
            );
        }
        labeled(theme, "mode", chips.into_any_element())
    }

    fn time_field(
        &self,
        theme: &Theme,
        label: &'static str,
        input: &Entity<TextInput>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(Theme::SPACE_XS))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.text_faint)
                    .child(label),
            )
            .child(div().w(px(90.)).child(input.clone()))
    }

    fn render_compress_controls(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::of(cx);
        let mut chips = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(Theme::SPACE_XS));
        for (ix, (label, _)) in COMPRESS_PRESETS.iter().enumerate() {
            chips = chips.child(
                self.chip(theme, *label, *label, self.target == Target::Preset(ix))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.target = Target::Preset(ix);
                        cx.notify();
                    })),
            );
        }
        chips = chips
            .child(
                self.chip(theme, "custom", "custom", self.target == Target::Custom)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.target = Target::Custom;
                        cx.notify();
                    })),
            )
            .when(self.target == Target::Custom, |d| {
                d.child(div().w(px(70.)).child(self.custom_mb.clone()))
            });

        let prediction: gpui::AnyElement = match (
            self.target_mb(cx),
            self.input.as_ref().and_then(|i| i.duration),
        ) {
            (Ok(mb), Some(duration)) => match video_bitrate_kbps(mb, duration, AUDIO_KBPS) {
                Ok(v) => div()
                    .text_size(px(10.))
                    .text_color(theme.text_muted)
                    .child(format!(
                        "≈ {v} kbps video · {AUDIO_KBPS} kbps audio · two-pass"
                    ))
                    .into_any_element(),
                Err(e) => div()
                    .text_size(px(10.))
                    .text_color(theme.danger)
                    .child(format!("{e:#}"))
                    .into_any_element(),
            },
            _ => div()
                .text_size(px(10.))
                .text_color(theme.text_faint)
                .child("")
                .into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_XS))
            .child(labeled(theme, "target size", chips.into_any_element()))
            .child(prediction)
            .into_any_element()
    }

    fn render_range_readout(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::of(cx);
        match self.parsed_range(cx) {
            Ok((start, end)) => {
                let end = end.or(self.input.as_ref().and_then(|i| i.duration));
                match end {
                    Some(end) if end > start => div()
                        .text_size(px(10.))
                        .text_color(theme.text_muted)
                        .child(format!("duration: {}", format_timestamp(end - start)))
                        .into_any_element(),
                    _ => div()
                        .text_size(px(10.))
                        .text_color(theme.text_faint)
                        .child("")
                        .into_any_element(),
                }
            }
            Err(e) => div()
                .text_size(px(10.))
                .text_color(theme.danger)
                .child(e)
                .into_any_element(),
        }
    }

    fn render_trim_controls(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::of(cx);
        let fields = div()
            .flex()
            .items_center()
            .gap(px(Theme::SPACE_MD))
            .child(self.time_field(theme, "start", &self.start_input))
            .child(self.time_field(theme, "end", &self.end_input));
        div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_XS))
            .child(labeled(
                theme,
                "trim range (no re-encode)",
                fields.into_any_element(),
            ))
            .child(self.render_range_readout(cx))
            .into_any_element()
    }

    fn render_gif_controls(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::of(cx);
        let mut fps_chips = div().flex().gap(px(Theme::SPACE_XS));
        for fps in [10u32, 12, 15, 24] {
            fps_chips = fps_chips.child(
                self.chip(
                    theme,
                    ("fps", fps as usize),
                    format!("{fps}"),
                    self.gif_fps == fps,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.gif_fps = fps;
                    cx.notify();
                })),
            );
        }
        let mut width_chips = div().flex().gap(px(Theme::SPACE_XS));
        for width in [320u32, 480, 640] {
            width_chips = width_chips.child(
                self.chip(
                    theme,
                    ("width", width as usize),
                    format!("{width}px"),
                    self.gif_width == width,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.gif_width = width;
                    cx.notify();
                })),
            );
        }
        let fields = div()
            .flex()
            .items_center()
            .gap(px(Theme::SPACE_MD))
            .child(self.time_field(theme, "start", &self.start_input))
            .child(self.time_field(theme, "end", &self.end_input))
            .child(
                self.chip(theme, "gif-loop", "loop", self.gif_loop)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.gif_loop = !this.gif_loop;
                        cx.notify();
                    })),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_MD))
            .child(labeled(theme, "fps", fps_chips.into_any_element()))
            .child(labeled(theme, "width", width_chips.into_any_element()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(Theme::SPACE_XS))
                    .child(labeled(theme, "range", fields.into_any_element()))
                    .child(self.render_range_readout(cx)),
            )
            .into_any_element()
    }

    fn render_frame_controls(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::of(cx);
        let fields = div()
            .flex()
            .items_center()
            .gap(px(Theme::SPACE_MD))
            .child(self.time_field(theme, "at", &self.start_input))
            .child(
                self.chip(
                    theme,
                    "contact-sheet",
                    "contact sheet 4×4",
                    self.contact_sheet,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.contact_sheet = !this.contact_sheet;
                    cx.notify();
                })),
            );
        let hint = if self.contact_sheet {
            "16 frames sampled across the whole video, tiled into one png"
        } else {
            "one png frame at the given timestamp"
        };
        div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_XS))
            .child(labeled(theme, "extract", fields.into_any_element()))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.text_faint)
                    .child(hint),
            )
            .into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::of(cx);
        match &self.status {
            Status::Idle => div().into_any_element(),
            Status::Running => {
                let pct = self.progress.load(Ordering::Relaxed).min(100);
                let label = if self.mode == Mode::Compress {
                    if pct < 50 {
                        format!("pass 1/2 · {pct}%")
                    } else {
                        format!("pass 2/2 · {pct}%")
                    }
                } else {
                    format!("{pct}%")
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(Theme::SPACE_SM))
                    .child(
                        div()
                            .flex_1()
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
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child(label),
                    )
                    .into_any_element()
            }
            Status::Done(outcome) => {
                let out_path = outcome.out_path.clone();
                let name = out_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let delta = savings(outcome.in_size, outcome.out_size);
                div()
                    .id("result")
                    .flex()
                    .items_center()
                    .gap(px(Theme::SPACE_SM))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&out_path)))
                    .child(
                        div()
                            .text_color(theme.success)
                            .max_w(px(360.))
                            .truncate()
                            .child(format!(
                                "{name} · {delta} · {}",
                                format_size(outcome.out_size)
                            )),
                    )
                    .child(div().text_color(theme.text_faint).child("reveal"))
                    .into_any_element()
            }
            Status::Error(e) => div()
                .text_size(px(11.))
                .text_color(theme.danger)
                .max_w(px(480.))
                .truncate()
                .child(e.clone())
                .into_any_element(),
        }
    }

    fn render_run_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let running = matches!(self.status, Status::Running);
        let can_run = !running && self.input.is_some() && self.current_job(cx).is_ok();
        div()
            .id("run")
            .px(px(Theme::SPACE_LG))
            .py(px(6.))
            .rounded(px(Theme::CONTROL_RADIUS))
            .bg(if can_run { theme.accent } else { theme.surface })
            .text_color(if can_run {
                theme.on_accent
            } else {
                theme.text_faint
            })
            .text_size(px(12.))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.run_job(cx)))
            .child(if running {
                "working…"
            } else {
                self.mode.verb()
            })
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

impl Render for VStudioTool {
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
                    .child("video studio"),
            );

        if self.ffmpeg.is_none() {
            return pane.child(self.render_missing_ffmpeg(cx));
        }

        if self.input.is_none() {
            pane = pane.child(
                crate::dropzone::drop_zone(
                    theme,
                    true,
                    "drag & drop a video here",
                    "or click to browse · compress · trim · gif · frames",
                )
                .on_click(cx.listener(|this, _, _, cx| this.browse(cx)))
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    if let Some(path) = paths.paths().first().cloned() {
                        this.set_input(path, cx);
                    }
                })),
            );
            return pane;
        }

        let controls = match self.mode {
            Mode::Compress => self.render_compress_controls(cx),
            Mode::Trim => self.render_trim_controls(cx),
            Mode::Gif => self.render_gif_controls(cx),
            Mode::Frame => self.render_frame_controls(cx),
        };

        pane.child(self.render_file_card(cx))
            .child(self.render_mode_chips(cx))
            .child(controls)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(Theme::SPACE_MD))
                    .child(self.render_run_button(cx))
                    .child(div().flex_1().child(self.render_status(cx))),
            )
    }
}
