use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, Processor, Rgb};

pub type Rgb8 = [u8; 3];

const ANSI: [Rgb8; 16] = [
    [0x2e, 0x34, 0x36],
    [0xcc, 0x00, 0x00],
    [0x4e, 0x9a, 0x06],
    [0xc4, 0xa0, 0x00],
    [0x34, 0x65, 0xa4],
    [0x75, 0x50, 0x7b],
    [0x06, 0x98, 0x9a],
    [0xd3, 0xd7, 0xcf],
    [0x55, 0x57, 0x53],
    [0xef, 0x29, 0x29],
    [0x8a, 0xe2, 0x34],
    [0xfc, 0xe9, 0x4f],
    [0x72, 0x9f, 0xcf],
    [0xad, 0x7f, 0xa8],
    [0x34, 0xe2, 0xe2],
    [0xee, 0xee, 0xec],
];
pub const FOREGROUND: Rgb8 = [0xd4, 0xd4, 0xd4];
pub const BACKGROUND: Rgb8 = [0x0a, 0x0a, 0x0a];
pub const CURSOR: Rgb8 = [0xfa, 0xfa, 0xfa];
const FOREGROUND_INDEX: usize = 256;
const BACKGROUND_INDEX: usize = 257;
const CUBE_START: usize = 16;
const GRAY_START: usize = 232;
const DIM_START: usize = 259;

fn palette(index: usize) -> Rgb8 {
    match index {
        0..16 => ANSI[index],
        CUBE_START..GRAY_START => {
            let n = index - CUBE_START;
            let level = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
            [level(n / 36), level(n / 6 % 6), level(n % 6)]
        }
        GRAY_START..256 => {
            let v = ((index - GRAY_START) * 10 + 8) as u8;
            [v, v, v]
        }
        FOREGROUND_INDEX => FOREGROUND,
        BACKGROUND_INDEX => BACKGROUND,
        DIM_START..267 => ANSI[index - DIM_START].map(|v| (u16::from(v) * 2 / 3) as u8),
        258 => CURSOR,
        _ => FOREGROUND,
    }
}

fn resolve(color: Color, bold: bool) -> Rgb8 {
    match color {
        Color::Spec(Rgb { r, g, b }) => [r, g, b],
        Color::Indexed(i) => palette(usize::from(i)),
        Color::Named(name) => {
            let index = name as usize;
            palette(if bold && index < 8 { index + 8 } else { index })
        }
    }
}

#[derive(Default)]
struct Pending {
    replies: Vec<u8>,
}

#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Pending>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let mut pending = self.0.lock().expect("terminal events lock");
        match event {
            Event::PtyWrite(text) => pending.replies.extend(text.into_bytes()),
            Event::ColorRequest(index, format) => {
                let [r, g, b] = palette(index);
                pending.replies.extend(format(Rgb { r, g, b }).into_bytes());
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Block,
    Underline,
    Beam,
}

#[derive(Clone, Copy)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
    pub shape: Shape,
}

#[derive(Clone)]
pub struct Cell {
    pub ch: char,
    pub fg: Rgb8,
    pub bg: Rgb8,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub cursor: Option<Cursor>,
}

impl Screen {
    pub fn row(&self, row: usize) -> &[Cell] {
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }
}

pub struct Key<'a> {
    pub name: &'a str,
    pub text: Option<&'a str>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

pub struct Vt {
    term: Term<Listener>,
    parser: Processor,
    listener: Listener,
}

impl Vt {
    pub fn new(cols: u16, rows: u16) -> Self {
        let listener = Listener::default();
        Self {
            term: Term::new(
                Config {
                    kitty_keyboard: true,
                    ..Config::default()
                },
                &size(cols, rows),
                listener.clone(),
            ),
            parser: Processor::default(),
            listener,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(size(cols, rows));
    }

    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(
            &mut self
                .listener
                .0
                .lock()
                .expect("terminal events lock")
                .replies,
        )
    }

