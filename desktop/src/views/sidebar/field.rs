use std::ops::Range;

use gpui::{
    App, AppContext, Bounds, Context, CursorStyle, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement, Pixels, Render, ShapedLine, Styled, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, canvas, div, fill, point, px, size,
};

use crate::strings::{Lang, Str};
use crate::theme::Theme;

pub enum FieldEvent {
    Changed,
    Step(isize),
    Enter,
    Escape,
}

pub struct Field {
    focus: FocusHandle,
    text: String,
    cursor: usize,
    marked: Option<Range<usize>>,
    line: Option<ShapedLine>,
    bounds: Bounds<Pixels>,
}

impl EventEmitter<FieldEvent> for Field {}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn to_utf16(text: &str, offset: usize) -> usize {
    utf16_len(&text[..offset])
}

fn to_utf8(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        if units >= offset {
            return index;
        }
        units += ch.len_utf16();
    }
    text.len()
}

fn range_to_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    to_utf16(text, range.start)..to_utf16(text, range.end)
}

fn range_to_utf8(text: &str, range: &Range<usize>) -> Range<usize> {
    to_utf8(text, range.start)..to_utf8(text, range.end)
}

impl Field {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            text: String::new(),
            cursor: 0,
            marked: None,
            line: None,
            bounds: Bounds::default(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    pub fn handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.text.clear();
        self.cursor = 0;
        self.marked = None;
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn before(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn after(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.text.len(), |ch| self.cursor + ch.len_utf8())
    }

    fn key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let stroke = &event.keystroke;
        if stroke.modifiers.control || stroke.modifiers.alt || stroke.modifiers.platform {
            return;
        }
        let shift = stroke.modifiers.shift;
        match stroke.key.as_str() {
            "enter" if shift => cx.emit(FieldEvent::Step(-1)),
            "enter" => cx.emit(FieldEvent::Enter),
            "escape" => cx.emit(FieldEvent::Escape),
            "down" => cx.emit(FieldEvent::Step(1)),
            "up" => cx.emit(FieldEvent::Step(-1)),
            "left" => self.cursor = self.before(),
            "right" => self.cursor = self.after(),
            "home" => self.cursor = 0,
            "end" => self.cursor = self.text.len(),
            "backspace" if self.cursor > 0 => {
                let start = self.before();
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
                cx.emit(FieldEvent::Changed);
            }
            "delete" if self.cursor < self.text.len() => {
                let end = self.after();
                self.text.replace_range(self.cursor..end, "");
                cx.emit(FieldEvent::Changed);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if let Some(line) = &self.line {
            self.cursor = line
                .closest_index_for_x(event.position.x - self.bounds.origin.x)
                .min(self.text.len());
            cx.notify();
        }
    }
}

impl EntityInputHandler for Field {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = range_to_utf8(&self.text, &range);
        *adjusted = Some(range_to_utf16(&self.text, &range));
        Some(self.text[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let at = to_utf16(&self.text, self.cursor);
        Some(UTF16Selection {
            range: at..at,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|range| range_to_utf16(&self.text, range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| range_to_utf8(&self.text, &r))
            .or(self.marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        self.text.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
        self.marked = None;
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| range_to_utf8(&self.text, &r))
            .or(self.marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        self.text.replace_range(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        self.cursor = match selected {
            Some(inner) => range.start + to_utf8(text, inner.end),
            None => range.start + text.len(),
        };
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.line.as_ref()?;
        let range = range_to_utf8(&self.text, &range);
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(range.start), bounds.top()),
            point(bounds.left() + line.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        at: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let line = self.line.as_ref()?;
        let index = line.index_for_x(at.x - self.bounds.origin.x)?;
        Some(to_utf16(&self.text, index.min(self.text.len())))
    }
}

struct Laid {
    line: ShapedLine,
    caret: Pixels,
}

impl Render for Field {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &cx.global::<Theme>().colors;
        let (ink, hint) = (colors.ink, colors.ink_3);
        let placeholder = Str::FindOpen.text(*cx.global::<Lang>());
        let focused = self.focus.is_focused(window);
        let me: Entity<Field> = cx.entity();
        let paint_me = me.clone();
        let text = self.text.clone();
        let marked = self.marked.clone();
        let cursor = self.cursor;
        let focus = self.focus.clone();
        div()
            .id("find-field")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .cursor(CursorStyle::IBeam)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _, cx| this.key(event, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::press))
            .child(
                canvas(
                    move |bounds, window, _| {
                        let style = window.text_style();
                        let empty = text.is_empty();
                        let shown = if empty { placeholder } else { text.as_str() };
                        let run = TextRun {
                            len: shown.len(),
                            font: style.font(),
                            color: (if empty { hint } else { ink }).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let runs = match marked.as_ref().filter(|_| !empty) {
                            Some(range) => [
                                TextRun {
                                    len: range.start,
                                    ..run.clone()
                                },
                                TextRun {
                                    len: range.end - range.start,
                                    underline: Some(UnderlineStyle {
                                        color: Some(run.color),
                                        thickness: px(1.0),
                                        wavy: false,
                                    }),
                                    ..run.clone()
                                },
                                TextRun {
                                    len: shown.len() - range.end,
                                    ..run
                                },
                            ]
                            .into_iter()
                            .filter(|run| run.len > 0)
                            .collect(),
                            None => vec![run],
                        };
                        let font_size = style.font_size.to_pixels(window.rem_size());
                        let line = window.text_system().shape_line(
                            shown.to_owned().into(),
                            font_size,
                            &runs,
                            None,
                        );
                        let caret = if empty {
                            px(0.0)
                        } else {
                            line.x_for_index(cursor)
                        };
                        let _ = bounds;
                        Laid { line, caret }
                    },
                    move |bounds, laid, window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, paint_me.clone()),
                            cx,
                        );
                        let height = window.line_height();
                        let top = bounds.origin.y + (bounds.size.height - height) / 2.0;
                        let origin = point(bounds.origin.x, top);
                        let _ = laid
                            .line
                            .paint(origin, height, TextAlign::Left, None, window, cx);
                        if focused {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(origin.x + laid.caret, top),
                                    size(px(1.0), height),
                                ),
                                cx.global::<Theme>().colors.ink,
                            ));
                        }
                        paint_me.update(cx, |field, _| {
                            field.line = Some(laid.line);
                            field.bounds = Bounds::new(origin, bounds.size);
                        });
                    },
                )
                .size_full(),
            )
    }
}

pub fn new(cx: &mut App) -> Entity<Field> {
    cx.new(Field::new)
}
