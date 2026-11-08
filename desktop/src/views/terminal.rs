use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, BorderStyle, Bounds, ClipboardItem, Context, DispatchPhase,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, FontStyle, FontWeight, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, MouseButton,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, Rgba, ScrollEvent,
    StrikethroughStyle, Styled, Subscription, TextAlign, TextRun, UTF16Selection, UnderlineStyle,
    Window, canvas, div, fill, outline, point, px, rgb, size,
};
use unicode_width::UnicodeWidthChar;

use crate::fonts::Face;
use crate::link::{self, Event, Pty};
use crate::theme::{Terminal, Theme};
use crate::views::compose;
use crate::views::inputs::Inputs;
use crate::vt::view::Picked;
use crate::vt::{self, Key, KeyKind, Screen, Shape, Vt};

mod pointer;

const RESET: &[u8] = b"\x1bc";
const SCALE_ENV: &str = "LOOM_DESKTOP_SCALE";
const LINE_HEIGHT: f32 = 1.3;
const MARKED_BACKGROUND: vt::Rgb8 = [0x3a, 0x3a, 0x55];
const READABLE_PX: f32 = 6.0;
const SNAPSHOT_GAP: Duration = Duration::from_millis(250);
const TYPING_HOLD: Duration = Duration::from_secs(1);
const START: (u16, u16) = (80, 24);
const CURSOR_ALPHA: f32 = 0.6;
const CURSOR_THIN: f32 = 2.0;
const PAD_X: f32 = 10.0;
const PAD_Y: f32 = 8.0;
const FIT_SLACK: f32 = 0.001;
const SCROLLBAR: f32 = 14.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputState {
    Open,
    Offline,
    Gone,
}

#[derive(Clone, Copy)]
struct Metrics {
    cell_width: Pixels,
    cell_height: Pixels,
}

#[derive(Clone)]
struct Style {
    face: Face,
    terminal: &'static Terminal,
    palette: vt::Palette,
}

struct Draw {
    style: Style,
    scale: f32,
    focused: bool,
    touched: bool,
}

impl Style {
    fn current(cx: &App) -> Style {
        let theme = cx.global::<Theme>();
        Style {
            face: Theme::face(cx),
            terminal: theme.terminal,
            palette: palette(theme.terminal),
        }
    }
}

fn byte(c: Rgba) -> vt::Rgb8 {
    [c.r, c.g, c.b].map(|v| (v * 255.0).round() as u8)
}

fn palette(terminal: &Terminal) -> vt::Palette {
    vt::Palette {
        ansi: terminal.ansi.map(byte),
        foreground: byte(terminal.foreground),
        background: byte(terminal.background),
        cursor: byte(terminal.cursor),
    }
}

struct Frame {
    screen: Screen,
    picked: Vec<Picked>,
}

pub struct TerminalView {
    vt: Vt,
    pty: Option<Pty>,
    focus: FocusHandle,
    settled: bool,
    scale: f32,
    area: Bounds<Pixels>,
    origin: gpui::Point<Pixels>,
    touched: bool,
    cell: Option<Metrics>,
    sent: (u16, u16),
    ended: Option<String>,
    gone: bool,
    marked: Option<String>,
    cursor: Option<vt::Cursor>,
    frame: Option<Rc<Frame>>,
    style: Style,
    dirty: bool,
    taken: Instant,
    typed: Option<Instant>,
    refresh: bool,
    picking: bool,
    reporting: Option<MouseButton>,
    last_cell: Option<(usize, usize)>,
    wheel_rest: f32,
    _focus_events: [Subscription; 2],
    _look: Subscription,
}

fn color([r, g, b]: vt::Rgb8) -> Rgba {
    rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
}

fn hsla(rgb: vt::Rgb8) -> Hsla {
    color(rgb).into()
}

fn cheap(face: &Face, scale: f32) -> bool {
    face.size * scale <= READABLE_PX
}