    pub fn snapshot(&self) -> Screen {
        let (cols, rows) = (self.term.columns(), self.term.screen_lines());
        let grid = self.term.grid();
        let mut cells = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                let cell = &grid[Line(row as i32)][Column(col)];
                let bold = cell.flags.contains(Flags::BOLD);
                let (mut fg, mut bg) = (resolve(cell.fg, bold), resolve(cell.bg, false));
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let blank = cell.flags.intersects(
                    Flags::HIDDEN | Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER,
                );
                cells.push(Cell {
                    ch: if blank { ' ' } else { cell.c },
                    fg,
                    bg,
                    bold,
                    italic: cell.flags.contains(Flags::ITALIC),
                    underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                    strike: cell.flags.contains(Flags::STRIKEOUT),
                });
            }
        }
        let cursor = self.term.renderable_content().cursor;
        let shape = match cursor.shape {
            CursorShape::Hidden => None,
            CursorShape::Underline => Some(Shape::Underline),
            CursorShape::Beam => Some(Shape::Beam),
            _ => Some(Shape::Block),
        };
        Screen {
            cols,
            rows,
            cells,
            cursor: shape
                .filter(|_| self.term.mode().contains(TermMode::SHOW_CURSOR))
                .and_then(|shape| {
                    let row = usize::try_from(cursor.point.line.0).ok()?;
                    (row < rows).then_some(Cursor {
                        col: cursor.point.column.0,
                        row,
                        shape,
                    })
                }),
        }
    }
}

#[allow(dead_code)]
impl Vt {
    pub fn encode_key(&self, key: &Key) -> Option<Vec<u8>> {
        self.encode_key_as(key, KeyKind::Press)
    }

    pub fn encode_key_as(&self, key: &Key, kind: KeyKind) -> Option<Vec<u8>> {
        let mode = *self.term.mode();
        let mods = 1 + u8::from(key.shift) + 2 * u8::from(key.alt) + 4 * u8::from(key.ctrl);
        let kitty =
            mode.intersects(TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC);
        if kitty && let Some(text) = kitty_key(mode, key, mods, kind) {
            return Some(text.into_bytes());
        }
        if kind == KeyKind::Release {
            return None;
        }
        legacy_key(mode.contains(TermMode::APP_CURSOR), key, mods)
    }

    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        let lines = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.term.mode().contains(TermMode::BRACKETED_PASTE) {
            format!("\x1b[200~{lines}\x1b[201~").into_bytes()
        } else {
            lines.into_bytes()
        }
    }

    pub fn encode_focus(&self, focused: bool) -> Option<Vec<u8>> {
        self.term.mode().contains(TermMode::FOCUS_IN_OUT).then(|| {
            if focused {
                b"\x1b[I".to_vec()
            } else {
                b"\x1b[O".to_vec()
            }
        })
    }

    pub fn mouse_reporting(&self) -> bool {
        self.term.mode().intersects(TermMode::MOUSE_MODE)
    }

    pub fn encode_mouse(&self, mouse: &Mouse) -> Option<Vec<u8>> {
        let mode = *self.term.mode();
        let (base, release) = match mouse.action {
            MouseAction::Press(button) => (button as u8, false),
            MouseAction::Release(button) => (button as u8, true),
            MouseAction::Drag(button) => (button as u8 + 32, false),
            MouseAction::Move => (3 + 32, false),
            MouseAction::Wheel { up, .. } => (if up { 64 } else { 65 }, false),
        };
        let wanted = match mouse.action {
            MouseAction::Press(_) | MouseAction::Release(_) | MouseAction::Wheel { .. } => {
                TermMode::MOUSE_MODE
            }
            MouseAction::Drag(_) => TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION,
            MouseAction::Move => TermMode::MOUSE_MOTION,
        };
        if !mode.intersects(wanted) {
            return self.alternate_scroll(&mode, mouse);
        }
        let repeat = match mouse.action {
            MouseAction::Wheel { lines, .. } => lines.max(1),
            _ => 1,
        };
        let sgr = mode.contains(TermMode::SGR_MOUSE);
        let flags = 4 * u8::from(mouse.shift) + 8 * u8::from(mouse.alt) + 16 * u8::from(mouse.ctrl);
        let (col, row) = (mouse.col + 1, mouse.row + 1);
        let one = if sgr {
            let code = base + flags;
            format!(
                "\x1b[<{code};{col};{row}{}",
                if release { 'm' } else { 'M' }
            )
            .into_bytes()
        } else {
            let code = if release { 3 } else { base } + flags;
            let mut bytes = b"\x1b[M".to_vec();
            for value in [usize::from(code), col, row] {
                let value = value + 32;
                if mode.contains(TermMode::UTF8_MOUSE) {
                    let c = char::from_u32(value as u32).filter(|_| value < 2048)?;
                    bytes.extend(c.to_string().into_bytes());
                } else {
                    bytes.push(u8::try_from(value).ok().filter(|v| *v < 255)?);
                }
            }
            bytes
        };
        Some(one.repeat(repeat))
    }

    fn alternate_scroll(&self, mode: &TermMode, mouse: &Mouse) -> Option<Vec<u8>> {
        let MouseAction::Wheel { up, lines } = mouse.action else {
            return None;
        };
        if mode.intersects(TermMode::MOUSE_MODE)
            || !mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
        {
            return None;
        }
        let arrow = match (up, mode.contains(TermMode::APP_CURSOR)) {
            (true, true) => "\x1bOA",
            (true, false) => "\x1b[A",
            (false, true) => "\x1bOB",
            (false, false) => "\x1b[B",
        };
        Some(arrow.repeat(lines.max(1)).into_bytes())
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Press,
    Repeat,
    Release,
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left = 0,
    Middle = 1,
    Right = 2,
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press(MouseButton),
    Release(MouseButton),
    Drag(MouseButton),
    Move,
    Wheel { up: bool, lines: usize },
}

