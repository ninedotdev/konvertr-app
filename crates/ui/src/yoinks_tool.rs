//! loader: paste a video URL, it auto-probes the available formats, pick a
//! quality chip, and yt-dlp downloads into ~/Downloads with live progress.
//! Centered single-column layout under the animated LOADER banner.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{Context, Entity, Subscription, Task, Window, div, prelude::*, px, relative};
use konvrt_core::yoinks::{
    DownloadChoice, ProbeResult, Progress, YoinkOutcome, download, downloads_dir, find_ytdlp,
    format_bytes, is_probable_url, probe, update_ytdlp,
};

use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;

/// Errors where a stale yt-dlp is the usual culprit — offer self-update.
const UPDATE_HINTS: [&str; 3] = [
    "unable to download",
    "Requested format is not available",
    "Sign in to confirm",
];

enum ProbeState {
    Idle,
    Probing,
    Ready(ProbeResult),
    Failed(String),
}

enum JobStatus {
    Queued,
    Downloading,
    Updating,
    Done(YoinkOutcome),
    Error(String),
}

struct Job {
    id: u64,
    url: String,
    title: String,
    choice: DownloadChoice,
    info_json: Option<PathBuf>,
    status: JobStatus,
    progress: Arc<Mutex<Progress>>,
}

pub struct YoinksTool {
    input: Entity<TextInput>,
    banner_phase: f32,
    banner_task: Option<Task<()>>,
    last_probed: Option<String>,
    probe_state: ProbeState,
    selected: usize,
    jobs: Vec<Job>,
    next_id: u64,
    downloading: bool,
    ytdlp: Option<PathBuf>,
    task: Option<Task<()>>,
    poll_task: Option<Task<()>>,
    _subscription: Subscription,
}

