use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use api::graph::search::WaitSearch;
use gpui::{
    AnyElement, Bounds, Context, Div, ElementId, HighlightStyle, InteractiveElement, IntoElement,
    ParentElement, Pixels, Rgba, ScrollHandle, StatefulInteractiveElement, Styled, Window, canvas,
    div, px, rgb,
};

use super::Dock;
use super::ui;
use crate::strings::Str;
use crate::theme::Colors;
use crate::views::icons::{Lucide, icon};
use crate::views::prose::prose;
use crate::views::tooltip::tip;

const NOW_INK: u32 = 0x171717;
const STEP: f32 = 22.0;
const EDGE: f32 = 6.0;
const MARK: &str = "\u{1}";

pub(crate) type Marks = Vec<(Range<usize>, HighlightStyle)>;

#[derive(Clone)]
struct Reveal {
    jump: Rc<Cell<bool>>,
    view: ScrollHandle,
}

impl Reveal {
    fn show(&self, at: Bounds<Pixels>, window: &mut Window) {
        let view = self.view.bounds();
        if !self.jump.get() || view.size.height <= px(0.0) {
            return;
        }
        self.jump.set(false);
        if at.top() >= view.top() + px(EDGE) && at.bottom() <= view.bottom() - px(EDGE) {
            return;
        }
        let mut offset = self.view.offset();
        offset.y += view.center().y - at.center().y;
        self.view.set_offset(offset);
        window.refresh();
    }
}

#[derive(Default)]
pub(super) struct State {
    words: String,
    current: Cell<usize>,
    total: Cell<usize>,
    jump: Rc<Cell<bool>>,
}

impl State {
    pub(super) fn follow(&mut self, search: Option<&WaitSearch>) {
        let words = search.map_or("", WaitSearch::words);
        if words != self.words {
            self.words = words.to_owned();
            self.restart();
        }
    }

    pub(super) fn restart(&self) {
        self.current.set(0);
        self.jump.set(!self.words.is_empty());
    }

    pub(super) fn begin(
        &self,
        search: Option<&WaitSearch>,
        colors: &Colors,
        view: &ScrollHandle,
    ) -> Option<Find> {
        let search = search?;
        Some(Find {
            words: search.words().to_owned(),
            id: search.id(),
            current: self.current.get(),
            seen: Rc::default(),
            any: colors.find,
            now: colors.find_now,
            reveal: Reveal {
                jump: self.jump.clone(),
                view: view.clone(),
            },
        })
    }

    pub(super) fn finish(
        &self,
        find: Option<&Find>,
        cx: &mut Context<Dock>,
    ) -> Option<(usize, usize)> {
        let Some(find) = find else {
            self.total.set(0);
            return None;
        };
        let total = find.seen.get();
        self.total.set(total);
        let at = self.current.get();
        if total > 0 && at >= total {
            self.current.set(total - 1);
            cx.notify();
        }
        Some((self.current.get().min(total.saturating_sub(1)), total))
    }

    fn step(&self, delta: isize) -> bool {
        let total = self.total.get();
        if total == 0 {
            return false;
        }
        let at = (self.current.get() as isize + delta).rem_euclid(total as isize);
        self.current.set(at as usize);
        self.jump.set(true);
        true
    }
}

#[derive(Clone)]
pub(crate) struct Find {
    words: String,
    id: Option<u32>,
    current: usize,
    seen: Rc<Cell<usize>>,
    any: Rgba,
    now: Rgba,
    reveal: Reveal,
}

pub(crate) struct Found {
    pub marks: Marks,
    pub now: Option<usize>,
}

fn occurrences(text: &str, words: &str) -> Vec<Range<usize>> {
    let mut lower = String::new();
    let mut origin: Vec<usize> = Vec::new();
    for (at, ch) in text.char_indices() {
        for low in ch.to_lowercase() {
            let before = lower.len();
            lower.push(low);
            origin.extend(std::iter::repeat_n(at, lower.len() - before));
        }
    }
    let mut found: Vec<Range<usize>> = Vec::new();
    for (start, hit) in lower.match_indices(words) {
        let from = origin[start];
        let last = origin[start + hit.len() - 1];
        let to = last + text[last..].chars().next().map_or(0, char::len_utf8);
        if found.last().is_none_or(|prev| prev.end <= from) {
            found.push(from..to);
        }
    }
    found
}

impl Find {
    fn take(&self) -> bool {
        let at = self.seen.get();
        self.seen.set(at + 1);
        at == self.current
    }

