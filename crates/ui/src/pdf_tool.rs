//! PDF suite: merge / split / extract / rotate PDFs and turn images into a
//! PDF — all local, on the background executor. Merge order is controllable
//! per row; split/extract/rotate take a print-dialog page-range string.

use std::path::{Path, PathBuf};

use gpui::{
    Context, Entity, ExternalPaths, PathPromptOptions, Subscription, Task, Window, div, prelude::*,
    px,
};
use konvrt_core::pdf::{self, ImagePdfOptions, PdfOutcome, SplitMode, parse_ranges};

use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Merge,
    Split,
    Extract,
    Rotate,
    Images,
}

impl Mode {
    const ALL: [Mode; 5] = [
        Mode::Merge,
        Mode::Split,
        Mode::Extract,
        Mode::Rotate,
        Mode::Images,
    ];

    fn label(self) -> &'static str {
        match self {
            Mode::Merge => "merge",
            Mode::Split => "split",
            Mode::Extract => "extract",
            Mode::Rotate => "rotate",
            Mode::Images => "images → pdf",
        }
    }

    fn accepts_images(self) -> bool {
        matches!(self, Mode::Images)
    }

    fn uses_ranges(self) -> bool {
        matches!(self, Mode::Split | Mode::Extract | Mode::Rotate)
    }
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp"
        )
    })
}

enum RowDone {
    One(PdfOutcome),
    Many(Vec<PathBuf>),
}

enum RowStatus {
    Pending,
    Working,
    Done(RowDone),
    Error(String),
}

struct FileEntry {
    path: PathBuf,
    size: u64,
    pages: Option<usize>,
    status: RowStatus,
}

enum GlobalResult {
    Done(PdfOutcome),
    Error(String),
}

pub struct PdfTool {
    files: Vec<FileEntry>,
    mode: Mode,
    rotate_deg: i64,
    ranges_input: Entity<TextInput>,
    ranges_ok: bool,
    ranges_empty: bool,
    working: bool,
    /// Result of the single-output modes (merge, images → pdf).
    global: Option<GlobalResult>,
    task: Option<Task<()>>,
    _subscription: Subscription,
}