fn metrics(window: &Window, face: &Face, scale: f32) -> Metrics {
    let system = window.text_system();
    let id = system.resolve_font(&face.font);
    let em = px(face.size * scale);
    Metrics {
        cell_width: system.advance(id, em, 'm').map_or(em * 0.6, |a| a.width),
        cell_height: px((face.size * LINE_HEIGHT).ceil() * scale),
    }
}

pub fn open(
    inputs: &Entity<Inputs>,
    base: &str,
    node: u32,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TerminalView> {
    let view = open_with(link::attach, base, node, window, cx);
    crate::views::inputs::offer(inputs, node, &view, cx);
    view
}

pub fn open_login(
    base: &str,
    login: u32,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TerminalView> {
    open_with(link::attach_login, base, login, window, cx)
}

type Attach = fn(&str, u32, async_channel::Sender<Event>) -> Result<Pty, String>;

fn open_with(
    attach: Attach,
    base: &str,
    id: u32,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TerminalView> {
    let opened = (|| -> Result<_, String> {
        let (events, inbox) = async_channel::unbounded();
        Ok((attach(base, id, events)?, inbox))
    })();
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
        let style = Style::current(cx);
        let focus = cx.focus_handle();
        let focus_events = [
            cx.on_focus_in(&focus, window, |this, _, _| {
                this.touched = true;
                this.report_focus(true);
            }),
            cx.on_focus_out(&focus, window, |this, _, _, _| this.report_focus(false)),
        ];
        Self {
            vt: Vt::new(START.0, START.1, style.palette),
            pty,
            focus,
            settled: false,
            scale: std::env::var(SCALE_ENV)
                .ok()
                .and_then(|scale| scale.parse().ok())
                .unwrap_or(1.0),
            area: Bounds::default(),
            origin: gpui::Point::default(),
            touched: false,
            cell: None,
            sent: (0, 0),
            ended,
            gone: false,
            marked: None,
            cursor: None,
            frame: None,
            style,
            dirty: true,
            taken: Instant::now(),
            typed: None,
            refresh: false,
            picking: false,
            reporting: None,
            last_cell: None,
            wheel_rest: 0.0,
            _focus_events: focus_events,
            _look: cx.observe_global::<Theme>(|this, cx| this.restyle(cx)),
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    /// Focus that no human asked for: waits while an IME composition is open.
    pub fn refocus(&self, window: &mut Window, cx: &mut App) -> bool {
        compose::refocus(&self.focus, window, cx)
    }

    pub fn composing(&self) -> bool {
        self.marked.is_some()
    }

    fn set_marked(&mut self, marked: Option<String>, cx: &mut App) {
        let open = marked.is_some();
        if open || self.marked.is_some() {
            compose::mark(&self.focus, open, cx);
        }
        self.marked = marked;
    }

    pub fn focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    pub fn set_scale(&mut self, scale: f32) {
        if scale != self.scale {
            self.scale = scale;
            self.dirty = true;
        }
    }

    fn cheap(&self) -> bool {
        cheap(&self.style.face, self.scale)
    }

    fn restyle(&mut self, cx: &mut Context<Self>) {
        let style = Style::current(cx);
        if style.face == self.style.face && style.palette == self.style.palette {
            return;
        }
        self.vt.set_palette(style.palette);
        self.style = style;
        self.changed(cx);
        cx.notify();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        let wait = SNAPSHOT_GAP.saturating_sub(self.taken.elapsed());
        let typing = self.typed.is_some_and(|at| at.elapsed() < TYPING_HOLD);
        if !self.cheap() || typing || wait.is_zero() {
            cx.notify();
        } else if !self.refresh {
            self.refresh = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |view, cx| {
                    view.refresh = false;
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn frame(&mut self) -> Rc<Frame> {
        if let Some(frame) = self.frame.as_ref().filter(|_| !self.dirty) {
            return frame.clone();
        }
        let offset = self.vt.display_offset();
        let mut screen = self.vt.snapshot();
        let rows = screen.rows;
        screen.cursor = screen.cursor.and_then(|cursor| {
            let row = cursor.row + offset;
            (row < rows).then_some(vt::Cursor { row, ..cursor })
        });
        let frame = Rc::new(Frame {
            screen,
            picked: self.vt.picked(),
        });
        self.frame = Some(frame.clone());
        self.dirty = false;
        self.taken = Instant::now();
        frame
    }

    fn apply(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Size { cols, rows } => {
                self.vt.resize(cols, rows);
                self.claim();
            }
            Event::Settled => self.settled = true,
            Event::Reset => {
                self.vt.feed(RESET);
                self.settled = false;
                self.sent = (0, 0);
                self.ended = None;
                self.gone = false;
            }
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
            Event::Gone => {
                self.gone = true;
                self.ended = Some("the terminal is gone".to_owned());
            }
            Event::Closed(reason) => {
                self.ended = Some(if reason.is_empty() {
                    "connection closed".to_owned()
                } else {
                    reason
                });
            }
        }
        self.changed(cx);
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

    fn resized(&mut self, area: Bounds<Pixels>, cell: Metrics, origin: gpui::Point<Pixels>) {
        self.area = area;
        self.origin = origin;
        self.cell = Some(cell);
        self.claim();
    }

    fn claim(&mut self) {
        let (Some(cell), true) = (self.cell, self.area.size.width > px(0.0)) else {
            return;
        };
        let cols = ((self.area.size.width / self.scale - px(SCROLLBAR)) / cell.cell_width
            + FIT_SLACK)
            .floor() as u16;
        let rows =
            (self.area.size.height / self.scale / cell.cell_height + FIT_SLACK).floor() as u16;
        let fit = (cols.max(2), rows.max(1));
        if fit != self.sent {
            self.sent = fit;
            if let Some(pty) = &self.pty {
                pty.resize(fit.0, fit.1);
            }
        }
    }

    fn type_in(&mut self, bytes: Vec<u8>) {
        self.claim();
        self.vt.select_clear();
        self.vt.scroll_to_bottom();
        self.dirty = true;
        self.typed = Some(Instant::now());
        self.send(bytes);
    }

    pub fn input_state(&self) -> InputState {
        if self.gone {
            InputState::Gone
        } else if self.ended.is_some() || self.pty.is_none() {
            InputState::Offline
        } else {
            InputState::Open
        }
    }

    pub fn deliver(&mut self, text: &str, cx: &mut Context<Self>) {
        let mut bytes = self.vt.encode_paste(text);
        bytes.push(b'\r');
        self.type_in(bytes);
        self.changed(cx);
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.vt.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        self.vt.select_clear();
        self.changed(cx);
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let bytes = self.vt.encode_paste(&text);
            self.type_in(bytes);
            self.changed(cx);
        }
    }

    fn key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let stroke = &event.keystroke;
        let command = stroke.modifiers.secondary(cx);
        let copying =
            command && stroke.key == "c" && (stroke.modifiers.shift || self.vt.has_selection());
        if copying || (command && stroke.key == "v") {
            if copying {
                self.copy(cx);
            } else {
                self.paste(cx);
            }
            cx.stop_propagation();
            return;
        }
        let kind = if event.is_held {
            KeyKind::Repeat
        } else {
            KeyKind::Press
        };
        self.press_key(stroke, kind, cx);
    }

    fn press_key(&mut self, stroke: &Keystroke, kind: KeyKind, cx: &mut Context<Self>) {
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
        if let Some(bytes) = self.vt.encode_key_as(&key, kind) {
            if kind == KeyKind::Release {
                self.send(bytes);
            } else {
                self.type_in(bytes);
                self.changed(cx);
            }
            cx.stop_propagation();
        }
    }

    fn report_focus(&self, focused: bool) {
        if let Some(bytes) = self.vt.encode_focus(focused) {
            self.send(bytes);
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
        self.set_marked(None, cx);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_marked(None, cx);
        if !text.is_empty() {
            self.type_in(text.as_bytes().to_vec());
        }
        self.changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.marked.is_none()
            && !new_text.is_empty()
            && let Some(pty) = &self.pty
        {
            pty.composing();
        }
        self.set_marked((!new_text.is_empty()).then(|| new_text.to_owned()), cx);
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
            self.origin + point(width * cols as f32, height * cursor.row as f32),
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
        let frame = self.frame();
        self.cursor = frame.screen.cursor;
        let marked = self.marked.clone();
        let focus = self.focus.clone();
        let face = self.style.face.clone();
        let draw = Draw {
            style: self.style.clone(),
            scale: self.scale,
            focused: self.focus.is_focused(window),
            touched: self.touched,
        };
        let pad = point(px(PAD_X * self.scale), px(PAD_Y * self.scale));
        let grid = (frame.screen.cols, frame.screen.rows);
        let scale = self.scale;
        let view = cx.entity();
        let view_input = view.clone();
        let view_pointer = view.clone();
        div()
            .id("terminal")
            .size_full()
            .relative()
            .bg(self.style.terminal.background)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.key(event, cx)))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, cx| {
                this.press_key(&event.keystroke, KeyKind::Release, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.press(event, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, event, window, cx| this.press(event, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event, window, cx| this.press(event, window, cx)),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                this.wheel(event, window, cx);
            }))
            .child(
                canvas(
                    move |outer, window, cx| {
                        let bounds = Bounds::new(
                            outer.origin + pad,
                            outer.size - size(pad.x * 2.0, pad.y * 2.0),
                        );
                        let base = metrics(window, &face, 1.0);
                        let cell = metrics(window, &face, scale);
                        let spare = bounds.size
                            - size(
                                cell.cell_width * grid.0 as f32,
                                cell.cell_height * grid.1 as f32,
                            );
                        let origin = bounds.origin
                            + point(
                                spare.width.max(px(0.0)) / 2.0,
                                spare.height.max(px(0.0)) / 2.0,
                            );
                        view.update(cx, |view, _| view.resized(bounds, base, origin));
                        Bounds::new(origin, bounds.size)
                    },
                    move |_, bounds, window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, view_input.clone()),
                            cx,
                        );
                        let moving = view_pointer.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture {
                                moving.update(cx, |view, cx| view.drag(event, window, cx));
                            }
                        });
                        let releasing = view_pointer.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture {
                                releasing.update(cx, |view, cx| view.release(event, window, cx));
                            }
                        });
                        paint(bounds, &frame, marked.as_deref(), &draw, window, cx);
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
                    .text_color(self.style.terminal.foreground)
                    .child(text)
            }))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Detail {
    Full,
    Colour,
}

