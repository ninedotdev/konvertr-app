//! Root view: titlebar (sidebar toggle · conversion tabs · theme toggle),
//! collapsible sidebar of tools, active tab's tool pane, and the full-window
//! image preview dialog.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Context, Entity, ObjectFit, Subscription, Task, Window, div, img, prelude::*, px};

use crate::audio_tool::AudioTool;
use crate::b64_tool::B64Tool;
use crate::clean_tool::CleanTool;
use crate::color_tool::ColorTool;
use crate::data_tool::DataTool;
use crate::devutils_tool::DevUtilsTool;
use crate::hash_tool::HashTool;
use crate::history::{self, HistoryStore};
use crate::icons::{self, icon};
use crate::icons_tool::IconsTool;
use crate::image_tool::{ImageTool, ImageToolEvent};
use crate::imgkit_tool::ImgKitTool;
use crate::pdf_tool::PdfTool;
use crate::svg_tool::SvgTool;
use crate::textkit_tool::TextKitTool;
use crate::theme::{Appearance, Theme};
use crate::updater::Updater;
use crate::video_tool::VideoTool;
use crate::vstudio_tool::VStudioTool;
use crate::yoinks_tool::YoinksTool;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolId {
    Image,
    ImgKit,
    Video,
    VStudio,
    Audio,
    Pdf,
    Data,
    SvgOptimizer,
    Color,
    Base64,
    Icons,
    DevUtils,
    TextKit,
    Clean,
    Hash,
    Yoinks,
}

impl ToolId {
    fn label(self) -> &'static str {
        match self {
            ToolId::Image => "image converter",
            ToolId::ImgKit => "image kit",
            ToolId::Video => "video converter",
            ToolId::VStudio => "video studio",
            ToolId::Audio => "audio converter",
            ToolId::Pdf => "pdf tools",
            ToolId::Data => "json / csv / yaml",
            ToolId::SvgOptimizer => "svg optimizer",
            ToolId::Color => "color converter",
            ToolId::Base64 => "base64",
            ToolId::Icons => "icon generator",
            ToolId::DevUtils => "dev utils",
            ToolId::TextKit => "text kit",
            ToolId::Clean => "metadata cleaner",
            ToolId::Hash => "checksums",
            ToolId::Yoinks => "loader",
        }
    }
}

const CATEGORIES: &[(&str, &[ToolId])] = &[
    (
        "convert",
        &[
            ToolId::Image,
            ToolId::ImgKit,
            ToolId::Video,
            ToolId::VStudio,
            ToolId::Audio,
            ToolId::Pdf,
            ToolId::Data,
        ],
    ),
    (
        "dev tools",
        &[
            ToolId::SvgOptimizer,
            ToolId::Color,
            ToolId::Base64,
            ToolId::Icons,
            ToolId::DevUtils,
            ToolId::TextKit,
        ],
    ),
    ("privacy", &[ToolId::Clean, ToolId::Hash]),
    ("grab", &[ToolId::Yoinks]),
];

/// A oneshot width tween (200ms ease-out), evaluated manually from render:
/// gpui keys `with_animation` by element-id path, so a remounting wrapper
/// would silently replay it.
#[derive(Clone, Copy)]
struct WidthTween {
    from: f32,
    to: f32,
    started: Instant,
}

impl WidthTween {
    fn new(from: f32, to: f32) -> Self {
        Self {
            from,
            to,
            started: Instant::now(),
        }
    }
}

const TWEEN_SECS: f32 = 0.2;

fn eval_tween(tween: &Option<WidthTween>, target: f32) -> f32 {
    let Some(t) = tween else { return target };
    let raw = t.started.elapsed().as_secs_f32() / TWEEN_SECS;
    if raw >= 1.0 {
        return target;
    }
    let eased = 1.0 - (1.0 - raw).powi(3); // ease-out cubic
    t.from + (t.to - t.from) * eased
}

fn tween_active(tween: &Option<WidthTween>) -> bool {
    tween
        .as_ref()
        .map(|t| t.started.elapsed().as_secs_f32() < TWEEN_SECS)
        .unwrap_or(false)
}

