use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use api::graph::text::{Align, Block, TextPart};
use api::graph::waits::WaitDoc;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, Rgba,
    StatefulInteractiveElement, Styled, div, px,
};

use super::body::{self, Follow, Target};
use super::find::Find;
use super::ui;
use crate::fonts;
use crate::theme::Colors;
use crate::views::prose::prose;

const GAP: f32 = 6.0;
const TIGHT: f32 = 2.0;
const CELL_MAX: f32 = 320.0;

struct Draw<'a> {
    root: ElementId,
    next: Cell<usize>,
    doc: Option<&'a WaitDoc>,
    color: Rgba,
    find: Option<&'a Find>,
    size: f32,
    cx: &'a App,
    c: Colors,
    follow: Follow,
}

fn mentions_doc(blocks: &[Block]) -> bool {
    blocks.iter().any(|block| match block {
        Block::Paragraph { parts } | Block::Heading { parts, .. } => body::has_doc(parts),
        Block::List { items, .. } => items.iter().any(|item| mentions_doc(item)),
        Block::Quote { blocks } => mentions_doc(blocks),
        Block::Table { head, rows, .. } => std::iter::once(head)
            .chain(rows)
            .any(|row| row.iter().any(|cell| body::has_doc(cell))),
        Block::Code { .. } | Block::Rule => false,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    id: impl Into<ElementId>,
    blocks: &[Block],
    plain: &str,
    doc: Option<&WaitDoc>,
    color: Rgba,
    find: Option<&Find>,
    size: f32,
    cx: &App,
    follow: impl Fn(Target, &mut App) + 'static,
) -> AnyElement {
    let draw = Draw {
        root: id.into(),
        next: Cell::new(0),
        doc,
        color,
        find,
        size,
        cx,
        c: ui::colors(cx),
        follow: Rc::new(follow),
    };
    let mut column = div().flex().flex_col().gap(px(GAP)).min_w_0();
    if blocks.is_empty() {
        column = column.children(draw.text(&[], plain));
    } else {
        column = column.children(blocks.iter().map(|block| draw.block(block)));
    }
    if let Some(doc) = doc.filter(|_| !mentions_doc(blocks)) {
        column =
            column.child(body::tail(draw.id("doc"), doc, color, cx, &draw.follow).self_start());
    }
    column.into_any_element()
}

impl Draw<'_> {
    fn id(&self, kind: &str) -> ElementId {
        let at = self.next.get();
        self.next.set(at + 1);
        ElementId::NamedChild(Arc::new(self.root.clone()), format!("{kind}-{at}").into())
    }

    fn text(&self, parts: &[TextPart], plain: &str) -> Option<AnyElement> {
        body::inline(
            self.id("text"),
            parts,
            plain,
            self.doc,
            self.color,
            self.find,
            self.cx,
            &self.follow,
        )
    }

    fn stack(&self, blocks: &[Block], gap: f32) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(px(gap))
            .min_w_0()
            .children(blocks.iter().map(|block| self.block(block)))
    }

    fn block(&self, block: &Block) -> AnyElement {
        match block {
            Block::Paragraph { parts } => self
                .text(parts, "")
                .unwrap_or_else(|| div().into_any_element()),
            Block::Heading { level, parts } => div()
                .pt(px(4.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(self.size + (4.0 - f32::from(*level)).clamp(0.0, 3.0)))
                .children(self.text(parts, ""))
                .into_any_element(),
            Block::Code { text } => self.code(text.trim_end_matches('\n')),
            Block::List { start, items } => self.list(*start, items),
            Block::Quote { blocks } => self
                .stack(blocks, GAP)
                .pl(px(10.0))
                .border_l(px(3.0))
                .border_color(self.c.line_strong)
                .text_color(self.c.ink_2)
                .into_any_element(),
            Block::Table { align, head, rows } => self.table(align, head, rows),
            Block::Rule => div()
                .my(px(2.0))
                .h(px(1.0))
                .w_full()
                .bg(self.c.line_strong)
                .into_any_element(),
        }
    }

    fn code(&self, text: &str) -> AnyElement {
        let shown = match self.find {
            Some(find) => find.apply(find.scan(text, 0, Vec::new()), prose(text.to_owned())),
            None => prose(text.to_owned()),
        };
        div()
            .id(self.id("code"))
            .w_full()
            .min_w_0()
            .overflow_x_scroll()
            .border_1()
            .border_color(self.c.line)
            .rounded(px(4.0))
            .bg(self.c.bg)
            .font_family(fonts::TERMINAL_FAMILY)
            .text_size(px(self.size - 1.0))
            .whitespace_nowrap()
            .child(div().flex_none().px(px(8.0)).py(px(6.0)).child(shown))
            .into_any_element()
    }

    fn list(&self, start: Option<u32>, items: &[Vec<Block>]) -> AnyElement {
        let first = start.unwrap_or(1);
        let last = first.saturating_add(items.len() as u32);
        let digits = last.to_string().len() as f32;
        let width = match start {
            Some(_) => (digits + 1.0) * self.size * 0.6 + 6.0,
            None => 0.0,
        }
        .max(self.size + 6.0);
        div()
            .flex()
            .flex_col()
            .gap(px(TIGHT))
            .min_w_0()
            .children(items.iter().enumerate().map(|(at, item)| {
                let marker = match start {
                    Some(_) => format!("{}.", first.saturating_add(at as u32)),
                    None => "\u{2022}".to_owned(),
                };
                div()
                    .flex()
                    .items_start()
                    .child(
                        div()
                            .flex_none()
                            .w(px(width))
                            .pr(px(6.0))
                            .flex()
                            .justify_end()
                            .text_color(self.c.ink_2)
                            .child(marker),
                    )
                    .child(self.stack(item, TIGHT).flex_1())
            }))
            .into_any_element()
    }

    fn table(
        &self,
        align: &[Align],
        head: &[Vec<TextPart>],
        rows: &[Vec<Vec<TextPart>>],
    ) -> AnyElement {
        let mut grid = div()
            .grid()
            .grid_cols_max_content(head.len() as u16)
            .flex_none()
            .border_t_1()
            .border_l_1()
            .border_color(self.c.line_strong);
        for (row, cells) in std::iter::once(head)
            .chain(rows.iter().map(Vec::as_slice))
            .enumerate()
        {
            for (column, cell) in cells.iter().enumerate() {
                let side = align.get(column).copied().unwrap_or(Align::None);
                grid = grid.child(
                    div()
                        .flex()
                        .px(px(8.0))
                        .py(px(3.0))
                        .max_w(px(CELL_MAX))
                        .border_r_1()
                        .border_b_1()
                        .border_color(self.c.line_strong)
                        .when(row == 0, |cell| {
                            cell.bg(self.c.bg).font_weight(FontWeight::SEMIBOLD)
                        })
                        .map(|cell| match side {
                            Align::Right => cell.justify_end(),
                            Align::Center => cell.justify_center(),
                            Align::None | Align::Left => cell,
                        })
                        .children(self.text(cell, "")),
                );
            }
        }
        div()
            .id(self.id("table"))
            .w_full()
            .min_w_0()
            .flex()
            .overflow_x_scroll()
            .child(grid)
            .into_any_element()
    }
}
