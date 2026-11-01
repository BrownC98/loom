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
            term: Term::new(Config::default(), &size(cols, rows), listener.clone()),
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

    pub fn encode_key(&self, key: &Key) -> Option<Vec<u8>> {
        let app_cursor = self.term.mode().contains(TermMode::APP_CURSOR);
        let modifier = 1 + u8::from(key.shift) + 2 * u8::from(key.alt) + 4 * u8::from(key.ctrl);
        let arrow = |letter: char| {
            Some(if modifier > 1 {
                format!("\x1b[1;{modifier}{letter}")
            } else if app_cursor {
                format!("\x1bO{letter}")
            } else {
                format!("\x1b[{letter}")
            })
        };
        let tilde = |code: u8| {
            Some(if modifier > 1 {
                format!("\x1b[{code};{modifier}~")
            } else {
                format!("\x1b[{code}~")
            })
        };
        let named = match key.name {
            "enter" => Some("\r".to_owned()),
            "backspace" => Some("\x7f".to_owned()),
            "escape" => Some("\x1b".to_owned()),
            "tab" if key.shift => Some("\x1b[Z".to_owned()),
            "tab" => Some("\t".to_owned()),
            "up" => arrow('A'),
            "down" => arrow('B'),
            "right" => arrow('C'),
            "left" => arrow('D'),
            "home" => arrow('H'),
            "end" => arrow('F'),
            "insert" => tilde(2),
            "delete" => tilde(3),
            "pageup" => tilde(5),
            "pagedown" => tilde(6),
            _ => None,
        };
        if let Some(named) = named {
            return Some(named.into_bytes());
        }
        let mut chars = key.name.chars();
        let single = chars.next().filter(|_| chars.next().is_none());
        if key.ctrl
            && let Some(c) = single.or((key.name == "space").then_some(' '))
        {
            let byte = match c {
                ' ' | '@' | '2' => 0,
                'a'..='z' | 'A'..='Z' | '[' | '\\' | ']' | '^' | '_' => c as u8 & 0x1f,
                _ => return None,
            };
            let mut bytes = Vec::new();
            if key.alt {
                bytes.push(0x1b);
            }
            bytes.push(byte);
            return Some(bytes);
        }
        let text = key.text.filter(|text| !text.is_empty())?;
        let mut bytes = Vec::new();
        if key.alt {
            bytes.push(0x1b);
        }
        bytes.extend(text.as_bytes());
        Some(bytes)
    }
}

fn size(cols: u16, rows: u16) -> TermSize {
    TermSize::new(usize::from(cols), usize::from(rows))
}