#[allow(dead_code)]
pub struct Mouse {
    pub action: MouseAction,
    pub col: usize,
    pub row: usize,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

fn single_char(name: &str) -> Option<char> {
    let mut chars = name.chars();
    chars.next().filter(|_| chars.next().is_none())
}

fn text_key(name: &str) -> Option<char> {
    if name == "space" {
        return Some(' ');
    }
    single_char(name).map(|c| c.to_lowercase().next().unwrap_or(c))
}

fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c {
        ' ' | '@' | '2' => 0,
        'a'..='z' | 'A'..='Z' | '[' | '\\' | ']' | '^' | '_' => c as u8 & 0x1f,
        '3' => 27,
        '4' => 28,
        '5' => 29,
        '6' => 30,
        '7' | '/' => 31,
        '8' | '?' => 127,
        _ => return None,
    })
}

fn function_key(name: &str) -> Option<(u32, char)> {
    Some(match name {
        "up" => (1, 'A'),
        "down" => (1, 'B'),
        "right" => (1, 'C'),
        "left" => (1, 'D'),
        "home" => (1, 'H'),
        "end" => (1, 'F'),
        "insert" => (2, '~'),
        "delete" => (3, '~'),
        "pageup" => (5, '~'),
        "pagedown" => (6, '~'),
        _ => {
            let n: u32 = name.strip_prefix('f')?.parse().ok()?;
            match n {
                1 => (1, 'P'),
                2 => (1, 'Q'),
                3 => (1, 'R'),
                4 => (1, 'S'),
                5 => (15, '~'),
                6 => (17, '~'),
                7 => (18, '~'),
                8 => (19, '~'),
                9 => (20, '~'),
                10 => (21, '~'),
                11 => (23, '~'),
                12 => (24, '~'),
                13 => (25, '~'),
                14 => (26, '~'),
                15 => (28, '~'),
                16 => (29, '~'),
                17 => (31, '~'),
                18 => (32, '~'),
                19 => (33, '~'),
                20 => (34, '~'),
                _ => return None,
            }
        }
    })
}

