use api::settings::{HexColor, Language, Theme as ThemeSetting, ThemedColor, UiColors};
use gpui::{
    Anchor, App, AppContext, Div, ElementId, Entity, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Rgba, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, anchored, deferred, div,
    point, px,
};
use serde_json::Value;

use super::agents::parts::badge;
use crate::settings::widgets::button;
use crate::settings::widgets::{
    Label, Select, SelectEvent, Text, TextEvent, field_reset, inert, segmented, tabbable,
};
use crate::settings::{Ctx, Dialog, Section, Writer};
use crate::strings::Str;
use crate::theme::{self, Colors, Scheme, Theme, luminance_of, ratio};
use crate::views::dock::ui::{GAP, SMALL, TEXT, colors, say, say_with};
use crate::views::icons::Lucide;
use crate::views::tooltip::tip;

const PRESETS: [&str; 8] = [
    "#ef4444", "#f97316", "#eab308", "#22c55e", "#14b8a6", "#3b82f6", "#8b5cf6", "#ec4899",
];
const SCHEMES: [Scheme; 2] = [Scheme::Light, Scheme::Dark];
const POPOVER_W: f32 = 340.0;
const SWATCH_W: f32 = 28.0;
const SWATCH_H: f32 = 20.0;

struct Row {
    name: Str,
    usage: Str,
    slot: fn(&UiColors) -> &ThemedColor,
    slot_mut: fn(&mut UiColors) -> &mut ThemedColor,
    paint: fn(&Colors) -> Rgba,
    against: fn(&Colors) -> Rgba,
}

const ROWS: [Row; 9] = [
    Row {
        name: Str::ColorsNameWarn,
        usage: Str::ColorsUseWarn,
        slot: |c| &c.warn,
        slot_mut: |c| &mut c.warn,
        paint: |c| c.warn,
        against: |c| c.warn_ink,
    },
    Row {
        name: Str::ColorsNameRun,
        usage: Str::ColorsUseRun,
        slot: |c| &c.run,
        slot_mut: |c| &mut c.run,
        paint: |c| c.run,
        against: |c| c.surface,
    },
    Row {
        name: Str::ColorsNameOk,
        usage: Str::ColorsUseOk,
        slot: |c| &c.ok,
        slot_mut: |c| &mut c.ok,
        paint: |c| c.ok,
        against: |c| c.surface,
    },
    Row {
        name: Str::ColorsNameErr,
        usage: Str::ColorsUseErr,
        slot: |c| &c.err,
        slot_mut: |c| &mut c.err,
        paint: |c| c.err,
        against: |c| c.err_ink,
    },
    Row {
        name: Str::ColorsNameEdge,
        usage: Str::ColorsUseEdge,
        slot: |c| &c.edge,
        slot_mut: |c| &mut c.edge,
        paint: |c| c.edge,
        against: |c| c.bg,
    },
    Row {
        name: Str::ColorsNameRepo,
        usage: Str::ColorsUseRepo,
        slot: |c| &c.repo,
        slot_mut: |c| &mut c.repo,
        paint: |c| c.repo_line,
        against: |c| c.bg,
    },
    Row {
        name: Str::ColorsNameWorktree,
        usage: Str::ColorsUseWorktree,
        slot: |c| &c.worktree,
        slot_mut: |c| &mut c.worktree,
        paint: |c| c.group_line,
        against: |c| c.bg,
    },
    Row {
        name: Str::ColorsNameNotice,
        usage: Str::ColorsUseNotice,
        slot: |c| &c.notice,
        slot_mut: |c| &mut c.notice,
        paint: |c| c.notice,
        against: |c| c.notice_ink,
    },
    Row {
        name: Str::ColorsNameAccent,
        usage: Str::ColorsUseAccent,
        slot: |c| &c.accent,
        slot_mut: |c| &mut c.accent,
        paint: |c| c.accent,
        against: |c| c.accent_ink,
    },
];

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Both,
    Only(Scheme),
}

