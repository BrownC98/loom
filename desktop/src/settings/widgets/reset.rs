use gpui::{Div, ElementId, ParentElement, Stateful, StatefulInteractiveElement, Styled, div, px};
use serde_json::Value;

use crate::settings::Ctx;
use crate::strings::Str;
use crate::views::dock::ui::{SMALL, button, say};

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
        button(id, cx)
            .self_start()
            .min_h(px(26.0))
            .text_size(px(SMALL))
            .child(div().child(say(cx, Str::SettingsFieldReset)))
            .on_click(move |_, _, cx| {
                let keys: Vec<&str> = path.iter().map(String::as_str).collect();
                writer.reset(cx, &keys)
            }),
    )
}
