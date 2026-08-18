//! Embedded icon assets + the gpui [`AssetSource`] serving them.
//! Solar Icons Linear, CC BY 4.0 — "Solar Icons by 480 Design".
//! Usage: `icon(icons::SUN).size(px(15.)).text_color(…)`.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString, Styled as _, Svg, svg};

macro_rules! icon_assets {
    ($(($const_name:ident, $path:literal)),+ $(,)?) => {
        $(pub const $const_name: &str = concat!("icons/", $path, ".svg");)+

        pub struct Assets;

        impl AssetSource for Assets {
            fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
                Ok(match path {
                    $(concat!("icons/", $path, ".svg") => Some(Cow::Borrowed(
                        include_bytes!(concat!("../assets/icons/", $path, ".svg")).as_slice(),
                    )),)+
                    _ => None,
                })
            }

            fn list(&self, path: &str) -> Result<Vec<SharedString>> {
                let all = [$(concat!("icons/", $path, ".svg")),+];
                Ok(all
                    .iter()
                    .filter(|p| p.starts_with(path))
                    .map(|p| SharedString::from(*p))
                    .collect())
            }
        }
    };
}

icon_assets![
    (SIDEBAR_LEFT, "sidebar-left"),
    (SIDEBAR_RIGHT, "sidebar-right"),
    (PLUS, "plus"),
    (CLOSE, "close"),
    (SUN, "sun"),
    (MOON, "moon"),
    (HISTORY, "history"),
];

pub fn icon(path: &'static str) -> Svg {
    svg().path(path).flex_none()
}
