use gpui::{HighlightStyle, Hsla};

use crate::theme::Colors;
use crate::views::prose::{Prose, prose};

const MARK_ALPHA: f32 = 0.35;

pub fn found(haystack: &str, query: &str) -> bool {
    haystack.to_lowercase().contains(&query.to_lowercase())
}

pub fn mark(text: &str, query: &str, colors: &Colors) -> Prose {
    let styled = prose(text.to_owned());
    if query.is_empty() {
        return styled;
    }
    let lower = text.to_lowercase();
    let needle = query.to_lowercase();
    if lower.len() != text.len() {
        return styled;
    }
    let style = HighlightStyle {
        background_color: Some(Hsla::from(colors.warn).opacity(MARK_ALPHA)),
        ..Default::default()
    };
    styled.with_highlights(
        lower
            .match_indices(&needle)
            .map(|(at, hit)| (at..at + hit.len(), style)),
    )
}
