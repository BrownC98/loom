use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, Font, FontStyle, FontWeight, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Pixels, Render,
    Rgba, StrikethroughStyle, Styled, TextAlign, TextRun, UnderlineStyle, Window, canvas, div,
    fill, font, point, px, rgb, size,
};

use crate::link::{self, Event, Pty};
use crate::vt::{self, Key, Screen, Shape, Vt};

pub const NODE_ENV: &str = "LOOM_DESKTOP_NODE";
const SCALE_ENV: &str = "LOOM_DESKTOP_SCALE";
#[cfg(target_os = "linux")]
const FONT_FAMILY: &str = "DejaVu Sans Mono";
#[cfg(target_os = "windows")]
const FONT_FAMILY: &str = "Consolas";
#[cfg(target_os = "macos")]
const FONT_FAMILY: &str = "Menlo";
const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT: f32 = 1.2;
const START: (u16, u16) = (80, 24);
const CURSOR_ALPHA: f32 = 0.6;
const CURSOR_IDLE_ALPHA: f32 = 0.3;
const CURSOR_THIN: f32 = 2.0;

#[derive(Clone, Copy)]
struct Metrics {
    cell_width: Pixels,
    cell_height: Pixels,
}

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

    fn key(&mut self, event: &KeyDownEvent) {
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
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let screen = self.vt.snapshot();
        let scale = self.scale;
        let focused = self.focus.is_focused(window);
        let view = cx.entity();
        div()
            .id("terminal")
            .size_full()
            .relative()
            .bg(color(vt::BACKGROUND))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, _| this.key(event)))
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
                    move |_, bounds, window, cx| paint(bounds, &screen, scale, focused, window, cx),
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
    }
}
