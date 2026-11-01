use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use api::graph::text::TextPart;
use api::graph::waits::WaitDoc;
use gpui::{
    AnyElement, App, Context, ElementId, Entity, HighlightStyle, InteractiveElement, IntoElement,
    ParentElement, Rgba, StatefulInteractiveElement, Styled, Task, UnderlineStyle, div, px,
};

use super::Dock;
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
}

impl Built {
    fn push(&mut self, label: &str, target: Option<Target>) {
        let start = self.shown.len();
        self.shown.push_str(label);
        if let Some(target) = target {
            self.links.push((start..self.shown.len(), target));
        }
    }
}

pub fn text(
    id: impl Into<ElementId>,
    parts: &[TextPart],
    plain: &str,
    doc: Option<&WaitDoc>,
    color: Rgba,
    cx: &mut Context<Dock>,
) -> AnyElement {
    let owner = cx.entity();
    render(id, parts, plain, doc, color, cx, move |target, cx| {
        owner.update(cx, |dock, cx| dock.follow(target, cx));
    })
}

pub fn open(store: &Entity<Store>, target: Target, cx: &mut App) -> Task<Result<(), String>> {
    match target {
        Target::Url(url) => Task::ready(ui::open_url(&url)),
        Target::Doc(id) => {
            let found = store.update(cx, |store, cx| store.doc_url(&id, cx));
            cx.spawn(async move |_| ui::open_url(&found.await?))
        }
    }
}

pub fn render(
    id: impl Into<ElementId>,
    parts: &[TextPart],
    plain: &str,
    doc: Option<&WaitDoc>,
    color: Rgba,
    cx: &App,
    follow: impl Fn(Target, &mut App) + 'static,
) -> AnyElement {
    let id = id.into();
    let mut built = Built::default();
    let mut inline = false;
    for part in parts {
        match part {
            TextPart::Text { text } => built.push(text, None),
            TextPart::Doc { label } => {
                inline = true;
                built.push(label, doc.map(|d| Target::Doc(d.id.clone())));
            }
            TextPart::Link { label, href } => built.push(label, Some(Target::Url(href.clone()))),
        }
    }
    if parts.is_empty() {
        built.push(plain, None);
    }
    let follow = Rc::new(follow);
    let tail = doc.filter(|_| !inline).map(|doc| {
        let target = Target::Doc(doc.id.clone());
        let follow = follow.clone();
        div()
            .id(id.clone())
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
            .on_click(move |_, _, cx| follow(target.clone(), cx))
    });
    let highlight = HighlightStyle {
        color: Some(color.into()),
        underline: Some(UnderlineStyle {
            color: Some(color.into()),
            thickness: px(1.0),
            wavy: false,
        }),
        ..Default::default()
    };
    let Built { shown, links } = built;
    let marks: Vec<_> = links.iter().map(|(r, _)| (r.clone(), highlight)).collect();
    let (ranges, targets): (Vec<_>, Vec<_>) = links.into_iter().unzip();
    let text = (!shown.is_empty()).then(|| {
        let follow = follow.clone();
        prose(shown).with_highlights(marks).on_click(
            ElementId::NamedChild(Arc::new(id), "text".into()),
            ranges,
            move |index, _, cx| follow(targets[index].clone(), cx),
        )
    });
    match (text, tail) {
        (Some(text), None) => text.into_any_element(),
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