fn legacy_key(app_cursor: bool, key: &Key, mods: u8) -> Option<Vec<u8>> {
    let alt = |mut bytes: Vec<u8>| {
        if key.alt {
            bytes.insert(0, 0x1b);
        }
        bytes
    };
    if let Some((number, fin)) = function_key(key.name) {
        let text = if mods > 1 {
            if fin == '~' {
                format!("\x1b[{number};{mods}~")
            } else {
                format!("\x1b[1;{mods}{fin}")
            }
        } else if fin == '~' {
            format!("\x1b[{number}~")
        } else if app_cursor || matches!(fin, 'P'..='S') {
            format!("\x1bO{fin}")
        } else {
            format!("\x1b[{fin}")
        };
        return Some(text.into_bytes());
    }
    match key.name {
        "enter" if key.shift && !key.ctrl => return Some(alt(b"\n".to_vec())),
        "enter" if key.ctrl => return Some(alt(b"\n".to_vec())),
        "enter" => return Some(alt(b"\r".to_vec())),
        "backspace" if key.ctrl => return Some(alt(vec![0x08])),
        "backspace" => return Some(alt(vec![0x7f])),
        "escape" => return Some(alt(vec![0x1b])),
        "tab" if key.shift => return Some(alt(b"\x1b[Z".to_vec())),
        "tab" if key.ctrl => return None,
        "tab" => return Some(alt(b"\t".to_vec())),
        _ => {}
    }
    if key.ctrl
        && let Some(byte) = text_key(key.name).and_then(ctrl_byte)
    {
        return Some(alt(vec![byte]));
    }
    if key.alt && !key.ctrl {
        let text = key
            .text
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                text_key(key.name).map(|c| {
                    if key.shift {
                        c.to_uppercase().collect()
                    } else {
                        c.to_string()
                    }
                })
            })?;
        return Some(alt(text.into_bytes()));
    }
    None
}

fn kitty_key(mode: TermMode, key: &Key, mods: u8, kind: KeyKind) -> Option<String> {
    let all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let events = mode.contains(TermMode::REPORT_EVENT_TYPES);
    if kind == KeyKind::Release && !events {
        return None;
    }
    let event = match kind {
        KeyKind::Press => 1,
        KeyKind::Repeat => 2,
        KeyKind::Release => 3,
    };
    let event = if events { event } else { 1 };
    let sequence = |number: u32, shifted: Option<u32>, fin: char, text: &str| {
        let mut out = String::from("\x1b[");
        let params = mods > 1 || event > 1 || !text.is_empty();
        if number != 1 || params || fin == '~' || fin == 'u' {
            out.push_str(&number.to_string());
        }
        if let Some(shifted) = shifted {
            out.push_str(&format!(":{shifted}"));
        }
        if params {
            out.push_str(&format!(";{mods}"));
            if event > 1 {
                out.push_str(&format!(":{event}"));
            }
            if !text.is_empty() {
                out.push_str(&format!(";{text}"));
            }
        }
        out.push(fin);
        out
    };
    if let Some((number, fin)) = function_key(key.name) {
        let (number, fin) = if key.name == "f3" {
            (13, '~')
        } else {
            (number, fin)
        };
        return Some(sequence(number, None, fin, ""));
    }
    let plain = |number: u32| (all || mods > 1).then(|| sequence(number, None, 'u', ""));
    match key.name {
        "escape" => return Some(sequence(27, None, 'u', "")),
        "enter" => return plain(13),
        "tab" => return plain(9),
        "backspace" => return plain(127),
        _ => {}
    }
    let c = text_key(key.name)?;
    if !key.ctrl && !key.alt {
        return None;
    }
    let shifted = (mode.contains(TermMode::REPORT_ALTERNATE_KEYS) && key.shift)
        .then(|| key.text.and_then(single_char))
        .flatten()
        .filter(|s| *s != c)
        .map(|s| s as u32);
    let text = if mode.contains(TermMode::REPORT_ASSOCIATED_TEXT) && all && kind != KeyKind::Release
    {
        key.text
            .filter(|_| !key.ctrl)
            .map(|t| {
                t.chars()
                    .map(|c| (c as u32).to_string())
                    .collect::<Vec<_>>()
                    .join(":")
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    Some(sequence(c as u32, shifted, 'u', &text))
}

fn size(cols: u16, rows: u16) -> TermSize {
    TermSize::new(usize::from(cols), usize::from(rows))
}
