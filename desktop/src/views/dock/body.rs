use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use api::graph::text::TextPart;
use api::graph::waits::WaitDoc;
use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, FontStyle, FontWeight, HighlightStyle,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Rgba, Stateful,
    StatefulInteractiveElement, Styled, Task, UnderlineStyle, div, px,
};

use super::Dock;
use super::find::Find;
use super::ui;
use crate::store::Store;
use crate::strings::Str;
use crate::views::icons::{Lucide, icon};
use crate::views::prose::prose;
use crate::views::tooltip::tip;

#[derive(Clone)]
pub enum Target {
    Doc(String),
    Url(String),
}

#[derive(Default)]
struct Built {
    shown: String,
    links: Vec<(Range<usize>, Target)>,
    styles: Vec<(Range<usize>, HighlightStyle)>,
}

impl Built {
    fn push(&mut self, label: &str, target: Option<Target>) {
        let start = self.shown.len();
        self.shown.push_str(label);
        if let Some(target) = target {
            self.links.push((start..self.shown.len(), target));
        }
    }

    fn styled(&mut self, text: &str, style: HighlightStyle) {
        let start = self.shown.len();
        self.shown.push_str(text);
        self.styles.push((start..self.shown.len(), style));
    }
}

pub fn text(
    id: impl Into<ElementId>,
    parts: &[TextPart],
    plain: &str,
    doc: Option<&WaitDoc>,
    color: Rgba,
    find: Option<&Find>,
    cx: &mut Context<Dock>,
) -> AnyElement {
    let owner = cx.entity();
    render(id, parts, plain, doc, color, find, cx, move |target, cx| {
        owner.update(cx, |dock, cx| dock.follow(target, cx));
    })
}

pub fn open(store: &Entity<Store>, target: Target, cx: &mut App) -> Task<Result<(), String>> {
    match target {
        Target::Url(url) => Task::ready(ui::open_url(&url)),
        Target::Doc(id) => Task::ready(
            store
                .read(cx)
                .doc_url(&id)
                .and_then(|url| ui::open_url(&url)),
        ),
    }
}

pub type Follow = Rc<dyn Fn(Target, &mut App)>;

pub fn has_doc(parts: &[TextPart]) -> bool {
    parts
        .iter()
        .any(|part| matches!(part, TextPart::Doc { .. }))
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    id: impl Into<ElementId>,
    parts: &[TextPart],
    plain: &str,
    doc: Option<&WaitDoc>,
    color: Rgba,
    find: Option<&Find>,
    cx: &App,
    follow: impl Fn(Target, &mut App) + 'static,
) -> AnyElement {
    let id = id.into();
    let follow: Follow = Rc::new(follow);
    let text = inline(id.clone(), parts, plain, doc, color, find, cx, &follow);
    let tail = doc
        .filter(|_| !has_doc(parts))
        .map(|doc| tail(id, doc, color, cx, &follow));
    match (text, tail) {
        (Some(text), None) => text,
        (None, Some(tail)) => tail.into_any_element(),
        (Some(text), Some(tail)) => div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(4.0))
            .child(text)
            .child(tail)
            .into_any_element(),
        (None, None) => div().into_any_element(),
    }
}

pub fn tail(id: ElementId, doc: &WaitDoc, color: Rgba, cx: &App, follow: &Follow) -> Stateful<Div> {
    let target = Target::Doc(doc.id.clone());
    let follow = follow.clone();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(3.0))
        .text_color(color)
        .underline()
        .cursor_pointer()
        .tooltip(tip(ui::say_with(
            cx,
            Str::InboxOpenDocLabel,
            &[("id", &doc.id), ("title", &doc.title)],
        )))
        .child(format!("{} {}", doc.id, doc.title))
        .child(icon(Lucide::ExternalLink, 12.0, color))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| follow(target.clone(), cx))
}

#[allow(clippy::too_many_arguments)]
pub fn inline(
    id: ElementId,
    parts: &[TextPart],
    plain: &str,
    doc: Option<&WaitDoc>,
    color: Rgba,
    find: Option<&Find>,
    cx: &App,
    follow: &Follow,
) -> Option<AnyElement> {
    let mut built = Built::default();
    for part in parts {
        match part {
            TextPart::Text { text } => built.push(text, None),
            TextPart::Doc { label } => built.push(label, doc.map(|d| Target::Doc(d.id.clone()))),
            TextPart::Link { label, href } => built.push(label, Some(Target::Url(href.clone()))),
            TextPart::Strong { text } => built.styled(
                text,
                HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
            ),
            TextPart::Emphasis { text } => built.styled(
                text,
                HighlightStyle {
                    font_style: Some(FontStyle::Italic),
                    ..Default::default()
                },
            ),
            TextPart::Code { text } => built.styled(
                text,
                HighlightStyle {
                    background_color: Some(ui::colors(cx).line.into()),
                    ..Default::default()
                },
            ),
        }
    }
    if parts.is_empty() {
        built.push(plain, None);
    }
    let highlight = HighlightStyle {
        color: Some(color.into()),
        underline: Some(UnderlineStyle {
            color: Some(color.into()),
            thickness: px(1.0),
            wavy: false,
        }),
        ..Default::default()
    };
    let Built {
        shown,
        links,
        styles,
    } = built;
    if shown.is_empty() {
        return None;
    }
    let mut marks: Vec<_> = links.iter().map(|(r, _)| (r.clone(), highlight)).collect();
    marks.extend(styles);
    marks.sort_by_key(|(r, _)| r.start);
    let (ranges, targets): (Vec<_>, Vec<_>) = links.into_iter().unzip();
    let follow = follow.clone();
    let base = match find {
        Some(find) => find.apply(find.scan(&shown, 0, marks), prose(shown)),
        None => prose(shown).with_highlights(marks),
    };
    Some(
        base.on_click(
            ElementId::NamedChild(Arc::new(id), "text".into()),
            ranges,
            move |index, _, cx| follow(targets[index].clone(), cx),
        )
        .into_any_element(),
    )
}