struct Pop {
    focus: FocusHandle,
    open: Option<usize>,
    target: Target,
    presets: Vec<(Rgba, Rgba)>,
}

#[derive(Default)]
pub struct State {
    pop: Option<Entity<Pop>>,
    hex: Option<Entity<Text>>,
    language: Option<Entity<Select>>,
    subs: Vec<Subscription>,
}

fn parse(text: &str) -> Option<HexColor> {
    serde_json::from_value(Value::String(text.to_ascii_lowercase())).ok()
}

fn side(color: &ThemedColor, scheme: Scheme) -> Option<&HexColor> {
    match scheme {
        Scheme::Light => color.light.as_ref(),
        Scheme::Dark => color.dark.as_ref(),
    }
}

fn changed(color: &ThemedColor) -> bool {
    color.light.is_some() || color.dark.is_some()
}

fn put(slot: &mut ThemedColor, target: Target, hex: Option<HexColor>) {
    match target {
        Target::Both => {
            slot.light = hex.clone();
            slot.dark = hex;
        }
        Target::Only(Scheme::Light) => slot.light = hex,
        Target::Only(Scheme::Dark) => slot.dark = hex,
    }
}

fn pick(writer: &Writer, row: usize, target: Target, hex: Option<HexColor>, cx: &mut App) {
    writer.edit(cx, move |settings| {
        put((ROWS[row].slot_mut)(&mut settings.colors), target, hex);
    });
}

fn touch(
    pop: &Entity<Pop>,
    dialog: &WeakEntity<Dialog>,
    cx: &mut App,
    change: impl FnOnce(&mut Pop),
) {
    pop.update(cx, |pop, _| change(pop));
    let _ = dialog.update(cx, |_, cx| cx.notify());
}

fn tones(row: &Row) -> Vec<(Rgba, Rgba)> {
    let toned = |hex: &HexColor, scheme: Scheme| {
        let mut only = UiColors::default();
        put(
            (row.slot_mut)(&mut only),
            Target::Only(scheme),
            Some(hex.clone()),
        );
        (row.paint)(&theme::colors(scheme, &only))
    };
    PRESETS
        .iter()
        .filter_map(|text| parse(text))
        .map(|hex| (toned(&hex, Scheme::Light), toned(&hex, Scheme::Dark)))
        .collect()
}

fn halves(
    light: Rgba,
    dark: Rgba,
    id: impl Into<ElementId>,
    width: f32,
    height: f32,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .overflow_hidden()
        .rounded(px(4.0))
        .w(px(width))
        .h(px(height))
        .child(div().flex_1().h_full().bg(light))
        .child(div().flex_1().h_full().bg(dark))
}

fn setup(
    state: &mut State,
    ctx: &Ctx,
    window: &mut Window,
    cx: &mut App,
) -> (Entity<Pop>, Entity<Text>, Entity<Select>) {
    if let (Some(pop), Some(hex), Some(language)) = (&state.pop, &state.hex, &state.language) {
        return (pop.clone(), hex.clone(), language.clone());
    }
    let pop = cx.new(|cx| Pop {
        focus: cx.focus_handle(),
        open: None,
        target: Target::Both,
        presets: Vec::new(),
    });
    let hex = cx.new(|cx| Text::mono("#rrggbb", window, cx));
    let language = cx.new(|cx| Select::new("language", cx).full());
    {
        let (pop, writer, dialog) = (pop.clone(), ctx.writer.clone(), ctx.dialog.clone());
        state
            .subs
            .push(cx.subscribe(&hex, move |_, event: &TextEvent, cx| {
                let TextEvent::Commit(text) = event;
                let (open, target) = {
                    let pop = pop.read(cx);
                    (pop.open, pop.target)
                };
                if let (Some(row), Some(hex)) = (open, parse(text.trim())) {
                    pick(&writer, row, target, Some(hex), cx);
                }
                let _ = dialog.update(cx, |_, cx| cx.notify());
            }));
    }
    {
        let writer = ctx.writer.clone();
        state
            .subs
            .push(cx.subscribe(&language, move |_, event: &SelectEvent, cx| {
                let SelectEvent::Pick(index) = event;
                let chosen = [Language::System, Language::En, Language::Ko][*index];
                writer.edit(cx, move |settings| settings.language = chosen);
            }));
    }
    state.pop = Some(pop.clone());
    state.hex = Some(hex.clone());
    state.language = Some(language.clone());
    (pop, hex, language)
}