struct Tab {
    active: ToolId,
    image_tool: Entity<ImageTool>,
    imgkit_tool: Entity<ImgKitTool>,
    video_tool: Entity<VideoTool>,
    audio_tool: Entity<AudioTool>,
    data_tool: Entity<DataTool>,
    svg_tool: Entity<SvgTool>,
    color_tool: Entity<ColorTool>,
    b64_tool: Entity<B64Tool>,
    icons_tool: Entity<IconsTool>,
    devutils_tool: Entity<DevUtilsTool>,
    textkit_tool: Entity<TextKitTool>,
    vstudio_tool: Entity<VStudioTool>,
    pdf_tool: Entity<PdfTool>,
    clean_tool: Entity<CleanTool>,
    hash_tool: Entity<HashTool>,
    yoinks_tool: Entity<YoinksTool>,
    _subscriptions: Vec<Subscription>,
}

pub struct Shell {
    tabs: Vec<Tab>,
    active_tab: usize,
    sidebar_open: bool,
    right_open: bool,
    sidebar_tween: Option<WidthTween>,
    right_tween: Option<WidthTween>,
    /// Repaint driver while a tween is mid-flight.
    frame_task: Option<Task<()>>,
    updater: Entity<Updater>,
    preview: Option<PathBuf>,
    _history_observer: Subscription,
}

impl Shell {
    pub const RIGHT_WIDTH: f32 = 264.0;

    pub fn new(cx: &mut Context<Self>) -> Self {
        let tab = Self::make_tab(cx);
        let history_observer =
            cx.observe_global::<HistoryStore>(|_this: &mut Shell, cx| cx.notify());
        Self {
            tabs: vec![tab],
            active_tab: 0,
            sidebar_open: true,
            right_open: false,
            sidebar_tween: None,
            right_tween: None,
            frame_task: None,
            updater: cx.new(Updater::new),
            preview: None,
            _history_observer: history_observer,
        }
    }

    fn sidebar_target(&self) -> f32 {
        if self.sidebar_open {
            Theme::SIDEBAR_WIDTH
        } else {
            0.0
        }
    }

    fn right_target(&self) -> f32 {
        if self.right_open {
            Self::RIGHT_WIDTH
        } else {
            0.0
        }
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        let from = eval_tween(&self.sidebar_tween, self.sidebar_target());
        self.sidebar_open = !self.sidebar_open;
        self.sidebar_tween = Some(WidthTween::new(from, self.sidebar_target()));
        cx.notify();
    }

    fn toggle_right(&mut self, cx: &mut Context<Self>) {
        let from = eval_tween(&self.right_tween, self.right_target());
        self.right_open = !self.right_open;
        self.right_tween = Some(WidthTween::new(from, self.right_target()));
        cx.notify();
    }

    fn make_tab(cx: &mut Context<Self>) -> Tab {
        let image_tool = cx.new(ImageTool::new);
        let sub = cx.subscribe(&image_tool, |this: &mut Shell, _, event, cx| {
            let ImageToolEvent::Preview(path) = event;
            this.preview = Some(path.clone());
            cx.notify();
        });
        Tab {
            active: ToolId::Image,
            image_tool,
            imgkit_tool: cx.new(ImgKitTool::new),
            video_tool: cx.new(VideoTool::new),
            audio_tool: cx.new(AudioTool::new),
            data_tool: cx.new(DataTool::new),
            svg_tool: cx.new(SvgTool::new),
            color_tool: cx.new(ColorTool::new),
            b64_tool: cx.new(B64Tool::new),
            icons_tool: cx.new(IconsTool::new),
            devutils_tool: cx.new(DevUtilsTool::new),
            textkit_tool: cx.new(TextKitTool::new),
            vstudio_tool: cx.new(VStudioTool::new),
            pdf_tool: cx.new(PdfTool::new),
            clean_tool: cx.new(CleanTool::new),
            hash_tool: cx.new(HashTool::new),
            yoinks_tool: cx.new(YoinksTool::new),
            _subscriptions: vec![sub],
        }
    }

    fn add_tab(&mut self, cx: &mut Context<Self>) {
        let tab = Self::make_tab(cx);
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        cx.notify();
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.tabs.len() <= 1 {
            return;
        }
        self.tabs.remove(ix);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        cx.notify();
    }

    fn select_tool(&mut self, tool: ToolId, cx: &mut Context<Self>) {
        self.tabs[self.active_tab].active = tool;
        cx.notify();
    }

