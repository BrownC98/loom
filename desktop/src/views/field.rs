use std::ops::Range;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, BoxShadow, ClipboardEntry, ClipboardItem, Context, CursorStyle, Div,
    Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, HighlightStyle, InspectorElementId, InteractiveElement,
    IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, Rgba, SharedString, Styled, StyledText, TextLayout,
    UTF16Selection, UnderlineStyle, Window, div, fill, point, px, size,
};

use crate::fonts::TERMINAL_FAMILY;
use crate::strings::{Lang, Str};
use crate::theme::{Colors, Theme};

const MASK: char = '\u{2022}';
const FONT_SIZE: f32 = 13.0;
const LINE: f32 = 18.0;
const FORM_HEIGHT: f32 = 35.0;
const FORM_LINE: f32 = 21.0;
const FORM_FONT: f32 = 14.0;
const FORM_RING: f32 = 2.0;
const NARROW_HEIGHT: f32 = 26.0;
const CARET: f32 = 1.5;
const ROUND: f32 = 6.0;

pub enum FieldEvent {
    Changed,
    Step(isize),
    Enter,
    Escape,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Chat,
    Plain,
    Mono,
    Narrow,
    Stepper,
}

enum Hint {
    Label(Str),
    Text(SharedString),
}

type Files = Box<dyn Fn(Vec<ClipboardEntry>, &mut Window, &mut App)>;

pub struct Field {
    focus: FocusHandle,
    text: String,
    hint: Hint,
    secret: bool,
    multiline: bool,
    boxed: bool,
    form: Form,
    scale: f32,
    tone: Option<Rgba>,
    files: Option<Files>,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    layout: Option<TextLayout>,
    selecting: bool,
    locked: bool,
}

impl EventEmitter<FieldEvent> for Field {}

impl Focusable for Field {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

pub fn new(placeholder: Str, cx: &mut App) -> Entity<Field> {
    cx.new(|cx| Field::new(Hint::Label(placeholder), false, false, false, cx))
}

pub fn boxed(hint: &str, secret: bool, multiline: bool, cx: &mut App) -> Entity<Field> {
    let hint = Hint::Text(SharedString::from(hint.to_owned()));
    cx.new(|cx| Field::new(hint, secret, multiline, true, cx))
}

pub fn boxed_label(label: Str, secret: bool, multiline: bool, cx: &mut App) -> Entity<Field> {
    cx.new(|cx| Field::new(Hint::Label(label), secret, multiline, true, cx))
}

pub fn form(hint: &str, secret: bool, multiline: bool, kind: Form, cx: &mut App) -> Entity<Field> {
    let hint = Hint::Text(SharedString::from(hint.to_owned()));
    cx.new(|cx| {
        let mut field = Field::new(hint, secret, multiline, true, cx);
        field.form = kind;
        field
    })
}

impl Field {
    fn new(hint: Hint, secret: bool, multiline: bool, boxed: bool, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            text: String::new(),
            hint,
            secret,
            multiline,
            boxed,
            form: Form::Chat,
            scale: 1.0,
            tone: None,
            files: None,
            selected: 0..0,
            reversed: false,
            marked: None,
            layout: None,
            selecting: false,
            locked: false,
        }
    }