pub fn sections(state: &mut State, ctx: &Ctx, window: &mut Window, cx: &mut App) -> Vec<Section> {
    let (pop, hex, language) = setup(state, ctx, window, cx);
    vec![
        theme_section(ctx, cx),
        colors_section(&pop, &hex, ctx, window, cx),
        language_section(&language, ctx, cx),
    ]
}

fn theme_section(ctx: &Ctx, cx: &mut App) -> Section {
    let c = colors(cx);
    let mut section = Section::new(Some(say(cx, Str::ThemeHeading)), ctx);
    let items: Vec<(ThemeSetting, Label)> = [
        (ThemeSetting::System, Str::CommonSystem, Lucide::Monitor),
        (ThemeSetting::Light, Str::ThemeLight, Lucide::Sun),
        (ThemeSetting::Dark, Str::ThemeDark, Lucide::Moon),
    ]
    .into_iter()
    .map(|(value, label, glyph)| {
        let _ = section.text(say(cx, label), cx);
        (
            value,
            Label::with_icon(SharedString::from(say(cx, label)), glyph),
        )
    })
    .collect();
    let writer = ctx.writer.clone();
    let row = div().flex().items_center().gap(px(GAP)).child(segmented(
        "theme",
        items,
        ctx.view.theme,
        move |value, _, cx| writer.edit(cx, |settings| settings.theme = value),
        cx,
    ));
    let hint = section.text(say(cx, Str::ThemeSystemHint), cx);
    section
        .beside(field_reset("reset-theme", &["theme"], ctx, cx))
        .child(row)
        .child(div().text_size(px(SMALL)).text_color(c.ink_3).child(hint))
}

fn language_section(select: &Entity<Select>, ctx: &Ctx, cx: &mut App) -> Section {
    let mut section = Section::new(Some(say(cx, Str::SettingsLanguage)), ctx);
    let items: Vec<SharedString> = [Str::CommonSystem, Str::LanguageNameEn, Str::LanguageNameKo]
        .into_iter()
        .map(|label| {
            let _ = section.text(say(cx, label), cx);
            SharedString::from(say(cx, label))
        })
        .collect();
    let current = match ctx.view.language {
        Language::System => 0,
        Language::En => 1,
        Language::Ko => 2,
    };
    select.update(cx, |select, cx| select.show(items, current, cx));
    let row = div()
        .flex()
        .items_center()
        .gap(px(GAP))
        .child(select.clone());
    section
        .beside(field_reset("reset-language", &["language"], ctx, cx))
        .child(row)
}

