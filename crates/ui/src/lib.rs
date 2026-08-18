//! konvrt-ui: the entire gpui app. The binary in apps/konvrt just calls
//! [`run_app`].

mod audio_tool;
mod b64_tool;
pub mod banner;
mod clean_tool;
mod color_tool;
mod data_tool;
mod devutils_tool;
mod dropzone;
mod hash_tool;
mod history;
mod icons;
mod icons_tool;
mod image_tool;
mod imgkit_tool;
mod pdf_tool;
mod shell;
mod svg_tool;
mod text_input;
mod textkit_tool;
mod theme;
mod updater;
mod video_tool;
mod vstudio_tool;
mod yoinks_tool;

use gpui::{
    App, AppContext as _, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, actions, point, px, size,
};

actions!(konvrt, [Quit]);

pub fn run_app() {
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(|cx: &mut App| {
            // Theme goes in before anything paints, or frame 1 flashes unstyled.
            theme::Theme::install(cx);
            history::init(cx);

            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            text_input::init(cx);

            open_main_window(cx);

            // Menus after the window: gpui snapshots the keymap when they're set.
            cx.set_menus(vec![Menu {
                name: "konvrt".into(),
                items: vec![MenuItem::action("Quit konvrt", Quit)],
                disabled: false,
            }]);
            cx.activate(true);
        });
}

fn open_main_window(cx: &mut App) {
    let theme = theme::Theme::of(cx);
    let window_background = theme.window_background_appearance();
    let bounds = Bounds::centered(None, size(px(1100.), px(760.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(760.), px(560.))),
            titlebar: Some(TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(14.))),
            }),
            window_background,
            // We own the titlebar: drag and double-click-to-zoom are wired to
            // the strip in `shell`, so buttons up there can't zoom the window.
            app_owns_titlebar_drag: true,
            app_id: Some("konvrt".into()),
            ..Default::default()
        },
        |_window, cx| cx.new(shell::Shell::new),
    )
    .expect("failed to open window");
}
