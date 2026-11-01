use api::settings::AlwaysOn;
use gpui::{App, FontWeight, IntoElement, ParentElement, Styled, div, px};

use super::parts::{hint, row, rows};
use crate::settings::widgets::{field_reset, switch};
use crate::settings::{Ctx, Section};
use crate::strings::Str;
use crate::views::dock::ui::{SMALL, TEXT, colors, say};

type Read = fn(&AlwaysOn) -> bool;
type Write = fn(&mut AlwaysOn, bool);

const SKILLS: [(Str, Str, Read, Write); 4] = [
    (
        Str::SettingsSkillOModeName,
        Str::SettingsSkillOModeDesc,
        |on| on.o_mode,
        |on, value| on.o_mode = value,
    ),
    (
        Str::SettingsSkillUserDocName,
        Str::SettingsSkillUserDocDesc,
        |on| on.user_doc,
        |on, value| on.user_doc = value,
    ),
    (
        Str::SettingsSkillSteOrwellName,
        Str::SettingsSkillSteOrwellDesc,
        |on| on.ste_orwell,
        |on, value| on.ste_orwell = value,
    ),
    (
        Str::SettingsSkillRaptor3Name,
        Str::SettingsSkillRaptor3Desc,
        |on| on.raptor3,
        |on, value| on.raptor3 = value,
    ),
];

pub fn section(ctx: &Ctx, cx: &App) -> Section {
    let c = colors(cx);
    let mut section = Section::new(Some(say(cx, Str::SettingsSkills)), ctx);
    let items = SKILLS
        .iter()
        .enumerate()
        .map(|(index, &(name, desc, read, write))| {
            let on = read(&ctx.view.always_on);
            let writer = ctx.writer.clone();
            let name = section.text(say(cx, name), cx);
            let desc = section.text(say(cx, desc), cx);
            let state = section.text(
                say(
                    cx,
                    if on {
                        Str::SettingsSkillOn
                    } else {
                        Str::SettingsSkillOff
                    },
                ),
                cx,
            );
            let mut main = div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(TEXT))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name),
                )
                .child(div().text_size(px(SMALL)).text_color(c.ink_3).child(desc));
            if index == 0 {
                let off = section.text(say(cx, Str::SettingsOModeOff), cx);
                main = main.child(div().text_size(px(SMALL)).text_color(c.ink_3).child(off));
            }
            row()
                .child(main)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(SMALL))
                        .text_color(c.ink_2)
                        .child(switch(
                            ("agents-skill", index),
                            on,
                            move |next, _, cx| {
                                writer
                                    .edit(cx, move |settings| write(&mut settings.always_on, next))
                            },
                            cx,
                        ))
                        .child(state),
                )
                .into_any_element()
        })
        .collect();
    let note = section.text(say(cx, Str::SettingsSkillsHint), cx);
    section
        .beside(field_reset("agents-skills-reset", &["alwaysOn"], ctx, cx))
        .child(rows(items, cx))
        .child(hint(note, cx))
}