fn colors_section(
    pop: &Entity<Pop>,
    hex: &Entity<Text>,
    ctx: &Ctx,
    window: &Window,
    cx: &mut App,
) -> Section {
    let c = colors(cx);
    let mut section = Section::new(Some(say(cx, Str::ColorsHeading)), ctx);
    let [light, dark] = SCHEMES.map(|scheme| theme::colors(scheme, &ctx.view.colors));
    let shown = cx.global::<Theme>().scheme;
    let now = match shown {
        Scheme::Light => &light,
        Scheme::Dark => &dark,
    };
    let (open, target, presets) = {
        let pop = pop.read(cx);
        (pop.open, pop.target, pop.presets.clone())
    };

    let mut rows = div()
        .flex()
        .flex_col()
        .rounded(px(8.0))
        .border_1()
        .border_color(c.line)
        .overflow_hidden();
    for (index, row) in ROWS.iter().enumerate() {
        let color = (row.slot)(&ctx.view.colors);
        let name = say(cx, row.name);
        let name_text = section.text(name, cx);
        let usage = section.text(say(cx, row.usage), cx);
        let reset_label = section.text(say(cx, Str::ColorsReset), cx);
        let default_label = say(cx, Str::ColorsDefault);
        let value = if color.light == color.dark {
            let text = color.light.as_ref().map_or(default_label, HexColor::as_str);
            div().child(text.to_owned())
        } else {
            div().flex().flex_col().children(SCHEMES.map(|scheme| {
                let label = match scheme {
                    Scheme::Light => Str::ThemeLight,
                    Scheme::Dark => Str::ThemeDark,
                };
                div()
                    .flex()
                    .gap(px(4.0))
                    .child(div().text_color(c.ink_3).child(say(cx, label)))
                    .child(
                        side(color, scheme)
                            .map_or(default_label, HexColor::as_str)
                            .to_owned(),
                    )
            }))
        };
        let writer = ctx.writer.clone();
        let reset = {
            let item = button(("color-reset", index), cx).child(reset_label);
            if changed(color) {
                item.on_click(move |_, _, cx| {
                    writer.edit(cx, |settings| {
                        *(ROWS[index].slot_mut)(&mut settings.colors) = ThemedColor::default();
                    });
                })
            } else {
                inert(item).opacity(0.5)
            }
        };
        let swatch = {
            let (pop, dialog) = (pop.clone(), ctx.dialog.clone());
            let both = color.light == color.dark;
            tabbable(
                halves(
                    (row.paint)(&light),
                    (row.paint)(&dark),
                    ("color-swatch", index),
                    SWATCH_W,
                    SWATCH_H,
                ),
                cx,
            )
            .cursor_pointer()
            .border_1()
            .border_color(c.line_strong)
            .on_click(move |_, window, cx| {
                touch(&pop, &dialog, cx, |pop| {
                    if pop.open == Some(index) {
                        pop.open = None;
                    } else {
                        pop.open = Some(index);
                        pop.target = if both {
                            Target::Both
                        } else {
                            Target::Only(shown)
                        };
                        pop.presets = tones(&ROWS[index]);
                    }
                });
                let focus = pop.read(cx).focus.clone();
                window.focus(&focus, cx);
            })
        };
        let host = div()
            .relative()
            .flex_none()
            .child(swatch)
            .children((open == Some(index)).then(|| {
                popover(
                    index,
                    name,
                    color,
                    [&light, &dark],
                    target,
                    &presets,
                    pop,
                    hex,
                    ctx,
                    window,
                    cx,
                )
            }));
        let mut line = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(12.0))
            .py(px(6.0))
            .text_size(px(TEXT))
            .child(host)
            .child(
                div()
                    .w(px(120.0))
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name_text),
            )
            .child(
                div()
                    .w(px(150.0))
                    .flex_none()
                    .text_size(px(SMALL))
                    .child(value),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(SMALL))
                    .text_color(c.ink_3)
                    .child(usage),
            )
            .child(reset);
        if index + 1 < ROWS.len() {
            line = line.border_b_1().border_color(c.line);
        }
        rows = rows.child(line);
    }

    let preview = section.text(say(cx, Str::ColorsPreview), cx);
    let pill_label = section.text(say(cx, Str::ColorsPreviewPill), cx);
    let badge_label = section.text(say(cx, Str::ColorsPreviewBadge), cx);
    let dot = |color: Rgba| div().size(px(7.0)).rounded_full().bg(color).flex_none();
    let waiting = div()
        .size(px(7.0))
        .flex_none()
        .rounded_full()
        .bg(now.ask)
        .shadow(vec![gpui::BoxShadow {
            color: gpui::Hsla::from(now.ask).opacity(0.25),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(3.0),
            inset: false,
        }]);
    let strip = div()
        .flex()
        .items_center()
        .gap(px(14.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(c.line)
        .bg(c.bg)
        .text_size(px(SMALL))
        .child(div().text_color(c.ink_3).child(preview))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .h(px(28.0))
                .px(px(10.0))
                .rounded(px(14.0))
                .border_1()
                .border_color(now.warn)
                .bg(mix(now.warn, now.surface, 0.1))
                .child(div().font_weight(FontWeight::BOLD).child(pill_label))
                .child(
                    div()
                        .min_w(px(18.0))
                        .h(px(18.0))
                        .px(px(5.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(9.0))
                        .bg(now.warn)
                        .text_color(now.warn_ink)
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("3"),
                ),
        )
        .child(badge(badge_label, true, now))
        .child(waiting)
        .child(dot(now.run))
        .child(dot(now.ok))
        .child(dot(now.err))
        .child(
            div()
                .flex()
                .p(px(4.0))
                .rounded(px(8.0))
                .border_2()
                .border_color(now.repo_line)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .p(px(4.0))
                        .rounded(px(6.0))
                        .border_2()
                        .border_color(now.group_line)
                        .child(sample_card(now))
                        .child(div().w(px(14.0)).h(px(2.0)).bg(now.edge))
                        .child(sample_card(now)),
                ),
        );
    let hint = section.text(say(cx, Str::ColorsHint), cx);
    section
        .beside(field_reset("reset-colors", &["colors"], ctx, cx))
        .child(rows)
        .child(strip)
        .child(div().text_size(px(SMALL)).text_color(c.ink_3).child(hint))
}

