use api::graph::GraphEvent;
use api::graph::events::{Entry, EventRecord};
use api::profile::AgentView;
use gpui::{
    AnyElement, App, Context, Div, Entity, FontWeight, InteractiveElement, IntoElement,
    ListAlignment, ListOffset, ListState, ParentElement, StatefulInteractiveElement, Styled, div,
    list, px,
};

use super::Dock;
use super::body;
use super::field::{self, Field};
use super::ui::{self, GAP, PAD, SMALL};
use crate::strings::{Lang, Str};
use crate::theme::Colors;
use crate::views::scrollbar::Bar;
use what::{Category, Ctx, Family, Seg, Tone};

pub mod refusal;
mod what;

const CATEGORIES: [(Category, Str); 3] = [
    (Category::Node, Str::EventsCategoryNode),
    (Category::Message, Str::EventsCategoryMessage),
    (Category::Notice, Str::EventsCategoryNotice),
];

const TIME: &str = "%H:%M:%S";
const INDENT: f32 = 80.0;

pub struct Events {
    records: Option<Vec<EventRecord>>,
    agents: Vec<AgentView>,
    newest: Option<u32>,
    held: Option<Vec<EventRecord>>,
    ticket: u64,
    error: Option<String>,
    hidden: Vec<Category>,
    query: Entity<Field>,
    needle: String,
    open: Option<u32>,
    shown: Vec<usize>,
    list: ListState,
    bar: Bar,
}

impl Events {
    pub fn new(cx: &mut App) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(240.0));
        Self {
            records: None,
            agents: Vec::new(),
            newest: None,
            held: None,
            ticket: 0,
            error: None,
            hidden: Vec::new(),
            query: field::new(Str::EventsFilterNamePlaceholder, cx),
            needle: String::new(),
            open: None,
            shown: Vec::new(),
            list: list.clone(),
            bar: Bar::list(&list),
        }
    }

    pub fn query(&self) -> &Entity<Field> {
        &self.query
    }

    fn passes(&self, record: &EventRecord) -> bool {
        !what::hidden(&record.entry)
            && !self.hidden.contains(&what::category_of(&record.entry))
            && (self.needle.is_empty() || what::matches_name(&record.entry, &self.needle))
    }
}