impl PdfTool {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let ranges_input = cx.new(|cx| TextInput::new(cx, "pages: 1-3,5,8-  (empty = all)"));
        let subscription = cx.subscribe(&ranges_input, |this: &mut PdfTool, _, event, cx| {
            let TextInputEvent::Edited = event;
            this.revalidate_ranges(cx);
            this.reset_results();
            cx.notify();
        });
        Self {
            files: Vec::new(),
            mode: Mode::Merge,
            rotate_deg: 90,
            ranges_input,
            ranges_ok: true,
            ranges_empty: true,
            working: false,
            global: None,
            task: None,
            _subscription: subscription,
        }
    }

    fn revalidate_ranges(&mut self, cx: &mut Context<Self>) {
        let text = self.ranges_input.read(cx).text().trim().to_string();
        // Syntax check against the largest probed page count (out-of-bounds
        // is re-checked per file at run time).
        let max_pages = self
            .files
            .iter()
            .filter_map(|f| f.pages)
            .max()
            .unwrap_or(9999);
        self.ranges_empty = text.is_empty();
        self.ranges_ok = text.is_empty() || parse_ranges(&text, max_pages).is_ok();
    }

    fn reset_results(&mut self) {
        if self.working {
            return;
        }
        for file in &mut self.files {
            if matches!(file.status, RowStatus::Done(_) | RowStatus::Error(_)) {
                file.status = RowStatus::Pending;
            }
        }
        self.global = None;
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let accept: fn(&Path) -> bool = if self.mode.accepts_images() {
            is_image
        } else {
            is_pdf
        };
        for path in paths {
            if !accept(&path) || self.files.iter().any(|f| f.path == path) {
                continue;
            }
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let probe = (!self.mode.accepts_images()).then(|| path.clone());
            self.files.push(FileEntry {
                path,
                size,
                pages: None,
                status: RowStatus::Pending,
            });
            if let Some(path) = probe {
                cx.spawn(async move |this, cx| {
                    let probe_path = path.clone();
                    let pages = cx
                        .background_spawn(async move { pdf::page_count(&probe_path).ok() })
                        .await;
                    this.update(cx, |tool, cx| {
                        if let Some(entry) = tool.files.iter_mut().find(|f| f.path == path) {
                            entry.pages = pages;
                        }
                        tool.revalidate_ranges(cx);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
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

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.working || mode == self.mode {
            return;
        }
        // PDF modes share the file list; images mode needs different inputs.
        if mode.accepts_images() != self.mode.accepts_images() {
            self.files.clear();
        }
        self.mode = mode;
        self.reset_results();
        self.revalidate_ranges(cx);
        cx.notify();
    }

    fn can_run(&self) -> bool {
        if self.working || self.files.is_empty() {
            return false;
        }
        match self.mode {
            Mode::Merge => self.files.len() >= 2,
            Mode::Extract => self.ranges_ok && !self.ranges_empty,
            Mode::Split | Mode::Rotate => self.ranges_ok,
            Mode::Images => true,
        }
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        if !self.can_run() {
            return;
        }
        let ranges_text = self.ranges_input.read(cx).text().trim().to_string();
        if self.mode == Mode::Extract && ranges_text.is_empty() {
            return;
        }
        self.working = true;
        self.global = None;
        cx.notify();
        match self.mode {
            Mode::Merge | Mode::Images => self.run_single_output(cx),
            _ => self.run_per_file(ranges_text, cx),
        }
    }

    fn run_single_output(&mut self, cx: &mut Context<Self>) {
        let mode = self.mode;
        let inputs: Vec<PathBuf> = self.files.iter().map(|f| f.path.clone()).collect();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let first = &inputs[0];
                    let stem = first
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("output");
                    match mode {
                        Mode::Merge => {
                            let out = first.with_file_name(format!("{stem}-merged.pdf"));
                            pdf::merge(&inputs, &out)
                        }
                        _ => {
                            let out = first.with_file_name(format!("{stem}.pdf"));
                            pdf::images_to_pdf(&inputs, ImagePdfOptions::default(), &out)
                        }
                    }
                })
                .await;
            this.update(cx, |tool, cx| {
                tool.global = Some(match result {
                    Ok(outcome) => {
                        crate::history::push(
                            cx,
                            crate::history::HistoryEntry {
                                tool: "pdf",
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
                        GlobalResult::Done(outcome)
                    }
                    Err(e) => GlobalResult::Error(format!("{e:#}")),
                });
                tool.working = false;
                cx.notify();
            })
            .ok();
        }));
    }

    fn run_per_file(&mut self, ranges_text: String, cx: &mut Context<Self>) {
        let mode = self.mode;
        let deg = self.rotate_deg;
        let jobs: Vec<(usize, PathBuf)> = self
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| !matches!(f.status, RowStatus::Done(_)))
            .map(|(ix, f)| (ix, f.path.clone()))
            .collect();
        if jobs.is_empty() {
            self.working = false;
            cx.notify();
            return;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            for (ix, path) in jobs {
                this.update(cx, |tool, cx| {
                    tool.files[ix].status = RowStatus::Working;
                    cx.notify();
                })
                .ok();
                let ranges_text = ranges_text.clone();
                let result = cx
                    .background_spawn(async move { run_row(mode, &path, deg, &ranges_text) })
                    .await;
                this.update(cx, |tool, cx| {
                    tool.files[ix].status = match result {
                        Ok(done) => {
                            push_row_history(cx, &tool.files[ix].path, &done);
                            RowStatus::Done(done)
                        }
                        Err(e) => RowStatus::Error(format!("{e:#}")),
                    };
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

    fn move_file(&mut self, ix: usize, delta: isize, cx: &mut Context<Self>) {
        if self.working {
            return;
        }
        let target = ix as isize + delta;
        if target < 0 || target as usize >= self.files.len() {
            return;
        }
        self.files.swap(ix, target as usize);
        self.reset_results();
        cx.notify();
    }

    fn render_drop_zone(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let (title, hint) = if self.mode.accepts_images() {
            (
                "drag & drop your images here",
                "or click to browse · png · jpeg · webp · one page per image",
            )
        } else {
            ("drag & drop your PDFs here", "or click to browse · .pdf")
        };
        crate::dropzone::drop_zone(theme, self.files.is_empty(), title, hint)
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
        let meta = match file.pages {
            Some(1) => format!("1 page · {}", format_size(file.size)),
            Some(pages) => format!("{pages} pages · {}", format_size(file.size)),
            None => format_size(file.size),
        };

        let status: gpui::AnyElement = match &file.status {
            RowStatus::Pending => div()
                .text_color(theme.text_faint)
                .child(meta)
                .into_any_element(),
            RowStatus::Working => div()
                .text_color(theme.text_muted)
                .child("working…")
                .into_any_element(),
            RowStatus::Done(RowDone::One(outcome)) => {
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
                            .max_w(px(280.))
                            .truncate()
                            .child(format!("{out_name} · {} pages", outcome.pages)),
                    )
                    .child(div().text_color(theme.text_faint).child("reveal"))
                    .into_any_element()
            }
            RowStatus::Done(RowDone::Many(outputs)) => {
                let reveal = outputs.first().cloned();
                div()
                    .id(("reveal", ix))
                    .flex()
                    .gap(px(Theme::SPACE_SM))
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if let Some(path) = &reveal {
                            cx.reveal_path(path);
                        }
                    }))
                    .child(
                        div()
                            .text_color(theme.success)
                            .child(format!("{} files", outputs.len())),
                    )
                    .child(div().text_color(theme.text_faint).child("reveal"))
                    .into_any_element()
            }
            RowStatus::Error(e) => div()
                .text_color(theme.danger)
                .max_w(px(320.))
                .truncate()
                .child(e.clone())
                .into_any_element(),
        };

        let mut row = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(Theme::SPACE_MD))
            .px(px(Theme::SPACE_MD))
            .py(px(Theme::SPACE_SM))
            .rounded(px(Theme::CONTROL_RADIUS))
            .bg(theme.surface);

        if self.mode == Mode::Merge {
            let order = div()
                .flex()
                .gap(px(2.))
                .child(
                    div()
                        .id(("up", ix))
                        .px(px(Theme::SPACE_XS))
                        .text_color(if ix > 0 {
                            theme.text_muted
                        } else {
                            theme.text_faint
                        })
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.text))
                        .on_click(cx.listener(move |this, _, _, cx| this.move_file(ix, -1, cx)))
                        .child("↑"),
                )
                .child(
                    div()
                        .id(("down", ix))
                        .px(px(Theme::SPACE_XS))
                        .text_color(if ix + 1 < self.files.len() {
                            theme.text_muted
                        } else {
                            theme.text_faint
                        })
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.text))
                        .on_click(cx.listener(move |this, _, _, cx| this.move_file(ix, 1, cx)))
                        .child("↓"),
                );
            row = row.child(order);
        }

        row.child(
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
                    if !this.working {
                        this.files.remove(ix);
                        this.reset_results();
                        cx.notify();
                    }
                }))
                .child("×"),
        )
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);

        let mut mode_chips = div().flex().flex_wrap().gap(px(Theme::SPACE_XS));
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

        let mut controls = div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_MD))
            .child(labeled(theme, "mode", mode_chips.into_any_element()));

        if self.mode == Mode::Rotate {
            let mut deg_chips = div().flex().gap(px(Theme::SPACE_XS));
            for deg in [90i64, 180, 270] {
                let selected = self.rotate_deg == deg;
                deg_chips = deg_chips.child(
                    div()
                        .id(("deg", deg as usize))
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
                            this.rotate_deg = deg;
                            this.reset_results();
                            cx.notify();
                        }))
                        .child(format!("{deg}°")),
                );
            }
            controls = controls.child(labeled(theme, "rotation", deg_chips.into_any_element()));
        }

        if self.mode.uses_ranges() {
            let mut ranges = div()
                .flex()
                .flex_col()
                .gap(px(Theme::SPACE_XS))
                .child(self.ranges_input.clone());
            if !self.ranges_ok {
                ranges = ranges.child(
                    div()
                        .text_size(px(10.))
                        .text_color(theme.danger)
                        .child("can't parse that — try 1-3,5,8-"),
                );
            }
            controls = controls.child(labeled(theme, "pages", ranges.into_any_element()));
        }

        let can_run = self.can_run();
        let n = self.files.len();
        let label = match self.mode {
            Mode::Merge => format!("merge {n} pdfs"),
            Mode::Split => {
                if n == 1 {
                    "split 1 pdf".to_string()
                } else {
                    format!("split {n} pdfs")
                }
            }
            Mode::Extract => "extract pages".to_string(),
            Mode::Rotate => {
                if n == 1 {
                    "rotate 1 pdf".to_string()
                } else {
                    format!("rotate {n} pdfs")
                }
            }
            Mode::Images => {
                if n == 1 {
                    "make a 1-page pdf".to_string()
                } else {
                    format!("make a {n}-page pdf")
                }
            }
        };

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
                        .bg(if can_run { theme.accent } else { theme.surface })
                        .text_color(if can_run {
                            theme.on_accent
                        } else {
                            theme.text_faint
                        })
                        .text_size(px(12.))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.run(cx)))
                        .child(if self.working {
                            "working…".to_string()
                        } else {
                            label
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
                                this.global = None;
                                cx.notify();
                            }
                        }))
                        .child("clear all"),
                ),
        )
    }

    fn render_global_result(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let theme = Theme::of(cx);
        match self.global.as_ref()? {
            GlobalResult::Error(e) => Some(
                div()
                    .px(px(Theme::SPACE_MD))
                    .py(px(Theme::SPACE_SM))
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .bg(theme.surface)
                    .text_size(px(11.))
                    .text_color(theme.danger)
                    .child(e.clone())
                    .into_any_element(),
            ),
            GlobalResult::Done(outcome) => {
                let out_path = outcome.out_path.clone();
                let name = out_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Some(
                    div()
                        .id("global-reveal")
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(Theme::SPACE_MD))
                        .px(px(Theme::SPACE_MD))
                        .py(px(Theme::SPACE_SM))
                        .rounded(px(Theme::CONTROL_RADIUS))
                        .bg(theme.surface)
                        .cursor_pointer()
                        .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&out_path)))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.success)
                                .truncate()
                                .child(format!(
                                    "{name} · {} pages · {}",
                                    outcome.pages,
                                    format_size(outcome.out_size)
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.text_faint)
                                .child("reveal"),
                        )
                        .into_any_element(),
                )
            }
        }
    }
}