#[allow(clippy::too_many_arguments)]
fn popover(
    index: usize,
    name: &str,
    color: &ThemedColor,
    painted: [&Colors; 2],
    target: Target,
    presets: &[(Rgba, Rgba)],
    pop: &Entity<Pop>,
    hex: &Entity<Text>,
    ctx: &Ctx,
    window: &Window,
    cx: &mut App,
) -> impl IntoElement {
    let c = colors(cx);
    let row = &ROWS[index];
    let current = match target {
        Target::Both if color.light == color.dark => color.light.clone(),
        Target::Both => None,
        Target::Only(scheme) => side(color, scheme).cloned(),
    };
    hex.update(cx, |text, cx| {
        text.show(current.as_ref().map_or("", HexColor::as_str), window, cx)
    });
    let focus = pop.read(cx).focus.clone();
    let close = {
        let (pop, dialog) = (pop.clone(), ctx.dialog.clone());
        move |cx: &mut App| touch(&pop, &dialog, cx, |pop| pop.open = None)
    };
    let close_here = close.clone();
    let title = div()
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .font_weight(FontWeight::SEMIBOLD)
                .child(name.to_owned()),
        )
        .child(
            button("color-pop-close", cx)
                .border_color(gpui::transparent_black())
                .bg(c.panel)
                .child("\u{00D7}")
                .on_click(move |_, _, cx| close_here(cx)),
        );

    let schemes =
        div()
            .flex()
            .gap(px(GAP))
            .children(
                SCHEMES
                    .into_iter()
                    .zip(painted)
                    .enumerate()
                    .map(|(at, (scheme, set))| {
                        let pressed = target == Target::Both || target == Target::Only(scheme);
                        let label = match scheme {
                            Scheme::Light => Str::ThemeLight,
                            Scheme::Dark => Str::ThemeDark,
                        };
                        let ratio_text = side(color, scheme).map(|_| {
                            format!(
                                "{:.1}:1",
                                ratio(
                                    luminance_of((row.paint)(set)),
                                    luminance_of((row.against)(set))
                                )
                            )
                        });
                        let (pop, dialog) = (pop.clone(), ctx.dialog.clone());
                        let hint = if target == Target::Only(scheme) {
                            say(cx, Str::ColorsBothThemes).to_owned()
                        } else {
                            say_with(cx, Str::ColorsOnlyTheme, &[("theme", say(cx, label))])
                        };
                        button(("color-scheme", at), cx)
                            .tooltip(tip(hint))
                            .flex_1()
                            .gap(px(6.0))
                            .border_color(if pressed { c.accent } else { c.line_strong })
                            .bg(if pressed { c.accent_wash } else { c.surface })
                            .child(
                                div()
                                    .size(px(22.0))
                                    .rounded(px(4.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg((row.paint)(set))
                                    .text_color((row.against)(set))
                                    .child("3"),
                            )
                            .child(say(cx, label))
                            .children(ratio_text.map(|text| div().text_color(c.ink_3).child(text)))
                            .on_click(move |_, _, cx| {
                                touch(&pop, &dialog, cx, |pop| {
                                    pop.target = if pop.target == Target::Only(scheme) {
                                        Target::Both
                                    } else {
                                        Target::Only(scheme)
                                    };
                                })
                            })
                    }),
            );

    let swatches = div().flex().flex_wrap().gap(px(6.0)).children(
        PRESETS
            .iter()
            .zip(presets)
            .enumerate()
            .map(|(at, (text, &(light, dark)))| {
                let (left, right) = match target {
                    Target::Both => (light, dark),
                    Target::Only(Scheme::Light) => (light, light),
                    Target::Only(Scheme::Dark) => (dark, dark),
                };
                let chosen = current.as_ref().map(HexColor::as_str) == Some(*text);
                let (writer, picked) = (ctx.writer.clone(), parse(text));
                tabbable(
                    halves(left, right, ("color-preset", at), SWATCH_W, SWATCH_H + 6.0),
                    cx,
                )
                .cursor_pointer()
                .border_2()
                .border_color(if chosen { c.accent } else { c.line_strong })
                .on_click(move |_, _, cx| pick(&writer, index, target, picked.clone(), cx))
            }),
    );

    let writer = ctx.writer.clone();
    let can_reset = match target {
        Target::Both => changed(color),
        Target::Only(scheme) => side(color, scheme).is_some(),
    };
    let reset = {
        let item = button("color-pop-reset", cx).child(say(cx, Str::ColorsReset));
        if can_reset {
            let close = close.clone();
            item.on_click(move |_, _, cx| {
                pick(&writer, index, target, None, cx);
                close(cx);
            })
        } else {
            inert(item).opacity(0.5)
        }
    };
    let hint = |text: Str| {
        div()
            .text_size(px(SMALL))
            .text_color(c.ink_3)
            .child(say(cx, text))
    };
    let panel = div()
        .id("color-popover")
        .track_focus(&focus)
        .w(px(POPOVER_W))
        .occlude()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .p(px(14.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(c.line_strong)
        .bg(c.panel)
        .text_size(px(TEXT))
        .text_color(c.ink)
        .on_key_down({
            let close = close.clone();
            move |event, _, cx| {
                if event.keystroke.key == "escape" {
                    close(cx);
                    cx.stop_propagation();
                }
            }
        })
        .child(title)
        .child(schemes)
        .child(hint(Str::ColorsPresets))
        .child(swatches)
        .child(hint(Str::ColorsValue))
        .child(div().child(hex.clone()))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(GAP))
                .child(reset)
                .child(hint(Str::ColorsSaved)),
        );
    let view = window.viewport_size();
    let backdrop = deferred(
        anchored()
            .position(point(px(0.0), px(0.0)))
            .anchor(Anchor::TopLeft)
            .child(
                div()
                    .w(view.width)
                    .h(view.height)
                    .occlude()
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| close(cx)),
            ),
    )
    .with_priority(1);
    let body = deferred(
        anchored()
            .anchor(Anchor::TopLeft)
            .offset(point(px(0.0), px(SWATCH_H + 6.0)))
            .snap_to_window()
            .child(panel),
    )
    .with_priority(2);
    div().child(backdrop).child(body)
}

fn mix(top: Rgba, under: Rgba, amount: f32) -> Rgba {
    let blend = |a: f32, b: f32| a * amount + b * (1.0 - amount);
    Rgba {
        r: blend(top.r, under.r),
        g: blend(top.g, under.g),
        b: blend(top.b, under.b),
        a: 1.0,
    }
}

fn sample_card(now: &Colors) -> Div {
    div()
        .w(px(22.0))
        .h(px(14.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(now.line_strong)
        .bg(now.term)
}