impl YoinksTool {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let input =
            cx.new(|cx| TextInput::new(cx, "paste a video URL (youtube, x, tiktok, instagram…)"));
        let subscription = cx.subscribe(&input, |this: &mut YoinksTool, input, event, cx| {
            let TextInputEvent::Edited = event;
            let text = input.read(cx).text().trim().to_string();
            if is_probable_url(&text) {
                if this.last_probed.as_deref() != Some(text.as_str()) {
                    this.start_probe(text, cx);
                }
            } else {
                this.last_probed = None;
                this.probe_state = ProbeState::Idle;
            }
            cx.notify();
        });
        Self {
            input,
            banner_phase: 0.0,
            banner_task: None,
            last_probed: None,
            probe_state: ProbeState::Idle,
            selected: 0,
            jobs: Vec::new(),
            next_id: 0,
            downloading: false,
            ytdlp: find_ytdlp(),
            task: None,
            poll_task: None,
            _subscription: subscription,
        }
    }

    fn start_probe(&mut self, url: String, cx: &mut Context<Self>) {
        let Some(ytdlp) = self.ytdlp.clone() else {
            return;
        };
        self.last_probed = Some(url.clone());
        self.probe_state = ProbeState::Probing;
        self.selected = 0;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let probe_url = url.clone();
            let result = cx
                .background_spawn(async move { probe(&ytdlp, &probe_url) })
                .await;
            this.update(cx, |tool, cx| {
                // A newer URL may have been probed meanwhile — drop stale results.
                if tool.last_probed.as_deref() != Some(url.as_str()) {
                    return;
                }
                tool.probe_state = match result {
                    Ok(probed) => ProbeState::Ready(probed),
                    Err(e) => ProbeState::Failed(format!("{e:#}")),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn grab(&mut self, cx: &mut Context<Self>) {
        let (Some(url), ProbeState::Ready(probed)) = (self.last_probed.clone(), &self.probe_state)
        else {
            return;
        };
        let Some(choice) = probed.choices.get(self.selected).cloned() else {
            return;
        };
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.push(Job {
            id,
            url,
            title: probed.title.clone(),
            choice,
            info_json: Some(probed.info_json_path.clone()),
            status: JobStatus::Queued,
            progress: Arc::new(Mutex::new(Progress::default())),
        });
        cx.notify();
        self.pump(cx);
    }

    /// Drain the queue serially; new jobs enqueued mid-run get picked up.
    fn pump(&mut self, cx: &mut Context<Self>) {
        if self.downloading {
            return;
        }
        let Some(ytdlp) = self.ytdlp.clone() else {
            return;
        };
        self.downloading = true;
        cx.notify();
        // Repaint on a timer while downloading so the progress rows track the
        // Mutex the background thread writes into.
        self.poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let downloading = this
                    .update(cx, |tool, cx| {
                        cx.notify();
                        tool.downloading
                    })
                    .unwrap_or(false);
                if !downloading {
                    break;
                }
            }
        }));
        self.task = Some(cx.spawn(async move |this, cx| {
            loop {
                let next = this
                    .update(cx, |tool, cx| {
                        let job = tool
                            .jobs
                            .iter_mut()
                            .find(|j| matches!(j.status, JobStatus::Queued))?;
                        job.status = JobStatus::Downloading;
                        *job.progress.lock().unwrap() = Progress::default();
                        cx.notify();
                        Some((
                            job.id,
                            job.url.clone(),
                            job.choice.clone(),
                            job.info_json.clone(),
                            job.progress.clone(),
                        ))
                    })
                    .ok()
                    .flatten();
                let Some((id, url, choice, info_json, progress)) = next else {
                    break;
                };
                let ytdlp = ytdlp.clone();
                let result = cx
                    .background_spawn(async move {
                        let ffmpeg = konvrt_core::video::find_ffmpeg();
                        download(
                            &ytdlp,
                            ffmpeg.as_deref(),
                            &url,
                            info_json.as_deref(),
                            &choice,
                            &downloads_dir(),
                            &progress,
                        )
                    })
                    .await;
                this.update(cx, |tool, cx| {
                    if let Some(job) = tool.jobs.iter_mut().find(|j| j.id == id) {
                        job.status = match result {
                            Ok(outcome) => {
                                let out_size = std::fs::metadata(&outcome.out_path)
                                    .map(|m| m.len())
                                    .unwrap_or(0);
                                crate::history::push(
                                    cx,
                                    crate::history::HistoryEntry {
                                        tool: "grab",
                                        name: outcome.title.clone(),
                                        out_path: outcome.out_path.clone(),
                                        in_size: 0,
                                        out_size,
                                    },
                                );
                                JobStatus::Done(outcome)
                            }
                            Err(e) => JobStatus::Error(format!("{e:#}")),
                        };
                    }
                    cx.notify();
                })
                .ok();
            }
            this.update(cx, |tool, cx| {
                tool.downloading = false;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Self-update yt-dlp, then re-enqueue the failed job.
    fn update_and_retry(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ytdlp) = self.ytdlp.clone() else {
            return;
        };
        let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) else {
            return;
        };
        job.status = JobStatus::Updating;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { update_ytdlp(&ytdlp) })
                .await;
            this.update(cx, |tool, cx| {
                if let Some(job) = tool.jobs.iter_mut().find(|j| j.id == id) {
                    match result {
                        Ok(_) => {
                            job.status = JobStatus::Queued;
                            tool.pump(cx);
                        }
                        Err(e) => job.status = JobStatus::Error(format!("{e:#}")),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_missing_ytdlp(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child("yt-dlp not found — brew install yt-dlp (bundling comes later)"),
            )
            .child(
                div()
                    .id("retry-ytdlp")
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
                        this.ytdlp = find_ytdlp();
                        cx.notify();
                    }))
                    .child("retry"),
            )
    }

    fn render_probe_area(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let theme = Theme::of(cx);
        match &self.probe_state {
            ProbeState::Idle => None,
            ProbeState::Probing => Some(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_muted)
                    .child("fetching formats…")
                    .into_any_element(),
            ),
            ProbeState::Failed(e) => Some(
                div()
                    .text_size(px(11.))
                    .text_color(theme.danger)
                    .line_clamp(2)
                    .child(e.clone())
                    .into_any_element(),
            ),
            ProbeState::Ready(probed) => {
                let mut meta = probed.title.clone();
                if let Some(uploader) = &probed.uploader {
                    meta.push_str(" · ");
                    meta.push_str(uploader);
                }
                if let Some(d) = probed.duration_secs {
                    let dur = format_duration(d);
                    if !dur.is_empty() {
                        meta.push_str(" · ");
                        meta.push_str(&dur);
                    }
                }

                let mut chips = div().flex().flex_wrap().gap(px(Theme::SPACE_XS));
                for (ix, choice) in probed.choices.iter().enumerate() {
                    let selected = ix == self.selected;
                    chips = chips.child(
                        div()
                            .id(("choice", ix))
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
                                this.selected = ix;
                                cx.notify();
                            }))
                            .child(choice.label.clone()),
                    );
                }

                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(Theme::SPACE_SM))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.text)
                                .truncate()
                                .child(meta),
                        )
                        .child(chips)
                        .child(
                            div().flex().justify_center().child(
                                div()
                                    .id("grab")
                                    .px(px(Theme::SPACE_LG))
                                    .py(px(6.))
                                    .rounded(px(Theme::CONTROL_RADIUS))
                                    .bg(theme.accent)
                                    .text_color(theme.on_accent)
                                    .text_size(px(12.))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.grab(cx)))
                                    .child("grab it"),
                            ),
                        )
                        .into_any_element(),
                )
            }
        }
    }

    fn render_job_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let job = &self.jobs[ix];

        let status: gpui::AnyElement = match &job.status {
            JobStatus::Queued => div()
                .text_color(theme.text_faint)
                .child(format!("queued · {}", job.choice.label))
                .into_any_element(),
            JobStatus::Updating => div()
                .text_color(theme.text_muted)
                .child("updating yt-dlp…")
                .into_any_element(),
            JobStatus::Downloading => {
                let p = job.progress.lock().unwrap().clone();
                if p.processing {
                    div()
                        .text_color(theme.text_muted)
                        .child("processing…")
                        .into_any_element()
                } else if p.retrying && p.downloaded == 0 {
                    div()
                        .text_color(theme.text_muted)
                        .child("retrying via fallback…")
                        .into_any_element()
                } else {
                    let pct = match p.total {
                        Some(total) if total > 0 => {
                            ((p.downloaded as f64 / total as f64) * 100.0).min(100.0) as u8
                        }
                        _ => 0,
                    };
                    let mut text = if p.retrying {
                        format!("fallback · {pct}%")
                    } else {
                        format!("{pct}%")
                    };
                    if let Some(speed) = p.speed {
                        let s = format_bytes(speed);
                        if !s.is_empty() {
                            text.push_str(&format!(" · {s}/s"));
                        }
                    }
                    if let Some(eta) = p.eta_secs {
                        text.push_str(&format!(" · eta {}", format_duration(eta as f64)));
                    }
                    if p.total_parts > 1 {
                        text.push_str(&format!(" · {}/{}", p.part + 1, p.total_parts));
                    }
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
                        .child(div().text_color(theme.text_muted).child(text))
                        .into_any_element()
                }
            }
            JobStatus::Done(outcome) => {
                let out_path = outcome.out_path.clone();
                div()
                    .id(("reveal", ix))
                    .flex()
                    .gap(px(Theme::SPACE_SM))
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&out_path)))
                    .child(div().text_color(theme.success).child("saved to Downloads"))
                    .child(div().text_color(theme.text_faint).child("reveal"))
                    .into_any_element()
            }
            JobStatus::Error(_) => div().into_any_element(),
        };

        let removable = !matches!(job.status, JobStatus::Downloading | JobStatus::Updating);
        let id = job.id;
        let mut row = div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_XS))
            .px(px(Theme::SPACE_MD))
            .py(px(Theme::SPACE_SM))
            .rounded(px(Theme::CONTROL_RADIUS))
            .bg(theme.surface)
            .child(
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
                            .child(job.title.clone()),
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
                                if removable {
                                    this.jobs.retain(|j| j.id != id);
                                    cx.notify();
                                }
                            }))
                            .child("×"),
                    ),
            );

        if let JobStatus::Error(e) = &job.status {
            let offer_update = UPDATE_HINTS.iter().any(|hint| e.contains(hint));
            let mut error_line = div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(Theme::SPACE_MD))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(11.))
                        .text_color(theme.danger)
                        .line_clamp(2)
                        .child(e.clone()),
                );
            if offer_update {
                error_line = error_line.child(
                    div()
                        .id(("update-retry", ix))
                        .flex_none()
                        .px(px(Theme::SPACE_SM))
                        .py(px(2.))
                        .rounded(px(Theme::CONTROL_RADIUS))
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(11.))
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.surface_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_and_retry(id, cx);
                        }))
                        .child("update yt-dlp & retry"),
                );
            }
            row = row.child(error_line);
        }

        row
    }
}

