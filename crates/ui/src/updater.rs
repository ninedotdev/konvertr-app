//! Update pill in the titlebar: checks on launch, downloads on click, then
//! swaps the bundle and relaunches.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use gpui::{Context, Task, div, prelude::*, px};
use konvrt_core::update::{self, Available};

use crate::theme::Theme;

#[derive(Clone)]
pub enum State {
    /// Up to date, still checking, or not installed as an app bundle.
    Idle,
    Available(Available),
    Downloading,
    Ready,
    Failed(String),
}

pub struct Updater {
    pub state: State,
    progress: Arc<AtomicU8>,
    bundle: Option<PathBuf>,
    task: Option<Task<()>>,
    poll: Option<Task<()>>,
}

impl Updater {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut updater = Self {
            state: State::Idle,
            progress: Arc::new(AtomicU8::new(0)),
            bundle: update::current_bundle(),
            task: None,
            poll: None,
        };
        if updater.bundle.is_some() {
            updater.check(cx);
        }
        updater
    }

    /// Ask the manifest what's out there. Failures stay silent: a missing
    /// manifest must not nag someone who just wants to convert a file.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        self.task = Some(cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async { update::check() }).await;
            this.update(cx, |updater, cx| {
                if let Ok(Some(available)) = found {
                    updater.state = State::Available(available);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn install(&mut self, available: Available, cx: &mut Context<Self>) {
        let Some(bundle) = self.bundle.clone() else {
            return;
        };
        self.progress.store(0, Ordering::Relaxed);
        self.state = State::Downloading;
        cx.notify();

        // Repaint while the background thread moves the byte counter.
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let downloading = this
                    .update(cx, |updater, cx| {
                        cx.notify();
                        matches!(updater.state, State::Downloading)
                    })
                    .unwrap_or(false);
                if !downloading {
                    break;
                }
            }
        }));

        let progress = self.progress.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let installed = cx
                .background_spawn(async move {
                    let staged = update::stage(&available.artifact, &progress)?;
                    update::apply(&staged, &bundle)?;
                    anyhow::Ok(bundle)
                })
                .await;
            this.update(cx, |updater, cx| {
                match installed {
                    Ok(bundle) => {
                        updater.state = State::Ready;
                        updater.bundle = Some(bundle);
                    }
                    Err(e) => updater.state = State::Failed(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn restart(&mut self, cx: &mut Context<Self>) {
        if let Some(bundle) = &self.bundle {
            update::relaunch_after_exit(bundle);
        }
        cx.quit();
    }
}

impl Render for Updater {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let pill = |label: String| {
            div()
                .flex()
                .items_center()
                .px(px(Theme::SPACE_SM))
                .py(px(2.))
                .rounded(px(Theme::CONTROL_RADIUS))
                .text_size(px(10.))
                .child(label)
        };

        match self.state.clone() {
            State::Idle => div().into_any_element(),
            State::Available(available) => pill(format!("update to {}", available.version))
                .id("update-available")
                .bg(theme.surface_hover)
                .text_color(theme.text)
                .cursor_pointer()
                .hover(|s| s.bg(theme.border_strong))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.install(available.clone(), cx);
                }))
                .into_any_element(),
            State::Downloading => pill(format!(
                "updating… {}%",
                self.progress.load(Ordering::Relaxed)
            ))
            .text_color(theme.text_muted)
            .into_any_element(),
            State::Ready => pill("restart to update".to_string())
                .id("update-ready")
                .bg(theme.accent)
                .text_color(theme.on_accent)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.restart(cx)))
                .into_any_element(),
            // The reason lives in the tooltip-ish title; the pill stays small.
            State::Failed(error) => pill("update failed — retry".to_string())
                .id("update-failed")
                .text_color(theme.danger)
                .cursor_pointer()
                .hover(|s| s.text_color(theme.text))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state = State::Idle;
                    this.check(cx);
                }))
                .tooltip(move |_, cx| {
                    let error = error.clone();
                    cx.new(|_| ErrorTooltip(error)).into()
                })
                .into_any_element(),
        }
    }
}

struct ErrorTooltip(String);

impl Render for ErrorTooltip {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .max_w(px(320.))
            .px(px(Theme::SPACE_SM))
            .py(px(Theme::SPACE_XS))
            .rounded(px(Theme::CONTROL_RADIUS))
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border)
            .text_size(px(10.))
            .text_color(theme.text_muted)
            .child(self.0.clone())
    }
}