impl Detail {
    fn of(cheap: bool) -> Detail {
        if cheap { Detail::Colour } else { Detail::Full }
    }
}

fn styled(face: &Face, cell: &vt::Cell, len: usize, detail: Detail) -> TextRun {
    let (bold, italic, underline, strike) = match detail {
        Detail::Full => (cell.bold, cell.italic, cell.underline, cell.strike),
        Detail::Colour => (false, false, false, false),
    };
    let mut run_font = face.font.clone();
    if bold {
        run_font.weight = FontWeight::BOLD;
    }
    if italic {
        run_font.style = FontStyle::Italic;
    }
    let fg = hsla(cell.fg);
    TextRun {
        len,
        font: run_font,
        color: fg,
        background_color: None,
        underline: underline.then_some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(fg),
            wavy: false,
        }),
        strikethrough: strike.then_some(StrikethroughStyle {
            thickness: px(1.0),
            color: Some(fg),
        }),
    }
}

fn same_style(a: &vt::Cell, b: &vt::Cell, detail: Detail) -> bool {
    if detail == Detail::Colour {
        return b.ch == ' ' || a.fg == b.fg;
    }
    (a.fg, a.bold, a.italic, a.underline, a.strike)
        == (b.fg, b.bold, b.italic, b.underline, b.strike)
}

