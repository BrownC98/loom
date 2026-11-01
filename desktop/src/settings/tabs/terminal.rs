use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, Div, Entity, FontWeight, HighlightStyle, Hsla, IntoElement,
    ParentElement, Rgba, ScrollHandle, SharedString, Styled, StyledText, Subscription, Window, div,
    point, px,
};

use crate::fonts::TERMINAL_FAMILY;
use crate::settings::widgets::{
    Number, NumberEvent, Text, TextEvent, field_reset, list_box_on, switch,
};
use crate::settings::{Ctx, Section};
use crate::strings::Str;
use crate::theme::{Scheme, Terminal, chosen_terminal_name, terminal_themes};
use crate::views::dock::ui::{GAP, SMALL, TEXT, colors, say, say_with};

const SIZE_RANGE: std::ops::RangeInclusive<i64> = 10..=24;
const DEFAULT_SIZE: i64 = 12;
const LIST_W: f32 = 240.0;
const LIST_H: f32 = 236.0;
const PREVIEW_H: f32 = 216.0;
const ROW_H: f32 = 28.0;
const ROWS_ABOVE: usize = 4;
const CHIP: f32 = 14.0;
const SWATCH_W: f32 = 26.0;
const SWATCH_H: f32 = 18.0;

#[derive(Default)]
struct Picker {
    hover: Rc<Cell<Option<usize>>>,
    scroll: ScrollHandle,
}

#[derive(Default)]
pub struct State {
    size: Option<Entity<Number>>,
    family: Option<Entity<Text>>,
    dark: Picker,
    light: Picker,
    _subs: Vec<Subscription>,
}

type Span = (&'static str, Option<Rgba>, Option<Rgba>);

fn line(spans: &[Span]) -> StyledText {
    let mut text = String::new();
    let mut styles = Vec::new();
    for (part, color, background) in spans {
        let from = text.len();
        text.push_str(part);
        if color.is_some() || background.is_some() {
            styles.push((
                from..text.len(),
                HighlightStyle {
                    color: color.map(Hsla::from),
                    background_color: background.map(Hsla::from),
                    ..Default::default()
                },
            ));
        }
    }
    StyledText::new(text).with_highlights(styles)
}

fn chips(colors: &[Rgba]) -> Div {
    div().flex().children(
        colors
            .iter()
            .map(|color| div().size(px(CHIP)).bg(*color).mr(px(2.0))),
    )
}

fn swatch(theme: &Terminal, border: Rgba) -> AnyElement {
    div()
        .flex_none()
        .w(px(SWATCH_W))
        .h(px(SWATCH_H))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .border_1()
        .border_color(border)
        .bg(theme.background)
        .text_color(theme.foreground)
        .font_family(TERMINAL_FAMILY)
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(11.0))
        .child("Aa")
        .into_any_element()
}

fn sample(theme: &Terminal) -> Div {
    let a = &theme.ansi;
    let (red, green, yellow) = (Some(a[1]), Some(a[2]), Some(a[3]));
    let (blue, magenta, cyan) = (Some(a[4]), Some(a[5]), Some(a[6]));
    let dim = Some(a[8]);
    let prompt = |command: &'static str| {
        line(&[
            ("~/loom", green, None),
            (" ", None, None),
            ("$", dim, None),
            (" ", None, None),
            (command, None, None),
        ])
    };
    div()
        .flex()
        .flex_col()
        .child(prompt("git status -sb"))
        .child(line(&[
            ("## main", cyan, None),
            ("...", None, None),
            ("origin/main", red, None),
        ]))
        .child(line(&[
            (" ", None, None),
            ("M", red, None),
            (" src/settings.rs", None, None),
        ]))
        .child(line(&[
            ("??", red, None),
            (" web/src/theme.ts", None, None),
        ]))
        .child(prompt("cargo test"))
        .child(line(&[
            ("test settings::put_then_get ... ", None, None),
            ("ok", green, None),
        ]))
        .child(line(&[
            ("test daemon::", None, None),
            (
                "with_theme",
                Some(theme.selection_foreground),
                Some(theme.selection_background),
            ),
            (" ... ", None, None),
            ("FAILED", red, None),
        ]))
        .child(line(&[
            ("warning", yellow, None),
            (": ", None, None),
            ("1 test", blue, None),
            (" ", None, None),
            ("skipped", magenta, None),
        ]))
        .child(chips(&a[..8]))
        .child(chips(&a[8..]))
        .child(
            div()
                .flex()
                .child(prompt(""))
                .child(div().w(px(7.0)).bg(theme.cursor)),
        )
}

