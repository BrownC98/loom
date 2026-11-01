use std::borrow::Cow;
use std::ops::Range;

use gpui::{
    App, AppContext, Bounds, Context, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    Font, FontStyle, FontWeight, Global, Hsla, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, Pixels, Render, Rgba, StrikethroughStyle, Styled, TextAlign,
    TextRun, UTF16Selection, UnderlineStyle, Window, canvas, div, fill, font, point, px, rgb, size,
};
use unicode_width::UnicodeWidthChar;

use crate::link::{self, Event, Pty};
use crate::vt::{self, Key, Screen, Shape, Vt};

pub const NODE_ENV: &str = "LOOM_DESKTOP_NODE";
const SCALE_ENV: &str = "LOOM_DESKTOP_SCALE";
const FONT_FAMILY: &str = "Jetendard";
const FONT_FILES: [&[u8]; 2] = [
    include_bytes!("../../../web/src/fonts/jetendard/Jetendard-Regular.woff2"),
    include_bytes!("../../../web/src/fonts/jetendard/Jetendard-Bold.woff2"),
];
const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT: f32 = 1.3;
const MARKED_BACKGROUND: vt::Rgb8 = [0x3a, 0x3a, 0x55];
const START: (u16, u16) = (80, 24);
const CURSOR_ALPHA: f32 = 0.6;
const CURSOR_IDLE_ALPHA: f32 = 0.3;
const CURSOR_THIN: f32 = 2.0;

#[derive(Clone, Copy)]
struct Metrics {
    cell_width: Pixels,
    cell_height: Pixels,
}

struct FontsLoaded;

impl Global for FontsLoaded {}

pub struct TerminalView {
    vt: Vt,
    pty: Option<Pty>,
    focus: FocusHandle,
    settled: bool,
    scale: f32,
    area: Bounds<Pixels>,
    cell: Option<Metrics>,
    sent: (u16, u16),
    ended: Option<String>,
    marked: Option<String>,
    cursor: Option<vt::Cursor>,
}

fn color([r, g, b]: vt::Rgb8) -> Rgba {
    rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
}

fn hsla(rgb: vt::Rgb8) -> Hsla {
    color(rgb).into()
}

fn metrics(window: &Window, scale: f32) -> Metrics {
    let system = window.text_system();
    let id = system.resolve_font(&font(FONT_FAMILY));
    let em = px(FONT_SIZE * scale);
    Metrics {
        cell_width: system.advance(id, em, 'm').map_or(em * 0.6, |a| a.width),
        cell_height: em * LINE_HEIGHT,
    }
}

