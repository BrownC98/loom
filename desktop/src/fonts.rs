use std::borrow::Cow;

use gpui::App;

pub const UI_FAMILY: &str = "Pretendard";
pub const TERMINAL_FAMILY: &str = "Jetendard";

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