fn picker_block(
    id: &'static str,
    section: &mut Section,
    picker: &Picker,
    chosen: usize,
    disabled: bool,
    pick: impl Fn(&'static str, &mut App) + 'static,
    cx: &App,
) -> Div {
    let c = colors(cx);
    let themes = terminal_themes();
    let hovered = picker.hover.get().filter(|_| !disabled);
    let shown = hovered.unwrap_or(chosen).min(themes.len() - 1);
    let (name, theme) = &themes[shown];
    let caption = say_with(cx, Str::ThemePreview, &[("name", name)]);
    let caption = section.text(&caption, cx);
    let hover = picker.hover.clone();
    let list = list_box_on(
        id,
        themes
            .iter()
            .map(|(name, theme)| (SharedString::from(*name), Some(swatch(theme, c.line))))
            .collect(),
        Some(chosen),
        LIST_W,
        LIST_H,
        move |index, _, cx| {
            if !disabled {
                pick(terminal_themes()[index].0, cx);
            }
        },
        Some(
            move |index: Option<usize>, window: &mut Window, _: &mut App| {
                if !disabled && hover.get() != index {
                    hover.set(index);
                    window.refresh();
                }
            },
        ),
        Some(&picker.scroll),
        cx,
    );
    let block = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(GAP))
        .child(
            div()
                .text_size(px(SMALL))
                .text_color(c.ink_3)
                .child(caption),
        )
        .child(
            div()
                .h(px(PREVIEW_H))
                .px(px(12.0))
                .py(px(10.0))
                .overflow_hidden()
                .rounded(px(6.0))
                .border_1()
                .border_color(c.line_strong)
                .bg(theme.background)
                .text_color(theme.foreground)
                .font_family(TERMINAL_FAMILY)
                .text_size(px(12.0))
                .line_height(px(18.0))
                .child(sample(theme)),
        );
    let row = div().flex().gap(px(12.0)).child(list).child(block);
    if disabled { row.opacity(0.5) } else { row }
}

fn scroll_to_chosen(picker: &Picker, chosen: usize) {
    let rows = chosen.saturating_sub(ROWS_ABOVE) as f32;
    picker.scroll.set_offset(point(px(0.0), px(-rows * ROW_H)));
}

fn hint(section: &mut Section, text: Str, cx: &App) -> Div {
    div()
        .text_size(px(SMALL))
        .text_color(colors(cx).ink_3)
        .child(section.text(say(cx, text), cx))
}

fn with_reset(
    section: Section,
    id: &'static str,
    path: &'static [&'static str],
    ctx: &Ctx,
    cx: &App,
) -> Section {
    match field_reset(id, path, ctx, cx) {
        Some(reset) => section.child(reset),
        None => section,
    }
}

fn index_of(name: &str) -> usize {
    terminal_themes()
        .iter()
        .position(|(known, _)| *known == name)
        .unwrap_or_default()
}

fn create(state: &mut State, ctx: &Ctx, window: &mut Window, cx: &mut App) {
    let size = cx.new(|cx| Number::new(SIZE_RANGE, false, true, window, cx));
    let family = cx.new(|cx| {
        Text::new(
            &format!("\"{TERMINAL_FAMILY}\", \"Loom Symbols\", monospace"),
            false,
            window,
            cx,
        )
    });
    let writer = ctx.writer.clone();
    let for_size = cx.subscribe(&size, move |_, event: &NumberEvent, cx: &mut App| {
        let NumberEvent::Commit(value) = event;
        if let Some(size) = value.and_then(|n| u8::try_from(n).ok()) {
            writer.edit(cx, |view| view.terminal_font.size = Some(size));
        }
    });
    let writer = ctx.writer.clone();
    let for_family = cx.subscribe(&family, move |_, event: &TextEvent, cx: &mut App| {
        let TextEvent::Commit(text) = event;
        let family = Some(text.trim().to_owned()).filter(|family| !family.is_empty());
        writer.edit(cx, |view| view.terminal_font.family = family);
    });
    let saved = &ctx.view.terminal_theme;
    scroll_to_chosen(
        &state.dark,
        index_of(chosen_terminal_name(saved.dark.as_deref(), Scheme::Dark)),
    );
    scroll_to_chosen(
        &state.light,
        index_of(chosen_terminal_name(saved.light.as_deref(), Scheme::Light)),
    );
    state.size = Some(size);
    state.family = Some(family);
    state._subs = vec![for_size, for_family];
}