pub fn show(
    started: Result<String, String>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TerminalView> {
    let opened = started.and_then(|base| {
        let nodes = link::nodes(&base)?;
        let wanted = std::env::var(NODE_ENV)
            .ok()
            .and_then(|id| id.parse::<u32>().ok());
        let node = nodes
            .iter()
            .find(|n| wanted.is_none_or(|id| n.id == id))
            .ok_or("no node to show")?;
        let (events, inbox) = async_channel::unbounded();
        Ok((link::attach(&base, node.id, events)?, inbox))
    });
    cx.new(|cx| TerminalView::new(opened, window, cx))
}

impl TerminalView {
    fn new(
        opened: Result<(Pty, async_channel::Receiver<Event>), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (pty, ended) = match opened {
            Ok((pty, inbox)) => {
                cx.spawn(async move |this, cx| {
                    while let Ok(event) = inbox.recv().await {
                        if this.update(cx, |view, cx| view.apply(event, cx)).is_err() {
                            break;
                        }
                    }
                })
                .detach();
                (Some(pty), None)
            }
            Err(error) => (None, Some(error)),
        };
        if !cx.has_global::<FontsLoaded>() {
            let files = FONT_FILES
                .map(|woff2| {
                    Cow::Owned(wuff::decompress_woff2(woff2).expect("bundled terminal fonts"))
                })
                .to_vec();
            cx.text_system()
                .add_fonts(files)
                .expect("bundled terminal fonts");
            cx.set_global(FontsLoaded);
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            vt: Vt::new(START.0, START.1),
            pty,
            focus,
            settled: false,
            scale: std::env::var(SCALE_ENV)
                .ok()
                .and_then(|scale| scale.parse().ok())
                .unwrap_or(1.0),
            area: Bounds::default(),
            cell: None,
            sent: (0, 0),
            ended,
            marked: None,
            cursor: None,
        }
    }

    fn apply(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Size { cols, rows } => {
                self.vt.resize(cols, rows);
                self.claim();
            }
            Event::Settled => self.settled = true,
            Event::Backlog(bytes) => {
                self.vt.feed(&bytes);
                let replies = self.vt.take_replies();
                if !self.settled {
                    self.reply(replies);
                }
            }
            Event::Output(bytes) => {
                self.vt.feed(&bytes);
                let replies = self.vt.take_replies();
                self.reply(replies);
            }
            Event::Gone => self.ended = Some("the terminal is gone".to_owned()),
            Event::Closed(reason) => {
                self.ended = Some(if reason.is_empty() {
                    "connection closed".to_owned()
                } else {
                    reason
                });
            }
        }
        cx.notify();
    }

    fn reply(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.send(bytes);
        }
    }

    fn send(&self, bytes: Vec<u8>) {
        if let Some(pty) = &self.pty {
            pty.input(bytes);
        }
    }

    fn resized(&mut self, area: Bounds<Pixels>, cell: Metrics) {
        self.area = area;
        self.cell = Some(cell);
        self.claim();
    }

    fn claim(&mut self) {
        let (Some(cell), true) = (self.cell, self.area.size.width > px(0.0)) else {
            return;
        };
        let cols = (self.area.size.width / self.scale / cell.cell_width).floor() as u16;
        let rows = (self.area.size.height / self.scale / cell.cell_height).floor() as u16;
        let fit = (cols.max(2), rows.max(1));
        if fit != self.sent {
            self.sent = fit;
            if let Some(pty) = &self.pty {
                pty.resize(fit.0, fit.1);
            }
        }
    }

    fn key(&mut self, event: &KeyDownEvent, cx: &mut App) {
        let stroke = &event.keystroke;
        if stroke.modifiers.platform {
            return;
        }
        let key = Key {
            name: &stroke.key,
            text: stroke.key_char.as_deref(),
            ctrl: stroke.modifiers.control,
            alt: stroke.modifiers.alt,
            shift: stroke.modifiers.shift,
        };
        if let Some(bytes) = self.vt.encode_key(&key) {
            self.claim();
            self.send(bytes);
            cx.stop_propagation();
        }
    }

    fn marked_len(&self) -> usize {
        self.marked
            .as_deref()
            .map_or(0, |text| text.encode_utf16().count())
    }
}

fn utf16_slice(text: &str, range: &Range<usize>) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let end = range.end.min(units.len());
    let start = range.start.min(end);
    String::from_utf16_lossy(&units[start..end])
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.marked.as_deref()?;
        let end = range.end.min(self.marked_len());
        *adjusted_range = Some(range.start.min(end)..end);
        Some(utf16_slice(text, &range))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.marked_len();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|_| 0..self.marked_len())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        if !text.is_empty() {
            self.claim();
            self.send(text.as_bytes().to_vec());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| new_text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (cursor, cell) = (self.cursor?, self.cell?);
        let text = self.marked.as_deref().unwrap_or_default();
        let before = utf16_slice(text, &(0..range_utf16.start));
        let cols = before
            .chars()
            .map(|c| c.width().unwrap_or(1))
            .sum::<usize>()
            + cursor.col;
        let (width, height) = (cell.cell_width * self.scale, cell.cell_height * self.scale);
        Some(Bounds::new(
            self.area.origin + point(width * cols as f32, height * cursor.row as f32),
            size(width, height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let screen = self.vt.snapshot();
        self.cursor = screen.cursor;
        let marked = self.marked.clone();
        let focus = self.focus.clone();
        let scale = self.scale;
        let focused = self.focus.is_focused(window);
        let view = cx.entity();
        let view_input = view.clone();
        div()
            .id("terminal")
            .size_full()
            .relative()
            .bg(color(vt::BACKGROUND))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.key(event, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
            )
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let base = metrics(window, 1.0);
                        view.update(cx, |view, _| view.resized(bounds, base));
                        bounds
                    },
                    move |_, bounds, window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, view_input.clone()),
                            cx,
                        );
                        paint(
                            bounds,
                            &screen,
                            marked.as_deref(),
                            scale,
                            focused,
                            window,
                            cx,
                        );
                    },
                )
                .size_full(),
            )
            .children(self.ended.clone().map(|text| {
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .px_2()
                    .bg(hsla([0x55, 0x57, 0x53]))
                    .text_color(color(vt::FOREGROUND))
                    .child(text)
            }))
    }
}

