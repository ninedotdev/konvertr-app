//! Dark glass theme, installed as a gpui Global before anything paints.
//! Layout tokens are plain numbers; colors are paint (comet convention).

use gpui::{App, Global, Hsla, SharedString, WindowBackgroundAppearance, rgb, rgba};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

pub struct Theme {
    pub appearance: Appearance,
    /// Frost tint painted over the blurred desktop (translucent → glass).
    pub glass: Hsla,
    /// Raised panel tone (cards, rows) — translucent over glass.
    pub surface: Hsla,
    pub surface_hover: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub accent: Hsla,
    pub on_accent: Hsla,
    pub danger: Hsla,
    pub success: Hsla,
    pub font_ui: SharedString,
}

impl Global for Theme {}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex: u32) -> Hsla {
    rgba(hex).into()
}

impl Theme {
    pub const TITLEBAR_HEIGHT: f32 = 40.0;
    pub const SIDEBAR_WIDTH: f32 = 216.0;
    pub const PANEL_RADIUS: f32 = 10.0;
    pub const CONTROL_RADIUS: f32 = 6.0;
    pub const SPACE_XS: f32 = 4.0;
    pub const SPACE_SM: f32 = 8.0;
    pub const SPACE_MD: f32 = 12.0;
    pub const SPACE_LG: f32 = 16.0;

    pub fn dark() -> Self {
        Self {
            appearance: Appearance::Dark,
            glass: ca(0x0a0a0aa8),
            surface: ca(0xffffff0a),
            surface_hover: ca(0xffffff14),
            border: ca(0xffffff1a),
            border_strong: ca(0xffffff33),
            text: c(0xfafafa),
            text_muted: c(0xa3a3a3),
            text_faint: c(0x616161),
            accent: c(0xfafafa),
            on_accent: c(0x0a0a0a),
            danger: c(0xf87171),
            success: c(0x4ade80),
            font_ui: "Menlo".into(),
        }
    }

    /// Designed light frost, not an inverted dark (comet convention): near-white
    /// translucent glass over the blurred desktop, ink washes for surfaces.
    pub fn light() -> Self {
        Self {
            appearance: Appearance::Light,
            glass: ca(0xf5f5f5cc),
            surface: ca(0x0000000d),
            surface_hover: ca(0x00000017),
            border: ca(0x00000021),
            border_strong: ca(0x00000040),
            text: c(0x171717),
            text_muted: c(0x525252),
            text_faint: c(0x9e9e9e),
            accent: c(0x171717),
            on_accent: c(0xfafafa),
            danger: c(0xdc2626),
            success: c(0x15803d),
            font_ui: "Menlo".into(),
        }
    }

    pub fn install(cx: &mut App) {
        cx.set_global(Self::dark());
    }

    /// Swap appearance. Colors are read at paint time, so refresh windows
    /// rather than notifying any one view.
    pub fn toggle(cx: &mut App) {
        let next = match Self::of(cx).appearance {
            Appearance::Dark => Self::light(),
            Appearance::Light => Self::dark(),
        };
        cx.set_global(next);
        cx.refresh_windows();
    }

    pub fn of(cx: &App) -> &Theme {
        cx.global::<Theme>()
    }

    pub fn window_background_appearance(&self) -> WindowBackgroundAppearance {
        if self.glass.a < 1.0 {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Opaque
        }
    }
}