pub fn sections(state: &mut State, ctx: &Ctx, window: &mut Window, cx: &mut App) -> Vec<Section> {
    if state.size.is_none() {
        create(state, ctx, window, cx);
    }
    let (Some(size), Some(family)) = (state.size.clone(), state.family.clone()) else {
        return Vec::new();
    };
    let font = &ctx.view.terminal_font;
    let current = font.size.map(i64::from).unwrap_or(DEFAULT_SIZE);
    size.update(cx, |number, cx| number.show(Some(current), window, cx));
    family.update(cx, |text, cx| {
        text.show(font.family.as_deref().unwrap_or_default(), window, cx);
    });
    let cx: &App = cx;
    let c = colors(cx);
    let saved = &ctx.view.terminal_theme;

    let size_section = with_reset(
        Section::new(Some(say(cx, Str::TerminalFontSize)), ctx).child(size),
        "terminal-size-reset",
        &["terminalFont", "size"],
        ctx,
        cx,
    );

    let mut family_section = Section::new(Some(say(cx, Str::TerminalFontFamily)), ctx);
    let family_hint = hint(&mut family_section, Str::TerminalFontFamilyHint, cx);
    let family_section = with_reset(
        family_section.child(family).child(family_hint),
        "terminal-family-reset",
        &["terminalFont", "family"],
        ctx,
        cx,
    );

    let mut dark = Section::new(Some(say(cx, Str::ThemeDarkTheme)), ctx);
    let writer = ctx.writer.clone();
    let dark_chosen = index_of(chosen_terminal_name(saved.dark.as_deref(), Scheme::Dark));
    let dark_list = picker_block(
        "terminal-dark-theme",
        &mut dark,
        &state.dark,
        dark_chosen,
        false,
        move |name, cx| writer.edit(cx, |view| view.terminal_theme.dark = Some(name.to_owned())),
        cx,
    );
    let dark_hint = hint(&mut dark, Str::ThemePreviewHint, cx);
    let dark = with_reset(
        dark.child(dark_list).child(dark_hint),
        "terminal-dark-reset",
        &["terminalTheme", "dark"],
        ctx,
        cx,
    );

    let mut light = Section::new(Some(say(cx, Str::ThemeLightTheme)), ctx);
    let writer = ctx.writer.clone();
    let match_dark = switch(
        "terminal-match-dark",
        saved.match_dark,
        move |on, _, cx| writer.edit(cx, |view| view.terminal_theme.match_dark = on),
        cx,
    );
    let match_row = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(px(TEXT))
        .font_weight(FontWeight::NORMAL)
        .text_color(c.ink_2)
        .child(match_dark)
        .child(light.text(say(cx, Str::ThemeMatchDark), cx));
    let writer = ctx.writer.clone();
    let light_chosen = index_of(chosen_terminal_name(saved.light.as_deref(), Scheme::Light));
    let light_list = picker_block(
        "terminal-light-theme",
        &mut light,
        &state.light,
        light_chosen,
        saved.match_dark,
        move |name, cx| writer.edit(cx, |view| view.terminal_theme.light = Some(name.to_owned())),
        cx,
    );
    let light = with_reset(
        with_reset(
            light.child(match_row).child(light_list),
            "terminal-light-reset",
            &["terminalTheme", "light"],
            ctx,
            cx,
        ),
        "terminal-match-dark-reset",
        &["terminalTheme", "matchDark"],
        ctx,
        cx,
    );

    vec![size_section, family_section, dark, light]
}