fn row_runs(face: &Face, cells: &[vt::Cell], detail: Detail) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = Vec::new();
    let mut first = 0;
    for (i, c) in cells.iter().enumerate() {
        if i > first && !same_style(&cells[first], c, detail) {
            let len = cells[first..i].iter().map(|c| c.ch.len_utf8()).sum();
            runs.push(styled(face, &cells[first], len, detail));
            first = i;
        }
    }
    let len = cells[first..].iter().map(|c| c.ch.len_utf8()).sum();
    runs.push(styled(face, &cells[first], len, detail));
    runs
}

fn paint(
    bounds: Bounds<Pixels>,
    frame: &Frame,
    marked: Option<&str>,
    draw: &Draw,
    window: &mut Window,
    cx: &mut App,
) {
    let Draw {
        style,
        scale,
        focused,
        touched,
    } = draw;
    let (scale, focused, touched) = (*scale, *focused, *touched);
    let face = &style.face;
    let screen = &frame.screen;
    let cheap = cheap(face, scale);
    let cell = metrics(window, face, scale);
    let em = px(face.size * scale);
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
            if cells[start].bg != style.palette.background {
                window.paint_quad(fill(
                    Bounds::new(
                        at(start, row),
                        size(cell.cell_width * (col - start) as f32, cell.cell_height),
                    ),
                    hsla(cells[start].bg),
                ));
            }
        }
        for picked in frame.picked.iter().filter(|p| p.row == row) {
            window.paint_quad(fill(
                Bounds::new(
                    at(picked.from, row),
                    size(
                        cell.cell_width * (picked.to + 1 - picked.from) as f32,
                        cell.cell_height,
                    ),
                ),
                style.terminal.selection_background,
            ));
        }
        if cells
            .iter()
            .all(|c| c.ch == ' ' && !c.underline && !c.strike)
        {
            continue;
        }
        let marked_cells;
        let cells = match frame
            .picked
            .iter()
            .filter(|p| p.row == row)
            .collect::<Vec<_>>()
        {
            picks if picks.is_empty() => cells,
            picks => {
                let mut owned = cells.to_vec();
                let ink = byte(style.terminal.selection_foreground);
                for pick in picks {
                    let to = pick.to.min(owned.len().saturating_sub(1));
                    for cell in owned.iter_mut().take(to + 1).skip(pick.from) {
                        cell.fg = ink;
                    }
                }
                marked_cells = owned;
                &marked_cells[..]
            }
        };
        let text: String = cells.iter().map(|c| c.ch).collect();
        let runs = row_runs(face, cells, Detail::of(cheap));
        let line = window.text_system().shape_line(
            text.into(),
            em,
            &runs,
            (!cheap).then_some(cell.cell_width),
        );
        let _ = line.paint(
            at(0, row),
            cell.cell_height,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
    if let Some(cursor) = screen.cursor.filter(|_| touched) {
        let mut hue: Hsla = style.terminal.cursor.into();
        hue.a = CURSOR_ALPHA;
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
        let bounds = Bounds::new(origin, extent);
        if focused || cursor.shape != Shape::Block {
            window.paint_quad(fill(bounds, hue));
        } else {
            let mut hollow = outline(bounds, hue, BorderStyle::Solid);
            hollow.border_widths = px(scale).into();
            window.paint_quad(hollow);
        }
        if let Some(text) = marked {
            paint_marked(text, cursor, &at, cell, draw, window, cx);
        }
    }
}

fn paint_marked(
    text: &str,
    cursor: vt::Cursor,
    at: &dyn Fn(usize, usize) -> gpui::Point<Pixels>,
    cell: Metrics,
    draw: &Draw,
    window: &mut Window,
    cx: &mut App,
) {
    let style = &draw.style;
    let em = px(style.face.size * draw.scale);
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
            fg: style.palette.foreground,
            bg: MARKED_BACKGROUND,
            bold: false,
            italic: false,
            underline: true,
            strike: false,
        };
        let run = styled(&style.face, &mark, c.len_utf8(), Detail::Full);
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
