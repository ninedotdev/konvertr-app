//! Icon set generator: drop one source image, pick a set (favicon / tauri /
//! electron) and an output folder, generate every artifact on the background
//! executor.

use std::path::{Path, PathBuf};

use gpui::{Context, ExternalPaths, PathPromptOptions, Task, Window, div, prelude::*, px};
use konvrt_core::icons::{GeneratedSet, IconSet};

use crate::theme::Theme;

enum GenState {
    Idle,
    Generating,
    Done(GeneratedSet),
    Error(String),
}

pub struct IconsTool {
    source: Option<PathBuf>,
    set: IconSet,
    out_dir: Option<PathBuf>,
    state: GenState,
    task: Option<Task<()>>,
}

impl IconsTool {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            source: None,
            set: IconSet::Favicon,
            out_dir: None,
            state: GenState::Idle,
            task: None,
        }
    }

    fn generating(&self) -> bool {
        matches!(self.state, GenState::Generating)
    }

    fn set_source(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.generating() {
            return;
        }
        if let Some(path) = paths
            .into_iter()
            .find(|p| konvrt_core::is_supported_input(p))
        {
            self.source = Some(path);
            self.state = GenState::Idle;
            cx.notify();
        }
    }

    fn browse_source(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |tool, cx| tool.set_source(paths, cx)).ok();
            }
        })
        .detach();
    }

    fn browse_out_dir(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update(cx, |tool, cx| {
                    if let Some(dir) = paths.into_iter().next() {
                        tool.out_dir = Some(dir);
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn generate(&mut self, cx: &mut Context<Self>) {
        if self.generating() {
            return;
        }
        let (Some(source), Some(out_dir)) = (self.source.clone(), self.out_dir.clone()) else {
            return;
        };
        let set = self.set;
        let in_size = std::fs::metadata(&source).map(|m| m.len()).unwrap_or(0);
        self.state = GenState::Generating;
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(
                    async move { konvrt_core::icons::generate(&source, set, &out_dir) },
                )
                .await;
            this.update(cx, |tool, cx| {
                tool.state = match result {
                    Ok(generated) => {
                        let set_dir = generated
                            .files
                            .first()
                            .and_then(|f| f.parent())
                            .map(std::path::Path::to_path_buf)
                            .unwrap_or_default();
                        crate::history::push(
                            cx,
                            crate::history::HistoryEntry {
                                tool: "icons",
                                name: format!(
                                    "{} set ({} files)",
                                    set.label(),
                                    generated.files.len()
                                ),
                                out_path: set_dir,
                                in_size,
                                out_size: 0,
                            },
                        );
                        GenState::Done(generated)
                    }
                    Err(e) => GenState::Error(format!("{e:#}")),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_icon_set(&mut self, set: IconSet, cx: &mut Context<Self>) {
        if self.generating() {
            return;
        }
        self.set = set;
        if matches!(self.state, GenState::Done(_) | GenState::Error(_)) {
            self.state = GenState::Idle;
        }
        cx.notify();
    }

    fn render_drop_zone(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        crate::dropzone::drop_zone(
            theme,
            self.source.is_none(),
            "drag & drop your source image here",
            "or click to browse · one square image, ideally 1024×1024 png",
        )
        .on_click(cx.listener(|this, _, _, cx| this.browse_source(cx)))
        .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
            this.set_source(paths.paths().to_vec(), cx);
        }))
    }

    fn render_source_row(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let theme = Theme::of(cx);
        let source = self.source.as_ref()?;
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let size = std::fs::metadata(source).map(|m| m.len()).unwrap_or(0);
        Some(
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
                        .child(format_size(size)),
                )
                .child(
                    div()
                        .id("remove-source")
                        .px(px(Theme::SPACE_XS))
                        .text_color(theme.text_faint)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.danger))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.generating() {
                                this.source = None;
                                this.state = GenState::Idle;
                                cx.notify();
                            }
                        }))
                        .child("×"),
                ),
        )
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);

        let mut set_chips = div().flex().flex_wrap().gap(px(Theme::SPACE_XS));
        for set in IconSet::ALL {
            let selected = self.set == set;
            set_chips = set_chips.child(
                div()
                    .id(set.label())
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
                    .on_click(cx.listener(move |this, _, _, cx| this.set_icon_set(set, cx)))
                    .child(set.label()),
            );
        }

        let out_dir_label = self
            .out_dir
            .as_ref()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_else(|| "choose output folder…".to_string());
        let out_dir_row = div()
            .id("out-dir")
            .flex()
            .max_w(px(420.))
            .px(px(Theme::SPACE_SM))
            .py(px(3.))
            .rounded(px(Theme::CONTROL_RADIUS))
            .border_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(if self.out_dir.is_some() {
                theme.text
            } else {
                theme.text_muted
            })
            .cursor_pointer()
            .hover(|s| s.bg(theme.surface_hover))
            .on_click(cx.listener(|this, _, _, cx| this.browse_out_dir(cx)))
            .child(div().min_w(px(0.)).truncate().child(out_dir_label));

        let can_generate = self.source.is_some() && self.out_dir.is_some() && !self.generating();
        div()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_MD))
            .child(labeled(theme, "icon set", set_chips.into_any_element()))
            .child(labeled(
                theme,
                "output folder",
                out_dir_row.into_any_element(),
            ))
            .child(
                div().flex().child(
                    div()
                        .id("generate")
                        .px(px(Theme::SPACE_LG))
                        .py(px(6.))
                        .rounded(px(Theme::CONTROL_RADIUS))
                        .bg(if can_generate {
                            theme.accent
                        } else {
                            theme.surface
                        })
                        .text_color(if can_generate {
                            theme.on_accent
                        } else {
                            theme.text_faint
                        })
                        .text_size(px(12.))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.generate(cx)))
                        .child(if self.generating() {
                            "generating…".to_string()
                        } else {
                            format!("generate {} set", self.set.label())
                        }),
                ),
            )
    }

    fn render_result(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let theme = Theme::of(cx);
        match &self.state {
            GenState::Idle | GenState::Generating => None,
            GenState::Error(e) => Some(
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
            GenState::Done(generated) => {
                // Every set writes its first file at the set folder's root.
                let set_dir = generated
                    .files
                    .first()
                    .and_then(|f| f.parent())
                    .map(Path::to_path_buf)?;

                let header = div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(Theme::SPACE_MD))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.success)
                            .child(format!(
                                "{} files → {}",
                                generated.files.len(),
                                set_dir.to_string_lossy()
                            )),
                    )
                    .child({
                        let reveal_dir = set_dir.clone();
                        div()
                            .id("reveal-set")
                            .px(px(Theme::SPACE_SM))
                            .py(px(3.))
                            .rounded(px(Theme::CONTROL_RADIUS))
                            .border_1()
                            .border_color(theme.border)
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.surface_hover))
                            .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&reveal_dir)))
                            .child("reveal")
                    });

                // Compact grouping: one line per subfolder ("" = set root),
                // file names joined with " · ".
                let mut groups: Vec<(String, Vec<String>)> = Vec::new();
                for file in &generated.files {
                    let rel = file.strip_prefix(&set_dir).unwrap_or(file);
                    let (group, name) = match rel.parent() {
                        Some(p) if !p.as_os_str().is_empty() => (
                            p.to_string_lossy().into_owned(),
                            rel.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                        ),
                        _ => (String::new(), rel.to_string_lossy().into_owned()),
                    };
                    match groups.iter_mut().find(|(g, _)| *g == group) {
                        Some((_, names)) => names.push(name),
                        None => groups.push((group, vec![name])),
                    }
                }

                let mut list = div()
                    .id("result-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap(px(Theme::SPACE_XS))
                    .overflow_y_scroll();
                for (group, names) in groups {
                    let mut row = div()
                        .px(px(Theme::SPACE_MD))
                        .py(px(Theme::SPACE_SM))
                        .rounded(px(Theme::CONTROL_RADIUS))
                        .bg(theme.surface)
                        .flex()
                        .flex_col()
                        .gap(px(2.));
                    if !group.is_empty() {
                        row = row.child(
                            div()
                                .text_size(px(10.))
                                .text_color(theme.text_faint)
                                .child(group),
                        );
                    }
                    list = list.child(
                        row.child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.text_muted)
                                .child(names.join(" · ")),
                        ),
                    );
                }

                Some(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.))
                        .gap(px(Theme::SPACE_SM))
                        .child(header)
                        .child(list)
                        .into_any_element(),
                )
            }
        }
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

impl Render for IconsTool {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let has_source = self.source.is_some();

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
                    .child("icon set generator"),
            )
            .child(self.render_drop_zone(cx));

        if has_source {
            if let Some(row) = self.render_source_row(cx) {
                pane = pane.child(row);
            }
            pane = pane.child(self.render_controls(cx));
            if let Some(result) = self.render_result(cx) {
                pane = pane.child(result);
            }
        }

        pane
    }
}