/// "3:25" / "1:02:03" (port of yoinks' formatDuration).
fn format_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return String::new();
    }
    let s = seconds.round() as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

impl Render for YoinksTool {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);

        // ~3s sweep, ticked every 120ms; started lazily so idle tabs with the
        // tool never constructed pay nothing.
        if self.banner_task.is_none() {
            self.banner_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(120))
                        .await;
                    if this
                        .update(cx, |tool, cx| {
                            tool.banner_phase = (tool.banner_phase + 0.04) % 1.0;
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }

        let pane = div().flex().flex_col().size_full().items_center();

        let mut column = div()
            .flex()
            .flex_col()
            .w_full()
            .max_w(px(560.))
            .flex_1()
            .min_h(px(0.))
            .pt(px(110.))
            .px(px(Theme::SPACE_LG))
            .pb(px(Theme::SPACE_LG))
            .gap(px(Theme::SPACE_MD))
            .child(crate::banner::loader_banner(theme, self.banner_phase));

        if self.ytdlp.is_none() {
            return pane.child(column.child(self.render_missing_ytdlp(cx)));
        }

        column = column.child(self.input.clone());
        if let Some(probe_area) = self.render_probe_area(cx) {
            column = column.child(probe_area);
        }

        if !self.jobs.is_empty() {
            let mut rows = div()
                .id("job-list")
                .flex()
                .flex_col()
                .flex_1()
                .gap(px(Theme::SPACE_XS))
                .overflow_y_scroll();
            for ix in 0..self.jobs.len() {
                rows = rows.child(self.render_job_row(ix, cx));
            }
            column = column.child(rows);
        }

        pane.child(column)
    }
}