fn styled(cell: &vt::Cell, len: usize) -> TextRun {
    let mut run_font: Font = font(FONT_FAMILY);
    if cell.bold {
        run_font.weight = FontWeight::BOLD;
    }
    if cell.italic {
        run_font.style = FontStyle::Italic;
    }
    let fg = hsla(cell.fg);
    TextRun {
        len,
        font: run_font,
        color: fg,
        background_color: None,
        underline: cell.underline.then_some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(fg),
            wavy: false,
        }),
        strikethrough: cell.strike.then_some(StrikethroughStyle {
            thickness: px(1.0),
            color: Some(fg),
        }),
    }
}

fn same_style(a: &vt::Cell, b: &vt::Cell) -> bool {
    (a.fg, a.bold, a.italic, a.underline, a.strike)
        == (b.fg, b.bold, b.italic, b.underline, b.strike)
}

fn paint(
    bounds: Bounds<Pixels>,
    screen: &Screen,
    marked: Option<&str>,
    scale: f32,
    focused: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let cell = metrics(window, scale);
    let em = px(FONT_SIZE * scale);
    let at = |col: usize, row: usize| {
        point(
            bounds.origin.x + cell.cell_width * col as f32,
            bounds.origin.y + cell.cell_height * row as f32,
        )
    };
    for row in 0..screen.rows {
        let cells = screen.row(row);
        let mut col = 0;
        while col < cells.len() {
            let start = col;
            while col < cells.len() && cells[col].bg == cells[start].bg {
                col += 1;
            }
            if cells[start].bg != vt::BACKGROUND {
                window.paint_quad(fill(
                    Bounds::new(
                        at(start, row),
                        size(cell.cell_width * (col - start) as f32, cell.cell_height),
                    ),
                    hsla(cells[start].bg),
                ));
            }
        }
        if cells
            .iter()
            .all(|c| c.ch == ' ' && !c.underline && !c.strike)
        {
            continue;
        }
        let text: String = cells.iter().map(|c| c.ch).collect();
        let mut runs: Vec<TextRun> = Vec::new();
        let mut first = 0;
        for (i, c) in cells.iter().enumerate() {
            if i > first && !same_style(&cells[first], c) {
                let len = cells[first..i].iter().map(|c| c.ch.len_utf8()).sum();
                runs.push(styled(&cells[first], len));
                first = i;
            }
        }
        let len = cells[first..].iter().map(|c| c.ch.len_utf8()).sum();
        runs.push(styled(&cells[first], len));
        let line = window
            .text_system()
            .shape_line(text.into(), em, &runs, Some(cell.cell_width));
        let _ = line.paint(
            at(0, row),
            cell.cell_height,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
    if let Some(cursor) = screen.cursor {
        let mut hue = hsla(vt::CURSOR);
        hue.a = if focused {
            CURSOR_ALPHA
        } else {
            CURSOR_IDLE_ALPHA
        };
        let thin = px(CURSOR_THIN * scale);
        let (origin, extent) = match cursor.shape {
            Shape::Block => (
                at(cursor.col, cursor.row),
                size(cell.cell_width, cell.cell_height),
            ),
            Shape::Beam => (at(cursor.col, cursor.row), size(thin, cell.cell_height)),
            Shape::Underline => (
                at(cursor.col, cursor.row + 1) - point(px(0.0), thin),
                size(cell.cell_width, thin),
            ),
        };
        window.paint_quad(fill(Bounds::new(origin, extent), hue));
        if let Some(text) = marked {
            paint_marked(text, cursor, &at, cell, em, window, cx);
        }
    }
}

fn paint_marked(
    text: &str,
    cursor: vt::Cursor,
    at: &dyn Fn(usize, usize) -> gpui::Point<Pixels>,
    cell: Metrics,
    em: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let glyphs: Vec<(char, usize)> = text
        .chars()
        .map(|c| (c, c.width().unwrap_or(1).max(1)))
        .collect();
    let cols: usize = glyphs.iter().map(|(_, width)| width).sum();
    window.paint_quad(fill(
        Bounds::new(
            at(cursor.col, cursor.row),
            size(cell.cell_width * cols as f32, cell.cell_height),
        ),
        hsla(MARKED_BACKGROUND),
    ));
    let mut col = cursor.col;
    for (c, width) in glyphs {
        let mark = vt::Cell {
            ch: c,
            fg: vt::FOREGROUND,
            bg: MARKED_BACKGROUND,
            bold: false,
            italic: false,
            underline: true,
            strike: false,
        };
        let run = styled(&mark, c.len_utf8());
        let line = window.text_system().shape_line(
            c.to_string().into(),
            em,
            &[run],
            Some(cell.cell_width * width as f32),
        );
        let _ = line.paint(
            at(col, cursor.row),
            cell.cell_height,
            TextAlign::Left,
            None,
            window,
            cx,
        );
        col += width;
    }
}
