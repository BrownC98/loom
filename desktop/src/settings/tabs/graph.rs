use std::cell::Cell;
use std::rc::Rc;

use api::settings::{DefaultCardView, EdgeStyle, FloatSize, GraphScope, SettingsView};
use gpui::{App, Div, ParentElement, SharedString, Styled, Window, div, px};

mod preview;

use crate::settings::widgets::{field_reset, segmented, segmented_on, switch};
use crate::settings::{Ctx, Section};
use crate::strings::Str;
use crate::views::dock::ui::{SMALL, TEXT, colors, say};

const SIZES: [(FloatSize, Str); 4] = [
    (FloatSize::Small, Str::FloatSizeSmall),
    (FloatSize::Normal, Str::FloatSizeNormal),
    (FloatSize::Large, Str::FloatSizeLarge),
    (FloatSize::Huge, Str::FloatSizeHuge),
];

#[derive(Default)]
pub struct State {
    composer_hover: Rc<Cell<Option<FloatSize>>>,
}

fn head(text: Str, ctx: &Ctx, cx: &App) -> Section {
    Section::new(Some(say(cx, text)), ctx)
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
    section.beside(field_reset(id, path, ctx, cx))
}

fn labels<T: Copy>(section: &mut Section, items: &[(T, Str)], cx: &App) -> Vec<(T, SharedString)> {
    items
        .iter()
        .map(|(value, text)| {
            let label = say(cx, *text);
            let _ = section.text(label, cx);
            (*value, SharedString::from(label))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn choice<T: Copy + PartialEq + 'static>(
    id: &'static str,
    items: &[(T, Str)],
    current: T,
    set: fn(&mut SettingsView, T),
    mut section: Section,
    hint_text: Str,
    reset: (&'static str, &'static [&'static str]),
    ctx: &Ctx,
    cx: &App,
) -> Section {
    let writer = ctx.writer.clone();
    let radios = segmented(
        id,
        labels(&mut section, items, cx),
        current,
        move |value, _, cx| writer.edit(cx, |view| set(view, value)),
        cx,
    );
    let note = hint(&mut section, hint_text, cx);
    with_reset(section.child(radios).child(note), reset.0, reset.1, ctx, cx)
}

fn preview(section: &mut Section, shown: FloatSize, cx: &App) -> Div {
    let name = say(cx, Str::FloatSizeSampleNode);
    let size_label = SIZES
        .iter()
        .find(|(size, _)| *size == shown)
        .map(|(_, label)| say(cx, *label))
        .unwrap_or_default();
    let cap = section.text(
        &crate::views::dock::ui::say_with(cx, Str::FloatSizePreview, &[("size", size_label)]),
        cx,
    );
    let _ = section.text(name, cx);
    preview::frame(
        cap,
        preview::composer(shown, name, cx),
        preview::composer(FloatSize::Huge, name, cx),
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
fn size_choice(
    id: &'static str,
    field: fn(&SettingsView) -> FloatSize,
    set: fn(&mut SettingsView, FloatSize),
    heading: Str,
    hint_text: Str,
    reset: (&'static str, &'static [&'static str]),
    hovered: &Rc<Cell<Option<FloatSize>>>,
    ctx: &Ctx,
    cx: &App,
) -> Section {
    let mut section = head(heading, ctx, cx);
    let current = field(ctx.view);
    let writer = ctx.writer.clone();
    let hover = hovered.clone();
    let radios = segmented_on(
        id,
        labels(&mut section, &SIZES, cx),
        current,
        move |value, _, cx| writer.edit(cx, |view| set(view, value)),
        Some(
            move |size: Option<FloatSize>, window: &mut Window, _: &mut App| {
                if hover.get() != size {
                    hover.set(size);
                    window.refresh();
                }
            },
        ),
        cx,
    );
    let shown = hovered.get().unwrap_or(current);
    let sample = preview(&mut section, shown, cx);
    let note = hint(&mut section, hint_text, cx);
    with_reset(
        section.child(radios).child(sample).child(note),
        reset.0,
        reset.1,
        ctx,
        cx,
    )
}

fn place(
    heading: Str,
    hint_text: Str,
    reset: (&'static str, &'static [&'static str]),
    ctx: &Ctx,
    cx: &App,
) -> Section {
    let mut section = head(heading, ctx, cx);
    let note = hint(&mut section, hint_text, cx);
    with_reset(section.child(note), reset.0, reset.1, ctx, cx)
}

pub fn sections(state: &mut State, ctx: &Ctx, _: &mut Window, cx: &mut App) -> Vec<Section> {
    let cx: &App = cx;
    let c = colors(cx);
    let mut composer = head(Str::ComposerHeading, ctx, cx);
    let writer = ctx.writer.clone();
    let toggle = switch(
        "graph-composer",
        !ctx.view.composer_off,
        composer.text(say(cx, Str::ComposerShow), cx),
        move |on, _, cx| writer.edit(cx, |view| view.composer_off = !on),
        cx,
    );
    let toggle_row = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(px(TEXT))
        .text_color(c.ink_2)
        .child(toggle);
    let composer_hint = hint(&mut composer, Str::ComposerHint, cx);
    let composer = with_reset(
        composer.child(toggle_row).child(composer_hint),
        "graph-composer-reset",
        &["composerOff"],
        ctx,
        cx,
    );

    vec![
        choice(
            "graph-card-view",
            &[
                (DefaultCardView::Simple, Str::CardViewSimple),
                (DefaultCardView::RootTerminal, Str::CardViewRootTerminal),
                (DefaultCardView::Standard, Str::CardViewStandard),
            ],
            ctx.view.card_view,
            |view, value| view.card_view = value,
            head(Str::CardViewHeading, ctx, cx),
            Str::CardViewHint,
            ("graph-card-view-reset", &["cardView"]),
            ctx,
            cx,
        ),
        choice(
            "graph-scope",
            &[
                (GraphScope::Repo, Str::GraphScopeRepo),
                (GraphScope::All, Str::GraphScopeAll),
            ],
            ctx.view.graph_scope,
            |view, value| view.graph_scope = value,
            head(Str::GraphScopeHeading, ctx, cx),
            Str::GraphScopeHint,
            ("graph-scope-reset", &["graphScope"]),
            ctx,
            cx,
        ),
        choice(
            "graph-edge-style",
            &[
                (EdgeStyle::Step, Str::EdgeStyleStep),
                (EdgeStyle::Curve, Str::EdgeStyleCurve),
            ],
            ctx.view.edge_style,
            |view, value| view.edge_style = value,
            head(Str::EdgeStyleHeading, ctx, cx),
            Str::EdgeStyleHint,
            ("graph-edge-style-reset", &["edgeStyle"]),
            ctx,
            cx,
        ),
        composer,
        size_choice(
            "graph-composer-size",
            |view| view.composer_size,
            |view, value| view.composer_size = value,
            Str::FloatSizeComposerSize,
            Str::FloatSizeComposerSizeHint,
            ("graph-composer-size-reset", &["composerSize"]),
            &state.composer_hover,
            ctx,
            cx,
        ),
        place(
            Str::ComposerPlace,
            Str::ComposerPlaceHint,
            ("graph-composer-place-reset", &["composerPlace"]),
            ctx,
            cx,
        ),
    ]
}