    fn probe(&self) -> impl Fn(Bounds<Pixels>, &mut Window) + 'static {
        let reveal = self.reveal.clone();
        move |at, window| reveal.show(at, window)
    }

    pub(crate) fn scan(&self, text: &str, shift: usize, base: Marks) -> Found {
        let hits: Vec<(Range<usize>, bool)> = occurrences(text, &self.words)
            .into_iter()
            .map(|range| (range.start + shift..range.end + shift, self.take()))
            .collect();
        let mut cuts: Vec<usize> = base
            .iter()
            .map(|(range, _)| range)
            .chain(hits.iter().map(|(range, _)| range))
            .flat_map(|range| [range.start, range.end])
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        let marks = cuts
            .windows(2)
            .filter_map(|pair| {
                let inside = |range: &Range<usize>| range.start <= pair[0] && pair[1] <= range.end;
                let link = base.iter().find(|(range, _)| inside(range));
                let hit = hits.iter().find(|(range, _)| inside(range));
                if link.is_none() && hit.is_none() {
                    return None;
                }
                let mut style = link.map(|(_, style)| *style).unwrap_or_default();
                if let Some((_, now)) = hit {
                    style.background_color = Some(if *now { self.now } else { self.any }.into());
                    if *now {
                        style.color = Some(rgb(NOW_INK).into());
                    }
                }
                Some((pair[0]..pair[1], style))
            })
            .collect();
        Found {
            marks,
            now: hits
                .iter()
                .find(|(_, now)| *now)
                .map(|(range, _)| range.start),
        }
    }

    pub(crate) fn apply(
        &self,
        found: Found,
        text: crate::views::prose::Prose,
    ) -> crate::views::prose::Prose {
        let text = text.with_highlights(found.marks);
        match found.now {
            Some(at) => text.with_probe(at, self.probe()),
            None => text,
        }
    }
}

pub(crate) fn line(find: Option<&Find>, before: &str, text: &str, after: &str) -> AnyElement {
    let whole = format!("{before}{text}{after}");
    let Some(find) = find else {
        return whole.into_any_element();
    };
    let found = find.scan(text, before.len(), Vec::new());
    if found.marks.is_empty() {
        return whole.into_any_element();
    }
    find.apply(found, prose(whole)).into_any_element()
}

pub(crate) fn instruction(find: Option<&Find>, cx: &gpui::App, text: &str) -> AnyElement {
    let template = ui::say_with(cx, Str::InboxInstruction, &[("text", MARK)]);
    match template.split_once(MARK) {
        Some((before, after)) => line(find, before, text, after),
        None => template.into_any_element(),
    }
}

pub(crate) fn chip(find: Option<&Find>, id: u32, base: Div) -> Div {
    let Some(find) = find.filter(|find| find.id == Some(id)) else {
        return base;
    };
    let now = find.take();
    let base = base.bg(if now { find.now } else { find.any });
    if !now {
        return base;
    }
    let reveal = find.reveal.clone();
    base.text_color(rgb(NOW_INK)).relative().child(
        canvas(
            |_, _, _| (),
            move |at, _, window, _| reveal.show(at, window),
        )
        .absolute()
        .size_full(),
    )
}

impl Dock {
    pub(super) fn find_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.find.step(delta) {
            cx.notify();
        }
    }

    pub(super) fn search_bar(&self, count: Option<(usize, usize)>, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let bar = ui::search(&self.search, cx)
            .w_full()
            .flex()
            .items_center()
            .gap(px(2.0))
            .pr(px(3.0));
        let Some((at, total)) = count else {
            return bar;
        };
        let counter = div()
            .flex_none()
            .px(px(4.0))
            .text_size(px(ui::SMALL))
            .text_color(if total == 0 { c.ink_3 } else { c.ink_2 })
            .child(format!("{} / {total}", (at + 1).min(total)));
        bar.child(counter)
            .child(self.step_button(
                "find-prev",
                Lucide::ChevronUp,
                Str::InboxFindPrev,
                -1,
                total,
                cx,
            ))
            .child(self.step_button(
                "find-next",
                Lucide::ChevronDown,
                Str::InboxFindNext,
                1,
                total,
                cx,
            ))
    }

    fn step_button(
        &self,
        id: impl Into<ElementId>,
        glyph: Lucide,
        label: Str,
        delta: isize,
        total: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = ui::colors(cx);
        let hover = c.surface_hover;
        let button = div()
            .id(id)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(px(STEP))
            .rounded(px(4.0))
            .tooltip(tip(ui::say(cx, label).to_owned()))
            .child(icon(glyph, 14.0, c.ink_2));
        if total == 0 {
            return button.opacity(0.4);
        }
        button
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |dock, _, _, cx| dock.find_step(delta, cx)))
    }
}