    /// The ONE top-left control cluster (comet pattern): pinned at the
    /// window's top-left in an overlay ABOVE the sidebar and headers, so the
    /// toggle never moves or remounts when the sidebar collapses.
    fn render_titlebar_cluster(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .absolute()
            .top_0()
            .left_0()
            .h(px(Theme::TITLEBAR_HEIGHT))
            .flex()
            .items_center()
            .pl(px(78.)) // clear the macOS traffic lights
            .child(
                titlebar_button(theme, "toggle-sidebar", icons::SIDEBAR_LEFT)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
            )
    }

    /// Main-column titlebar: tab strip + "+", theme toggle at the right. The
    /// bottom hairline spans only this column — the sidebar separator runs
    /// full height to the very top.
    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let mut bar = div()
            .flex()
            .flex_none()
            .items_center()
            .w_full()
            .h(px(Theme::TITLEBAR_HEIGHT))
            .pl(if self.sidebar_open {
                px(Theme::SPACE_MD)
            } else {
                px(78. + 28. + Theme::SPACE_SM) // traffic lights + cluster
            })
            .pr(px(Theme::SPACE_MD))
            .gap(px(Theme::SPACE_SM));

        for ix in 0..self.tabs.len() {
            let active = ix == self.active_tab;
            let mut tab = div()
                .id(("tab", ix))
                .flex()
                .items_center()
                .gap(px(Theme::SPACE_XS))
                .px(px(Theme::SPACE_SM))
                .py(px(3.))
                .rounded(px(Theme::CONTROL_RADIUS))
                .text_size(px(11.))
                .cursor_pointer()
                .text_color(if active { theme.text } else { theme.text_muted })
                .hover(|s| s.bg(theme.surface_hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.active_tab = ix;
                    cx.notify();
                }))
                .child(format!("conv {}", ix + 1));
            if active {
                tab = tab.bg(theme.surface_hover);
            }
            if self.tabs.len() > 1 {
                tab = tab.child(
                    div()
                        .id(("tab-close", ix))
                        .px(px(2.))
                        .text_color(theme.text_faint)
                        .hover(|s| s.text_color(theme.danger))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_tab(ix, cx);
                        }))
                        .child(icon(icons::CLOSE).size(px(10.))),
                );
            }
            bar = bar.child(tab);
        }

        bar.child(
            titlebar_button(theme, "add-tab", icons::PLUS)
                .on_click(cx.listener(|this, _, _, cx| this.add_tab(cx))),
        )
        .child(div().flex_1()) // drag region
        .child(self.updater.clone())
        .child(
            titlebar_button(
                theme,
                "toggle-theme",
                match theme.appearance {
                    Appearance::Dark => icons::SUN,
                    Appearance::Light => icons::MOON,
                },
            )
            .on_click(cx.listener(|_, _, _, cx| Theme::toggle(cx))),
        )
        .child(
            titlebar_button(theme, "toggle-right", icons::SIDEBAR_RIGHT)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_right(cx))),
        )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let current = self.tabs[self.active_tab].active;
        let mut sidebar = div()
            .flex()
            .flex_col()
            .w(px(Theme::SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            // Same soft lift as the right panel — pure glass reads black here.
            .when(matches!(theme.appearance, Appearance::Dark), |d| {
                d.bg(gpui::hsla(0., 0., 1., 0.035))
            })
            .px(px(Theme::SPACE_SM))
            .gap(px(Theme::SPACE_XS))
            // top strip: traffic lights + the overlay cluster live here
            .child(div().flex_none().h(px(Theme::TITLEBAR_HEIGHT)))
            .child(
                div()
                    .px(px(Theme::SPACE_SM))
                    .pb(px(Theme::SPACE_MD))
                    .text_size(px(15.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme.text)
                    .child("KONVERTR"),
            );

        for (ix, (category, tools)) in CATEGORIES.iter().enumerate() {
            sidebar = sidebar.child(
                div()
                    .px(px(Theme::SPACE_SM))
                    .pt(if ix == 0 { px(0.) } else { px(Theme::SPACE_MD) })
                    .pb(px(Theme::SPACE_XS))
                    .text_size(px(10.))
                    .text_color(theme.text_faint)
                    .child(category.to_uppercase()),
            );
            for tool in tools.iter().copied() {
                let active = current == tool;
                let mut row = div()
                    .id(tool.label())
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(Theme::SPACE_SM))
                    .py(px(5.))
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .text_color(if active { theme.text } else { theme.text_muted })
                    .hover(|s| s.bg(theme.surface_hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_tool(tool, cx)))
                    .child(tool.label());
                if active {
                    row = row.bg(theme.surface_hover);
                }
                sidebar = sidebar.child(row);
            }
        }

        sidebar.child(div().flex_1()).child(
            div()
                .px(px(Theme::SPACE_SM))
                .pb(px(Theme::SPACE_MD))
                .text_size(px(9.))
                .text_color(theme.text_faint)
                .child("100% local. your files never leave."),
        )
    }

    /// Right sidebar: session history + the counters every ad-riddled
    /// converter site earned us.
    fn render_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let (stats, rows) = {
            let store = cx.global::<HistoryStore>();
            let rows: Vec<(usize, &'static str, String, PathBuf, u64, u64)> = store
                .entries
                .iter()
                .enumerate()
                .rev()
                .take(100)
                .map(|(ix, e)| {
                    (
                        ix,
                        e.tool,
                        e.name.clone(),
                        e.out_path.clone(),
                        e.in_size,
                        e.out_size,
                    )
                })
                .collect();
            (store.stats(), rows)
        };

        let stat_row = |label: &'static str, value: String| {
            div()
                .flex()
                .justify_between()
                .text_size(px(11.))
                .child(div().text_color(theme.text_faint).child(label))
                .child(div().text_color(theme.text).child(value))
        };

        let mut panel = div()
            .flex()
            .flex_col()
            .w(px(Self::RIGHT_WIDTH))
            .flex_none()
            .h_full()
            // Dark's glass reads near-black on this edge; a soft white wash
            // lifts it to match the left sidebar.
            .when(matches!(theme.appearance, Appearance::Dark), |d| {
                d.bg(gpui::hsla(0., 0., 1., 0.035))
            })
            .px(px(Theme::SPACE_MD))
            .gap(px(Theme::SPACE_SM))
            .child(div().flex_none().h(px(Theme::TITLEBAR_HEIGHT)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(Theme::SPACE_SM))
                    .child(
                        icon(icons::HISTORY)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.text)
                            .child("history"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(Theme::SPACE_XS))
                    .p(px(Theme::SPACE_MD))
                    .rounded(px(Theme::PANEL_RADIUS))
                    .bg(theme.surface)
                    .child(stat_row("files konverted", stats.conversions.to_string()))
                    .child(stat_row(
                        "bytes shaved off",
                        history::human_bytes(stats.bytes_saved),
                    ))
                    .child(stat_row(
                        "ad-time dodged",
                        format!("~{}", history::human_secs(stats.seconds_dodged)),
                    ))
                    .child(stat_row("bytes uploaded", "0. always 0.".to_string())),
            );

        if rows.is_empty() {
            panel = panel.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.))
                    .text_color(theme.text_faint)
                    .child("nothing konverted yet"),
            );
        } else {
            let mut list = div()
                .id("history-list")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.))
                .gap(px(Theme::SPACE_XS))
                .overflow_y_scroll();
            for (ix, tool, name, out_path, in_size, out_size) in rows {
                let delta = if in_size > out_size && in_size > 0 {
                    format!(
                        "-{:.0}%",
                        100.0 - (out_size as f64 / in_size as f64) * 100.0
                    )
                } else {
                    history::human_bytes(out_size)
                };
                list = list.child(
                    div()
                        .id(("hist", ix))
                        .flex()
                        .items_center()
                        .gap(px(Theme::SPACE_SM))
                        .px(px(Theme::SPACE_SM))
                        .py(px(5.))
                        .rounded(px(Theme::CONTROL_RADIUS))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.surface_hover))
                        .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&out_path)))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(9.))
                                .text_color(theme.text_faint)
                                .child(tool),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_size(px(11.))
                                .text_color(theme.text)
                                .child(name),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(theme.success)
                                .child(delta),
                        ),
                );
            }
            panel = panel.child(list);
        }

        panel.child(
            div()
                .pb(px(Theme::SPACE_MD))
                .text_size(px(9.))
                .text_color(theme.text_faint)
                .child(history::quip(&stats)),
        )
    }

    fn render_preview(&self, path: &PathBuf, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        div()
            .id("preview-overlay")
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(Theme::SPACE_MD))
            .bg(gpui::hsla(0., 0., 0., 0.55))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.preview = None;
                cx.notify();
            }))
            .child(
                div()
                    .max_w(px(920.))
                    .max_h(px(620.))
                    .p(px(Theme::SPACE_SM))
                    .rounded(px(Theme::PANEL_RADIUS))
                    .bg(theme.glass)
                    .border_1()
                    .border_color(theme.border_strong)
                    .child(
                        img(Arc::<Path>::from(path.as_path()))
                            .max_w(px(900.))
                            .max_h(px(560.))
                            .object_fit(ObjectFit::Contain),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(gpui::hsla(0., 0., 0.98, 0.9))
                    .child(format!(
                        "{name} · {:.1} KB · click to close",
                        size as f64 / 1024.0
                    )),
            )
    }
}