/// One split/extract/rotate job for one file; runs on the background executor.
fn run_row(mode: Mode, path: &Path, deg: i64, ranges_text: &str) -> anyhow::Result<RowDone> {
    let total = pdf::page_count(path)?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let parent = path.parent().unwrap_or(Path::new("."));
    match mode {
        Mode::Split => {
            let split_mode = if ranges_text.is_empty() {
                SplitMode::EveryPage
            } else {
                SplitMode::Ranges(parse_ranges(ranges_text, total)?)
            };
            Ok(RowDone::Many(pdf::split(path, split_mode, parent)?))
        }
        Mode::Extract => {
            let ranges = parse_ranges(ranges_text, total)?;
            let out = path.with_file_name(format!("{stem}-extracted.pdf"));
            Ok(RowDone::One(pdf::extract_pages(path, &ranges, &out)?))
        }
        Mode::Rotate => {
            let pages: Option<Vec<usize>> = if ranges_text.is_empty() {
                None
            } else {
                let mut pages = Vec::new();
                for (start, end) in parse_ranges(ranges_text, total)? {
                    pages.extend(start..=end);
                }
                Some(pages)
            };
            let out = path.with_file_name(format!("{stem}-rotated.pdf"));
            Ok(RowDone::One(pdf::rotate(
                path,
                deg,
                pages.as_deref(),
                &out,
            )?))
        }
        Mode::Merge | Mode::Images => unreachable!("handled as single-output modes"),
    }
}

fn push_row_history(cx: &mut gpui::App, input: &Path, done: &RowDone) {
    let in_size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let entry = match done {
        RowDone::One(outcome) => crate::history::HistoryEntry {
            tool: "pdf",
            name: outcome
                .out_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            out_path: outcome.out_path.clone(),
            in_size: outcome.in_size,
            out_size: outcome.out_size,
        },
        RowDone::Many(outputs) => {
            let stem = input
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("split");
            crate::history::HistoryEntry {
                tool: "pdf",
                name: format!("{stem} → {} files", outputs.len()),
                out_path: outputs.first().cloned().unwrap_or_default(),
                in_size,
                out_size: 0,
            }
        }
    };
    crate::history::push(cx, entry);
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

impl Render for PdfTool {
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
                    .child("pdf tools"),
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
            pane = pane.child(rows);
            if let Some(global) = self.render_global_result(cx) {
                pane = pane.child(global);
            }
            pane = pane.child(self.render_controls(cx));
        }

        pane
    }
}