    pub fn set_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        if self.locked != locked {
            self.locked = locked;
            cx.notify();
        }
    }

    pub fn set_scale(&mut self, scale: f32, cx: &mut Context<Self>) {
        if self.scale != scale {
            self.scale = scale;
            cx.notify();
        }
    }

    pub fn set_tone(&mut self, tone: Rgba, cx: &mut Context<Self>) {
        if self.tone != Some(tone) {
            self.tone = Some(tone);
            cx.notify();
        }
    }

    pub fn focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    pub fn handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.replace_all(text, 0..text.len(), cx);
    }

    pub fn set_text_at_end(&mut self, text: &str, cx: &mut Context<Self>) {
        self.replace_all(text, text.len()..text.len(), cx);
    }

    fn replace_all(&mut self, text: &str, selected: Range<usize>, cx: &mut Context<Self>) {
        self.text = text.to_owned();
        self.selected = selected;
        self.reversed = false;
        self.marked = None;
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    pub fn on_files(
        &mut self,
        handler: impl Fn(Vec<ClipboardEntry>, &mut Window, &mut App) + 'static,
    ) {
        self.files = Some(Box::new(handler));
    }

    pub fn insert_token(&mut self, token: &str, window: &Window, cx: &mut Context<Self>) {
        let at = self.selected.start.min(self.text.len());
        let lead = at > 0 && !self.text[..at].ends_with(char::is_whitespace);
        let typed = format!("{}{token} ", if lead { " " } else { "" });
        self.put(at..at, &typed, window, cx);
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.text.clear();
        self.selected = 0..0;
        self.reversed = false;
        self.marked = None;
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn shown(&self) -> String {
        if self.secret {
            MASK.to_string().repeat(self.text.chars().count())
        } else {
            self.text.clone()
        }
    }

    fn shown_at(&self, at: usize) -> usize {
        if self.secret {
            self.text[..at.min(self.text.len())].chars().count() * MASK.len_utf8()
        } else {
            at
        }
    }

    fn real_at(&self, at: usize) -> usize {
        if self.secret {
            self.text
                .char_indices()
                .nth(at / MASK.len_utf8())
                .map_or(self.text.len(), |(index, _)| index)
        } else {
            at.min(self.text.len())
        }
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    fn before(&self, at: usize) -> usize {
        self.text[..at]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn after(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map_or(at, |next| at + next.len_utf8())
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |index| index + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |index| at + index)
    }

    fn move_to(&mut self, at: usize, window: &Window, cx: &mut Context<Self>) {
        self.selected = at..at;
        self.reversed = false;
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    fn extend_to(&mut self, at: usize, window: &Window, cx: &mut Context<Self>) {
        if self.reversed {
            self.selected.start = at;
        } else {
            self.selected.end = at;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    fn go(&mut self, at: usize, extend: bool, window: &Window, cx: &mut Context<Self>) {
        if extend {
            self.extend_to(at, window, cx);
        } else {
            self.move_to(at, window, cx);
        }
    }

    fn row(&self, rows: f32) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let from = layout.position_for_index(self.shown_at(self.cursor()))?;
        let target = point(from.x, from.y + layout.line_height() * rows);
        let found = layout
            .index_for_position(target)
            .unwrap_or_else(|near| near);
        Some(self.real_at(found))
    }

    fn at_mouse(&self, position: Point<Pixels>) -> usize {
        let Some(layout) = self.layout.as_ref() else {
            return 0;
        };
        if self.text.is_empty() {
            return 0;
        }
        let found = layout
            .index_for_position(position)
            .unwrap_or_else(|near| near);
        self.real_at(found)
    }

    fn put(&mut self, range: Range<usize>, text: &str, window: &Window, cx: &mut Context<Self>) {
        let text = if self.multiline {
            text.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            text.replace(['\r', '\n'], " ")
        };
        self.text.replace_range(range.clone(), &text);
        let end = range.start + text.len();
        self.selected = end..end;
        self.reversed = false;
        self.marked = None;
        window.notify_text_input_changed(&self.focus);
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn erase(&mut self, forward: bool, window: &Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let at = self.cursor();
            let other = if forward {
                self.after(at)
            } else {
                self.before(at)
            };
            if other == at {
                return;
            }
            self.selected = at.min(other)..at.max(other);
        }
        self.put(self.selected.clone(), "", window, cx);
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let stroke = &event.keystroke;
        let command = stroke.modifiers.secondary(cx);
        let shift = stroke.modifiers.shift;
        let other = stroke.modifiers.alt
            || (stroke.modifiers.control && !command)
            || (stroke.modifiers.platform && !command);
        if other || (self.marked.is_some() && !command) {
            return;
        }
        if self.locked && !matches!((command, stroke.key.as_str()), (true, "a" | "c")) {
            return;
        }
        let handled = match (command, stroke.key.as_str()) {
            (true, "a") => {
                self.selected = 0..self.text.len();
                self.reversed = false;
                cx.notify();
                true
            }
            (true, "c") => {
                self.copy(cx);
                true
            }
            (true, "x") => {
                self.copy(cx);
                if !self.selected.is_empty() {
                    self.put(self.selected.clone(), "", window, cx);
                }
                true
            }
            (true, "v") => {
                if let Some(item) = cx.read_from_clipboard() {
                    let files: Vec<ClipboardEntry> = item
                        .entries()
                        .iter()
                        .filter(|entry| !matches!(entry, ClipboardEntry::String(_)))
                        .cloned()
                        .collect();
                    if let (false, Some(take)) = (files.is_empty(), &self.files) {
                        take(files, window, cx);
                    } else if let Some(pasted) = item.text() {
                        self.put(self.selected.clone(), &pasted, window, cx);
                    }
                }
                true
            }
            (true, _) => false,
            (false, "backspace") => {
                self.erase(false, window, cx);
                true
            }
            (false, "delete") => {
                self.erase(true, window, cx);
                true
            }
            (false, "left") => {
                let at = if self.selected.is_empty() || shift {
                    self.before(self.cursor())
                } else {
                    self.selected.start
                };
                self.go(at, shift, window, cx);
                true
            }
            (false, "right") => {
                let at = if self.selected.is_empty() || shift {
                    self.after(self.cursor())
                } else {
                    self.selected.end
                };
                self.go(at, shift, window, cx);
                true
            }
            (false, "home") => {
                self.go(self.line_start(self.cursor()), shift, window, cx);
                true
            }
            (false, "end") => {
                self.go(self.line_end(self.cursor()), shift, window, cx);
                true
            }
            (false, "up") | (false, "down") if self.multiline => {
                let rows = if stroke.key == "up" { -1.0 } else { 1.0 };
                if let Some(at) = self.row(rows) {
                    self.go(at, shift, window, cx);
                }
                true
            }
            (false, "enter") if self.multiline && shift => {
                self.put(self.selected.clone(), "\n", window, cx);
                true
            }
            (false, "up") => {
                cx.emit(FieldEvent::Step(-1));
                true
            }
            (false, "down") => {
                cx.emit(FieldEvent::Step(1));
                true
            }
            (false, "enter") if shift => {
                cx.emit(FieldEvent::Step(-1));
                true
            }
            (false, "enter") => {
                cx.emit(FieldEvent::Enter);
                true
            }
            (false, "escape") => {
                cx.emit(FieldEvent::Escape);
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    fn copy(&self, cx: &mut Context<Self>) {
        if !self.selected.is_empty() && !self.secret {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text[self.selected.clone()].to_owned(),
            ));
        }
    }

    fn press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.selecting = true;
        let at = self.at_mouse(event.position);
        self.go(at, event.modifiers.shift, window, cx);
    }

    fn drag(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && event.dragging() {
            let at = self.at_mouse(event.position);
            self.extend_to(at, window, cx);
        } else {
            self.selecting = false;
        }
    }

    fn utf16_to_byte(&self, offset: usize) -> usize {
        let mut bytes = 0;
        let mut units = 0;
        for c in self.text.chars() {
            if units >= offset {
                break;
            }
            units += c.len_utf16();
            bytes += c.len_utf8();
        }
        bytes
    }

    fn byte_to_utf16(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())]
            .chars()
            .map(char::len_utf16)
            .sum()
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.utf16_to_byte(range.start)..self.utf16_to_byte(range.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.byte_to_utf16(range.start)..self.byte_to_utf16(range.end)
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
        let bytes = self.range_from_utf16(&range);
        let units = self.range_to_utf16(&bytes);
        let text = if self.secret {
            MASK.to_string().repeat(units.len())
        } else {
            self.text[bytes].to_owned()
        };
        *adjusted = Some(units);
        Some(text)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected),
            reversed: self.reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.locked {
            return;
        }
        let range = range
            .map(|range| self.range_from_utf16(&range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        self.put(range, text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.locked {
            return;
        }
        let range = range
            .map(|range| self.range_from_utf16(&range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        let start = range.start;
        self.text.replace_range(range, text);
        self.marked = (!text.is_empty()).then(|| start..start + text.len());
        let start_units = self.byte_to_utf16(start);
        let length = text.encode_utf16().count();
        self.selected = selected
            .map(|inside| {
                self.range_from_utf16(
                    &(start_units + inside.start.min(length)..start_units + inside.end.min(length)),
                )
            })
            .unwrap_or(start + text.len()..start + text.len());
        self.reversed = false;
        window.notify_text_input_changed(&self.focus);
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
        let layout = self.layout.as_ref()?;
        let bytes = self.range_from_utf16(&range);
        let from = layout.position_for_index(self.shown_at(bytes.start))?;
        let to = layout
            .position_for_index(self.shown_at(bytes.end))
            .unwrap_or(from);
        let width = if to.y == from.y {
            to.x - from.x
        } else {
            px(CARET)
        };
        Some(Bounds::new(
            point(from.x, from.y.max(bounds.top())),
            size(width, layout.line_height()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let found = layout
            .index_for_position(position)
            .unwrap_or_else(|near| near);
        Some(self.byte_to_utf16(self.real_at(found)))
    }
}

struct FieldText {
    field: Entity<Field>,
    text: StyledText,
}

impl IntoElement for FieldText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for FieldText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.text.request_layout(id, inspector, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.text.prepaint(id, inspector, bounds, state, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.field.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.field.clone()),
            cx,
        );
        self.text
            .paint(id, inspector, bounds, state, prepaint, window, cx);
        let layout = self.text.layout().clone();
        if focus.is_focused(window) {
            let field = self.field.read(cx);
            if field.selected.is_empty()
                && let Some(at) = layout.position_for_index(field.shown_at(field.cursor()))
            {
                let color = cx.global::<Theme>().colors.accent;
                window.paint_quad(fill(
                    Bounds::new(at, size(px(CARET), layout.line_height())),
                    color,
                ));
            }
        }
        self.field
            .update(cx, |field, _| field.layout = Some(layout));
    }
}

pub fn skin(base: Div, z: f32, focused: bool, colors: &Colors) -> Div {
    base.w_full()
        .min_h(px((LINE + 10.0) * z))
        .px(px(8.0 * z))
        .py(px(4.0 * z))
        .rounded(px(ROUND * z))
        .border_1()
        .border_color(if focused {
            colors.accent
        } else {
            colors.line_strong
        })
        .bg(colors.surface)
        .text_color(colors.ink)
        .text_size(px(FONT_SIZE * z))
        .line_height(px(LINE * z))
}

impl Render for Field {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &cx.global::<Theme>().colors;
        let (ink, muted, selection) = (colors.ink, colors.ink_3, colors.accent_wash);
        let focused = self.focus.is_focused(window);
        let empty = self.text.is_empty();
        let shown = if empty {
            match &self.hint {
                Hint::Label(label) => label.text(*cx.global::<Lang>()).to_owned(),
                Hint::Text(text) => text.to_string(),
            }
        } else {
            self.shown()
        };
        let mut highlights = Vec::new();
        if empty {
            highlights.push((
                0..shown.len(),
                HighlightStyle {
                    color: Some(muted.into()),
                    ..Default::default()
                },
            ));
        } else if let Some(marked) = &self.marked {
            highlights.push((
                self.shown_at(marked.start)..self.shown_at(marked.end),
                HighlightStyle {
                    underline: Some(UnderlineStyle {
                        color: Some(ink.into()),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..Default::default()
                },
            ));
        } else if focused && !self.selected.is_empty() {
            highlights.push((
                self.shown_at(self.selected.start)..self.shown_at(self.selected.end),
                HighlightStyle {
                    background_color: Some(selection.into()),
                    ..Default::default()
                },
            ));
        }
        highlights.retain(|(range, _)| range.start < range.end);
        let base = div()
            .key_context("Field")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::press))
            .on_mouse_move(cx.listener(Self::drag));
        let text = FieldText {
            field: cx.entity(),
            text: StyledText::new(shown).with_highlights(highlights),
        };
        if self.boxed {
            let z = self.scale;
            let skinned = skin(base, z, focused, colors);
            let skinned = match self.tone.filter(|_| focused) {
                Some(tone) => skinned.border_color(tone),
                None => skinned,
            };
            let ring = BoxShadow {
                color: colors.accent_wash.into(),
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: px(FORM_RING * z),
                inset: false,
            };
            let web = |el: Div| {
                el.min_h(px(FORM_HEIGHT * z))
                    .px(px(9.0 * z))
                    .py(px(6.0 * z))
                    .text_size(px(FORM_FONT * z))
                    .line_height(px(FORM_LINE * z))
                    .bg(colors.bg)
                    .when(focused, |el| el.shadow(vec![ring.clone()]))
            };
            let narrow = |el: Div| {
                el.min_h(px(NARROW_HEIGHT * z))
                    .px(px(8.0 * z))
                    .py_0()
                    .text_size(px(FORM_FONT * z))
                    .line_height(px((NARROW_HEIGHT - 2.0) * z))
                    .bg(colors.bg)
                    .when(focused, |el| el.shadow(vec![ring.clone()]))
            };
            match self.form {
                Form::Chat => skinned,
                Form::Plain => web(skinned),
                Form::Mono => web(skinned).font_family(TERMINAL_FAMILY),
                Form::Narrow => narrow(skinned)
                    .font_family(TERMINAL_FAMILY)
                    .text_size(px(12.0 * z)),
                Form::Stepper => narrow(skinned).px(px(2.0 * z)).flex().justify_center(),
            }
            .child(text)
        } else {
            base.flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(text)
        }
    }
}