fn titlebar_button(
    theme: &Theme,
    id: &'static str,
    icon_path: &'static str,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded(px(Theme::CONTROL_RADIUS))
        .cursor_pointer()
        .hover(|s| s.bg(theme.surface_hover))
        .child(icon(icon_path).size(px(15.)).text_color(theme.text_muted))
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = &self.tabs[self.active_tab];
        let main: gpui::AnyElement = match tab.active {
            ToolId::Image => tab.image_tool.clone().into_any_element(),
            ToolId::ImgKit => tab.imgkit_tool.clone().into_any_element(),
            ToolId::Video => tab.video_tool.clone().into_any_element(),
            ToolId::Audio => tab.audio_tool.clone().into_any_element(),
            ToolId::Data => tab.data_tool.clone().into_any_element(),
            ToolId::SvgOptimizer => tab.svg_tool.clone().into_any_element(),
            ToolId::Color => tab.color_tool.clone().into_any_element(),
            ToolId::Base64 => tab.b64_tool.clone().into_any_element(),
            ToolId::Icons => tab.icons_tool.clone().into_any_element(),
            ToolId::DevUtils => tab.devutils_tool.clone().into_any_element(),
            ToolId::TextKit => tab.textkit_tool.clone().into_any_element(),
            ToolId::VStudio => tab.vstudio_tool.clone().into_any_element(),
            ToolId::Pdf => tab.pdf_tool.clone().into_any_element(),
            ToolId::Clean => tab.clean_tool.clone().into_any_element(),
            ToolId::Hash => tab.hash_tool.clone().into_any_element(),
            ToolId::Yoinks => tab.yoinks_tool.clone().into_any_element(),
        };
        let preview = self.preview.clone();

        // Manual width tweens (comet pattern); keep frames coming mid-flight.
        let sidebar_w = eval_tween(&self.sidebar_tween, self.sidebar_target());
        let right_w = eval_tween(&self.right_tween, self.right_target());
        if (tween_active(&self.sidebar_tween) || tween_active(&self.right_tween))
            && self.frame_task.is_none()
        {
            self.frame_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                this.update(cx, |shell, cx| {
                    shell.frame_task = None;
                    cx.notify();
                })
                .ok();
            }));
        }

        let border = Theme::of(cx).border;
        let sidebar = div()
            .h_full()
            .flex_none()
            .overflow_hidden()
            .w(px(sidebar_w))
            .when(sidebar_w > 0.5, |d| d.border_r_1().border_color(border))
            .child(self.render_sidebar(cx));
        let right = div()
            .h_full()
            .flex_none()
            .overflow_hidden()
            .w(px(right_w))
            .when(right_w > 0.5, |d| d.border_l_1().border_color(border))
            .child(self.render_history(cx));

        let theme = Theme::of(cx);
        div()
            .id("shell-root")
            .relative()
            .flex()
            .flex_row()
            .size_full()
            .bg(theme.glass)
            .text_color(theme.text)
            .font_family(theme.font_ui.clone())
            .text_size(px(12.))
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .child(self.render_titlebar(cx))
                    .child(div().flex().flex_col().flex_1().min_h(px(0.)).child(main)),
            )
            .child(right)
            .child(self.render_titlebar_cluster(cx))
            .when_some(preview, |d, path| d.child(self.render_preview(&path, cx)))
    }
}
