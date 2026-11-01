use std::borrow::Cow;

use gpui::{
    AnyElement, AssetSource, FontWeight, IntoElement, ParentElement, Result, Rgba, SharedString,
    Styled, Svg, div, img, px, relative, svg,
};

include!(concat!(env!("OUT_DIR"), "/icons.rs"));

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let (file, options) = path.split_once('?').unwrap_or((path, ""));
        let Some((_, bytes)) = ASSETS.iter().find(|(name, _)| *name == file) else {
            return Ok(None);
        };
        if options.is_empty() {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        for option in options.split('&') {
            text = match option.split_once('=') {
                Some(("stroke", width)) => {
                    text.replace("stroke-width=\"2\"", &format!("stroke-width=\"{width}\""))
                }
                None if option == "fill" => text.replace("fill=\"none\"", "fill=\"currentColor\""),
                _ => text,
            };
        }
        Ok(Some(Cow::Owned(text.into_bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ASSETS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

pub fn icon(name: Lucide, size: f32, color: Rgba) -> Svg {
    svg()
        .path(name.path())
        .size(px(size))
        .flex_none()
        .text_color(color)
}

#[allow(dead_code)]
pub fn icon_styled(name: Lucide, size: f32, color: Rgba, stroke: f32, filled: bool) -> Svg {
    let fill = if filled { "&fill" } else { "" };
    svg()
        .path(format!("{}?stroke={stroke}{fill}", name.path()))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

#[allow(dead_code)]
pub fn agent_mark(agent: Option<&str>, label: &str, size: f32, color: Rgba) -> AnyElement {
    match agent {
        None => icon(Lucide::Terminal, size, color).into_any_element(),
        Some(id) => match logo(id) {
            Some(Logo::Color(path)) => img(path).size(px(size)).flex_none().into_any_element(),
            Some(Logo::Mono(path)) => svg()
                .path(path)
                .size(px(size))
                .flex_none()
                .text_color(color)
                .into_any_element(),
            None => div()
                .size(px(size))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(color)
                .rounded(px(3.0))
                .text_color(color)
                .text_size(px(9.0))
                .line_height(relative(1.0))
                .font_weight(FontWeight::BOLD)
                .child(
                    label
                        .chars()
                        .next()
                        .map(|c| c.to_uppercase().to_string())
                        .unwrap_or_default(),
                )
                .into_any_element(),
        },
    }
}
