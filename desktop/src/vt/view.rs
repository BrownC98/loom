use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{TermMode, viewport_to_point};

use super::Vt;

const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

#[derive(Clone, Copy)]
pub enum Grain {
    Cell,
    Word,
    Line,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Wheel {
    Report,
    Arrows { app_cursor: bool },
    History,
}

pub struct Picked {
    pub row: usize,
    pub from: usize,
    pub to: usize,
}

impl Vt {
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub fn scroll_display(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn wheel(&self) -> Wheel {
        let mode = self.term.mode();
        if mode.intersects(TermMode::MOUSE_MODE) {
            Wheel::Report
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            Wheel::Arrows {
                app_cursor: mode.contains(TermMode::APP_CURSOR),
            }
        } else {
            Wheel::History
        }
    }

    pub fn paste_bytes(&self, text: &str) -> Vec<u8> {
        let lines = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.term.mode().contains(TermMode::BRACKETED_PASTE) {
            format!("{PASTE_START}{lines}{PASTE_END}").into_bytes()
        } else {
            lines.into_bytes()
        }
    }

    pub fn select_start(&mut self, grain: Grain, col: usize, row: usize, right: bool) {
        let kind = match grain {
            Grain::Cell => SelectionType::Simple,
            Grain::Word => SelectionType::Semantic,
            Grain::Line => SelectionType::Lines,
        };
        let point = self.grid_point(col, row);
        self.term.selection = Some(Selection::new(kind, point, side(right)));
    }

    pub fn select_extend(&mut self, col: usize, row: usize, right: bool) {
        let point = self.grid_point(col, row);
        if let Some(selection) = &mut self.term.selection {
            selection.update(point, side(right));
        }
    }

    pub fn select_end(&mut self) {
        if self
            .term
            .selection
            .as_ref()
            .is_some_and(Selection::is_empty)
        {
            self.term.selection = None;
        }
    }

    pub fn select_clear(&mut self) {
        self.term.selection = None;
    }

    pub fn has_selection(&self) -> bool {
        self.term
            .selection
            .as_ref()
            .is_some_and(|selection| !selection.is_empty())
    }

    pub fn selected_text(&self) -> Option<String> {
        self.term
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    pub fn picked(&self) -> Vec<Picked> {
        let Some(range) = self
            .term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&self.term))
        else {
            return Vec::new();
        };
        let (offset, cols, rows) = (
            self.display_offset() as i32,
            self.term.columns(),
            self.term.screen_lines() as i32,
        );
        let first = (range.start.line.0 + offset).max(0);
        let last = (range.end.line.0 + offset).min(rows - 1);
        (first..=last)
            .map(|row| {
                let line = Line(row - offset);
                Picked {
                    row: row as usize,
                    from: if line == range.start.line {
                        range.start.column.0
                    } else {
                        0
                    },
                    to: if line == range.end.line {
                        range.end.column.0
                    } else {
                        cols - 1
                    },
                }
            })
            .collect()
    }

    fn grid_point(&self, col: usize, row: usize) -> Point {
        viewport_to_point(self.display_offset(), Point::new(row, Column(col)))
    }
}

fn side(right: bool) -> Side {
    if right { Side::Right } else { Side::Left }
}
