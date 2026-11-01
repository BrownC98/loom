use gpui::{Div, ElementId, ParentElement, Stateful, StatefulInteractiveElement, Styled, px};
use serde_json::Value;

use super::btn::ghost;
use crate::settings::Ctx;
use crate::strings::Str;
use crate::views::dock::ui::{SMALL, colors, say};
use crate::views::icons::{Lucide, icon};
use crate::views::tooltip::tip;

fn at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |value, key| value.get(*key))
}

pub fn field_reset(
    id: impl Into<ElementId>,
    path: &[&str],
    ctx: &Ctx,
    cx: &gpui::App,
) -> Option<Stateful<Div>> {
    let defaults = serde_json::to_value(ctx.defaults?).ok()?;
    let current = serde_json::to_value(ctx.view).ok()?;
    if at(&current, path) == at(&defaults, path) {
        return None;
    }
    let writer = ctx.writer.clone();
    let path: Vec<String> = path.iter().map(|key| (*key).to_owned()).collect();
    Some(
        ghost(id, cx)
            .self_start()
            .min_h(px(26.0))
            .px(px(8.0))
            .text_size(px(SMALL))
            .tooltip(tip(say(cx, Str::SettingsFieldResetHint).to_owned()))
            .child(icon(Lucide::RotateCcw, 14.0, colors(cx).ink_2))
            .child(say(cx, Str::SettingsFieldReset))
            .on_click(move |_, _, cx| {
                let keys: Vec<&str> = path.iter().map(String::as_str).collect();
                writer.reset(cx, &keys)
            }),
    )
}