impl Dock {
    pub(super) fn load_events(&mut self, cx: &mut Context<Self>) {
        self.events.ticket += 1;
        let ticket = self.events.ticket;
        self.events.newest = None;
        self.events.held = Some(Vec::new());
        let task = self.store.update(cx, |store, cx| store.event_console(cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |dock, cx| {
                if dock.events.ticket != ticket {
                    return;
                }
                match result {
                    Ok((log, agents)) => {
                        dock.events.newest = Some(log.last);
                        dock.events.records = Some(log.records);
                        dock.events.agents = agents;
                        dock.events.error = None;
                        dock.refilter(cx);
                        for record in dock.events.held.take().into_iter().flatten() {
                            if dock.events.newest.is_none() {
                                break;
                            }
                            dock.append(record, cx);
                        }
                    }
                    Err(error) => {
                        dock.events.held = None;
                        dock.events.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn feed(&mut self, event: &GraphEvent, cx: &mut Context<Self>) {
        let GraphEvent::Recorded { record } = event else {
            return;
        };
        let record: EventRecord = (**record).clone();
        if let Some(held) = &mut self.events.held {
            held.push(record);
            return;
        }
        self.append(record, cx);
    }

    fn append(&mut self, record: EventRecord, cx: &mut Context<Self>) {
        let Some(newest) = self.events.newest else {
            return;
        };
        if record.id <= newest {
            return;
        }
        if record.id != newest + 1 {
            self.load_events(cx);
            return;
        }
        self.events.newest = Some(record.id);
        if what::hidden(&record.entry) {
            return;
        }
        let events = &mut self.events;
        let passes = events.passes(&record);
        let Some(records) = &mut events.records else {
            return;
        };
        records.insert(0, record);
        for at in &mut events.shown {
            *at += 1;
        }
        if passes {
            events.shown.insert(0, 0);
            let top = events.list.logical_scroll_top();
            events.list.splice(0..0, 1);
            if top.item_ix == 0 && top.offset_in_item == px(0.0) {
                events.list.scroll_to(ListOffset::default());
            }
        }
        cx.notify();
    }

    pub(super) fn query_changed(&mut self, cx: &mut Context<Self>) {
        let needle = self.events.query.read(cx).text().trim().to_owned();
        if needle != self.events.needle {
            self.events.needle = needle;
            self.refilter(cx);
            cx.notify();
        }
    }

    fn refilter(&mut self, _: &mut Context<Self>) {
        let events = &mut self.events;
        let shown: Vec<usize> = events
            .records
            .iter()
            .flatten()
            .enumerate()
            .filter(|(_, record)| events.passes(record))
            .map(|(at, _)| at)
            .collect();
        events.list.reset(shown.len());
        events.shown = shown;
    }

    fn toggle_category(&mut self, category: Category, cx: &mut Context<Self>) {
        match self.events.hidden.iter().position(|&c| c == category) {
            Some(at) => {
                self.events.hidden.remove(at);
            }
            None => self.events.hidden.push(category),
        }
        self.refilter(cx);
        cx.notify();
    }

    fn pick_event(&mut self, id: u32, cx: &mut Context<Self>) {
        let opening = self.events.open != Some(id);
        self.events.open = opening.then_some(id);
        let keys = self
            .events
            .records
            .iter()
            .flatten()
            .find(|record| record.id == id)
            .filter(|_| opening)
            .map(|record| {
                what::involved(&record.entry)
                    .iter()
                    .map(u32::to_string)
                    .collect()
            });
        self.store
            .update(cx, |store, cx| store.set_picked(keys, cx));
        if opening {
            let first = self
                .events
                .records
                .iter()
                .flatten()
                .find(|record| record.id == id)
                .and_then(|record| {
                    let present = self.store.read(cx).snapshot()?;
                    what::involved(&record.entry)
                        .into_iter()
                        .find(|&node| present.nodes.iter().any(|n| n.id.0 == node))
                });
            if let Some(node) = first {
                self.store.update(cx, |store, cx| store.select(node, cx));
            }
        }
        if let Some(at) = self
            .events
            .shown
            .iter()
            .position(|&at| self.events.records.as_ref().is_some_and(|r| r[at].id == id))
        {
            self.events.list.remeasure_items(at..at + 1);
        }
        cx.notify();
    }

    pub(super) fn events(&self, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let chips = CATEGORIES.iter().map(|&(category, label)| {
            let on = !self.events.hidden.contains(&category);
            let chip = ui::button(("event-category", category as usize), cx);
            if on {
                chip.bg(c.accent_wash).border_color(c.accent)
            } else {
                chip
            }
            .child(ui::say(cx, label))
            .on_click(cx.listener(move |dock, _, _, cx| dock.toggle_category(category, cx)))
        });
        let head = div()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .p(px(PAD))
            .border_b_1()
            .border_color(c.line)
            .child(
                div()
                    .text_size(px(SMALL))
                    .text_color(c.ink_3)
                    .child(ui::say(cx, Str::EventsOrder)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .children(chips)
                    .child(div().flex_1().min_w_0().child(self.events.query.clone())),
            );
        let mut panel = div().h_full().flex().flex_col().child(head);
        if let Some(error) = &self.events.error {
            panel = panel.child(ui::note(error.clone(), true, cx));
        }
        let note =
            |text: Str, cx: &mut Context<Self>| ui::note(ui::say(cx, text).to_owned(), false, cx);
        match &self.events.records {
            None if self.events.error.is_none() => panel.child(note(Str::EventsReading, cx)),
            None => panel,
            Some(records) if records.is_empty() => panel.child(note(Str::EventsEmpty, cx)),
            Some(_) if self.events.shown.is_empty() => panel.child(note(Str::EventsNoMatch, cx)),
            Some(_) => panel.child(
                self.events
                    .bar
                    .wrap(
                        list(
                            self.events.list.clone(),
                            cx.processor(|dock: &mut Dock, ix: usize, _, cx| {
                                dock.event_row(ix, cx)
                            }),
                        ),
                        true,
                    )
                    .flex_1()
                    .min_h_0(),
            ),
        }
    }

    fn event_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let c = ui::colors(cx);
        let Some(record) = self
            .events
            .shown
            .get(ix)
            .and_then(|&at| self.events.records.as_ref()?.get(at))
        else {
            return div().into_any_element();
        };
        let nodes: Vec<(u32, String)> = self
            .store
            .read(cx)
            .snapshot()
            .map(|s| s.nodes.iter().map(|n| (n.id.0, n.name.clone())).collect())
            .unwrap_or_default();
        let by_name = |name: &str| nodes.iter().find(|(_, n)| n == name).map(|(id, _)| *id);
        let known = |id: u32| nodes.iter().any(|(n, _)| *n == id);
        let ctx = Ctx {
            lang: *cx.global::<Lang>(),
            agents: &self.events.agents,
            by_name: &by_name,
        };
        let id = record.id;
        let current = self.events.open == Some(id);
        let segs = what::what(record, &ctx);
        let body_parts = what::body_of(record, &ctx);
        let (message_kind, doc) = match &record.entry {
            Entry::Message { what, .. } => (what.as_ref(), None),
            Entry::Asked { doc, .. } | Entry::Reported { doc, .. } => (None, doc.as_ref()),
            _ => (None, None),
        };
        let facts = message_kind.zip(record.message.as_ref());
        let summary = message_kind.and_then(|kind| what::summary(kind, &ctx));
        let preview = match (&summary, &body_parts) {
            (Some(text), _) => {
                (!record.message.as_ref().is_some_and(|f| f.from_loom)).then(|| text.clone())
            }
            (None, Some(parts)) => what::plain(parts).lines().next().map(str::to_owned),
            (None, None) => None,
        }
        .filter(|text| !text.is_empty());
        let key = id as usize;

        let line = div()
            .flex()
            .flex_wrap()
            .items_baseline()
            .gap_x(px(GAP))
            .children(segs.into_iter().map(|seg| seg_element(seg, &known, &c)));
        let head = div()
            .id(("event-head", key))
            .flex()
            .gap(px(10.0))
            .px(px(PAD))
            .py(px(7.0))
            .cursor_pointer()
            .child(
                div()
                    .w(px(62.0))
                    .flex_none()
                    .text_size(px(SMALL))
                    .text_color(c.ink_3)
                    .child(ui::clock(record.at, TIME)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(line)
                    .children(preview.filter(|_| !current).map(|text| {
                        div()
                            .text_size(px(SMALL))
                            .text_color(c.ink_2)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(text)
                    })),
            )
            .on_click(cx.listener(move |dock, _, _, cx| dock.pick_event(id, cx)));

        let mut row = div()
            .id(("event", key))
            .relative()
            .mx(px(4.0))
            .rounded(px(6.0))
            .hover(move |style| style.bg(c.surface_hover))
            .child(head);
        if current {
            row = row.bg(c.accent_wash);
        }
        if let Some(family) = what::family_of(&record.entry) {
            row = row.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(7.0))
                    .bottom(px(7.0))
                    .w(px(3.0))
                    .rounded(px(2.0))
                    .bg(family_color(family, &c)),
            );
        }
        if current {
            row = row.child(self.event_details(record, facts, body_parts, doc, key, cx));
        }
        row.into_any_element()
    }

    fn event_details(
        &self,
        record: &EventRecord,
        facts: Option<(
            &api::graph::messages::MessageKind,
            &api::graph::events::MessageFacts,
        )>,
        body_parts: Option<Vec<api::graph::text::TextPart>>,
        doc: Option<&api::graph::waits::WaitDoc>,
        key: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = ui::colors(cx);
        let mut details = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .pl(px(INDENT))
            .pr(px(PAD))
            .pb(px(PAD))
            .text_size(px(SMALL));
        let line = |label: &str, value: AnyElement| {
            div()
                .flex()
                .gap(px(10.0))
                .child(
                    div()
                        .w(px(72.0))
                        .flex_none()
                        .text_color(c.ink_3)
                        .child(label.to_owned()),
                )
                .child(div().flex_1().min_w_0().text_color(c.ink).child(value))
        };
        let Some((kind, facts)) = facts else {
            if body_parts.is_some() || doc.is_some() {
                let text = body::text(
                    ("event-body", key),
                    body_parts.as_deref().unwrap_or_default(),
                    "",
                    doc,
                    cx,
                );
                details = details.child(div().text_color(c.ink_2).child(text));
            }
            return details;
        };
        for (at, field) in facts.fields.iter().enumerate() {
            let text = body::text(("event-field", key * 32 + at), &field.parts, "", None, cx);
            details = details.child(line(
                ui::say(cx, what::field_name(field.name)),
                text.into_any_element(),
            ));
        }
        if let Some(parts) = body_parts {
            let text = body::text(("event-body", key), &parts, "", None, cx);
            let mut value = div()
                .flex()
                .flex_col()
                .gap(px(GAP))
                .text_color(c.ink_2)
                .child(text);
            if !record.choices.is_empty() {
                value = value.child(div().flex().flex_wrap().gap(px(4.0)).children(
                    record.choices.iter().map(|choice| {
                        div()
                            .flex()
                            .gap(px(GAP))
                            .px(px(8.0))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(if choice.recommended {
                                c.warn
                            } else {
                                c.line_strong
                            })
                            .bg(c.surface)
                            .text_color(c.ink)
                            .child(div().text_color(c.ink_3).child(choice.key.to_string()))
                            .child(choice.label.clone())
                    }),
                ));
            }
            details = details.child(line(
                ui::say(cx, what::body_name(kind)),
                value.into_any_element(),
            ));
        }
        details
    }
}

fn family_color(family: Family, c: &Colors) -> gpui::Hsla {
    match family {
        Family::Question => c.warn,
        Family::Done => c.ok,
        Family::Fail => c.err,
    }
    .into()
}

fn seg_element(seg: Seg, known: &dyn Fn(u32) -> bool, c: &Colors) -> AnyElement {
    let base = div().text_size(px(SMALL));
    match seg {
        Seg::Kind(text, tone) => base
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(match tone {
                Tone::Plain => c.ink_2.into(),
                Tone::Family(family) => family_color(family, c),
            })
            .child(text),
        Seg::Who(id, name) => base
            .text_size(px(ui::TEXT))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if id.is_some_and(known) {
                c.ink
            } else {
                c.ink_3
            })
            .child(name),
        Seg::Loom => base
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c.ink_3)
            .child("loom"),
        Seg::Text(text) => base.text_size(px(ui::TEXT)).child(text),
        Seg::Note(text, error) => base
            .text_color(if error { c.err } else { c.ink_3 })
            .child(text),
    }
    .into_any_element()
}
