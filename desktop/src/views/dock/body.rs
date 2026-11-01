use std::ops::Range;

use api::graph::text::TextPart;
use api::graph::waits::WaitDoc;
use gpui::{Context, ElementId, HighlightStyle, InteractiveText, StyledText, UnderlineStyle, px};

use super::Dock;
use super::ui;

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
    cx: &mut Context<Dock>,
) -> InteractiveText {
    let c = ui::colors(cx);
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
    if let Some(doc) = doc
        && !inline
    {
        if !built.shown.is_empty() {
            built.push(" ", None);
        }
        built.push(
            &format!("{} {}", doc.id, doc.title),
            Some(Target::Doc(doc.id.clone())),
        );
    }
    let highlight = HighlightStyle {
        color: Some(c.accent.into()),
        underline: Some(UnderlineStyle {
            color: Some(c.accent.into()),
            thickness: px(1.0),
            wavy: false,
        }),
        ..Default::default()
    };
    let Built { shown, links } = built;
    let marks: Vec<_> = links.iter().map(|(r, _)| (r.clone(), highlight)).collect();
    let (ranges, targets): (Vec<_>, Vec<_>) = links.into_iter().unzip();
    let owner = cx.entity();
    InteractiveText::new(id, StyledText::new(shown).with_highlights(marks)).on_click(
        ranges,
        move |index, _, cx| {
            let target = targets[index].clone();
            owner.update(cx, |dock, cx| dock.follow(target, cx));
        },
    )
}
