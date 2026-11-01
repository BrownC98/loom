use std::borrow::Cow;

use api::settings::TerminalFont;
use gpui::{App, Font, FontFallbacks, font};

pub const UI_FAMILY: &str = "Pretendard";
pub const TERMINAL_FAMILY: &str = "Jetendard";
pub const TERMINAL_SIZE: f32 = 12.0;
const GENERIC: [&str; 6] = [
    "monospace",
    "serif",
    "sans-serif",
    "cursive",
    "fantasy",
    "system-ui",
];

#[derive(Clone, PartialEq)]
pub struct Face {
    pub font: Font,
    pub size: f32,
}

pub struct Wanted {
    families: Vec<String>,
    size: f32,
}

pub fn wanted(saved: &TerminalFont) -> Wanted {
    let mut families: Vec<String> = saved
        .family
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(|name| name.trim().trim_matches(['"', '\'']).trim().to_owned())
        .filter(|name| !name.is_empty() && !GENERIC.contains(&name.to_lowercase().as_str()))
        .collect();
    if !families.iter().any(|name| name == TERMINAL_FAMILY) {
        families.push(TERMINAL_FAMILY.to_owned());
    }
    Wanted {
        families,
        size: saved.size.map_or(TERMINAL_SIZE, f32::from),
    }
}

impl Wanted {
    pub fn resolve(&self, installed: &[String]) -> Face {
        let mut found: Vec<String> = self
            .families
            .iter()
            .filter(|name| installed.iter().any(|have| have.eq_ignore_ascii_case(name)))
            .cloned()
            .collect();
        if found.is_empty() {
            found.push(TERMINAL_FAMILY.to_owned());
        }
        let mut primary = font(found.remove(0));
        primary.fallbacks = Some(FontFallbacks::from_fonts(found));
        Face {
            font: primary,
            size: self.size,
        }
    }
}

const FILES: [&[u8]; 5] = [
    include_bytes!(concat!(env!("PRETENDARD_DIR"), "/Pretendard-Regular.woff2")),
    include_bytes!(concat!(env!("PRETENDARD_DIR"), "/Pretendard-Medium.woff2")),
    include_bytes!(concat!(
        env!("PRETENDARD_DIR"),
        "/Pretendard-SemiBold.woff2"
    )),
    include_bytes!("../../web/src/fonts/jetendard/Jetendard-Regular.woff2"),
    include_bytes!("../../web/src/fonts/jetendard/Jetendard-Bold.woff2"),
];

pub fn register(cx: &App) {
    let fonts = FILES
        .map(|woff2| Cow::Owned(wuff::decompress_woff2(woff2).expect("bundled fonts")))
        .to_vec();
    cx.text_system().add_fonts(fonts).expect("bundled fonts");
}
