use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, AvailableSpace, Bounds, CursorStyle, DispatchPhase, Element, ElementId, FontId,
    GlobalElementId, HighlightStyle, Hitbox, HitboxBehavior, InspectorElementId, IntoElement,
    LayoutId, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, Size,
    TextRun, TextStyle, WhiteSpace, Window, WrappedLine,
};

type Marks = Vec<(Range<usize>, HighlightStyle)>;

struct Laid {
    lines: Vec<WrappedLine>,
    line_height: Pixels,
    wrap_width: Option<Pixels>,
    size: Size<Pixels>,
}

type Click = Rc<dyn Fn(usize, &mut Window, &mut App)>;

struct Clicks {
    id: ElementId,
    ranges: Vec<Range<usize>>,
    call: Click,
}

#[derive(Default)]
struct Press {
    down: Cell<Option<usize>>,
    over: Cell<bool>,
}

pub struct Prose {
    text: SharedString,
    marks: Marks,
    clicks: Option<Clicks>,
    laid: Rc<RefCell<Option<Laid>>>,
}

pub fn prose(text: impl Into<SharedString>) -> Prose {
    Prose {
        text: text.into(),
        marks: Vec::new(),
        clicks: None,
        laid: Rc::default(),
    }
}

impl Prose {
    pub fn with_highlights(
        mut self,
        marks: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    ) -> Self {
        self.marks = marks.into_iter().collect();
        self
    }

    pub fn on_click(
        mut self,
        id: ElementId,
        ranges: Vec<Range<usize>>,
        call: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.clicks = Some(Clicks {
            id,
            ranges,
            call: Rc::new(call),
        });
        self
    }
}

fn index_at(
    laid: &Rc<RefCell<Option<Laid>>>,
    bounds: Bounds<Pixels>,
    at: Point<Pixels>,
) -> Option<usize> {
    let laid = laid.borrow();
    let laid = laid.as_ref()?;
    let mut origin = bounds.origin;
    let mut start = 0;
    for line in &laid.lines {
        let bottom = origin.y + line.size(laid.line_height).height;
        if at.y > bottom {
            origin.y = bottom;
            start += line.len() + 1;
        } else {
            return line
                .index_for_position(at - origin, laid.line_height)
                .ok()
                .map(|found| start + found);
        }
    }
    None
}

fn hit(ranges: &[Range<usize>], index: Option<usize>) -> Option<usize> {
    let index = index?;
    ranges.iter().position(|range| range.contains(&index))
}

fn hangul(ch: char) -> bool {
    matches!(ch, '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}' | '\u{AC00}'..='\u{D7AF}')
}

fn runs(text: &str, style: &TextStyle, marks: &Marks) -> Vec<TextRun> {
    let mut runs = Vec::new();
    let mut at = 0;
    for (range, mark) in marks {
        if at < range.start {
            runs.push(style.to_run(range.start - at));
        }
        runs.push(style.clone().highlight(*mark).to_run(range.len()));
        at = range.end;
    }
    if at < text.len() {
        runs.push(style.to_run(text.len() - at));
    }
    runs
}

thread_local! {
    static WIDTHS: RefCell<HashMap<(FontId, u32, char), Pixels>> = RefCell::default();
}

fn width(window: &Window, font: FontId, font_size: Pixels, ch: char) -> Pixels {
    let key = (font, font_size.as_f32().to_bits(), ch);
    if let Some(found) = WIDTHS.with_borrow(|widths| widths.get(&key).copied()) {
        return found;
    }
    let found = window.text_system().layout_width(font, font_size, ch);
    WIDTHS.with_borrow_mut(|widths| widths.insert(key, found));
    found
}

struct Breaker {
    bytes: Vec<u8>,
    limit: Pixels,
    clamp: Option<usize>,
    line: Pixels,
    filled: bool,
    word: Pixels,
    gap: Pixels,
    first_gap: Option<usize>,
    lines: usize,
    start: usize,
    changed: bool,
    cut: Option<(usize, usize)>,
}

impl Breaker {
    fn next_line(&mut self, at: usize) {
        let before = self.start;
        self.lines += 1;
        self.start = at + 1;
        if self.cut.is_none() && self.clamp.is_some_and(|clamp| self.lines > clamp) {
            self.cut = Some((at, before));
        }
    }

    fn settle(&mut self) {
        if self.word <= Pixels::ZERO {
            return;
        }
        match self.first_gap.take() {
            Some(at) if self.filled && self.line + self.gap + self.word > self.limit => {
                self.bytes[at] = b'\n';
                self.changed = true;
                self.line = self.word;
                self.next_line(at);
            }
            _ => self.line += self.gap + self.word,
        }
        if self.word > self.limit {
            self.line = self.limit;
        }
        self.filled = true;
        self.word = Pixels::ZERO;
        self.gap = Pixels::ZERO;
    }

    fn newline(&mut self, at: usize) {
        self.settle();
        self.line = Pixels::ZERO;
        self.filled = false;
        self.gap = Pixels::ZERO;
        self.first_gap = None;
        self.next_line(at);
    }

    fn space(&mut self, at: usize, advance: Pixels) {
        self.settle();
        if self.filled {
            self.gap += advance;
            self.first_gap.get_or_insert(at);
        } else {
            self.line += advance;
        }
    }
}

struct Wrapped {
    text: String,
    runs: Vec<TextRun>,
}

fn ending(runs: &[TextRun], len: usize) -> Vec<TextRun> {
    let mut kept = Vec::new();
    let mut left = len;
    for run in runs {
        if left == 0 {
            break;
        }
        let take = run.len.min(left);
        kept.push(TextRun {
            len: take,
            ..run.clone()
        });
        left -= take;
    }
    kept
}

fn wrap(
    text: &str,
    runs: &[TextRun],
    font_size: Pixels,
    limit: Pixels,
    clamp: Option<usize>,
    window: &Window,
) -> Option<Wrapped> {
    let mut ends = Vec::new();
    let mut end = 0;
    for run in runs {
        end += run.len;
        ends.push((end, window.text_system().resolve_font(&run.font)));
    }
    let font_at = |at: usize| {
        ends.iter()
            .find(|(end, _)| at < *end)
            .or(ends.last())
            .map(|(_, font)| *font)
    };
    let mut breaker = Breaker {
        bytes: text.as_bytes().to_vec(),
        limit,
        clamp,
        line: Pixels::ZERO,
        filled: false,
        word: Pixels::ZERO,
        gap: Pixels::ZERO,
        first_gap: None,
        lines: 1,
        start: 0,
        changed: false,
        cut: None,
    };
    for (at, ch) in text.char_indices() {
        if breaker.cut.is_some() {
            break;
        }
        let advance = width(window, font_at(at)?, font_size, ch);
        match ch {
            '\n' => breaker.newline(at),
            ' ' => breaker.space(at, advance),
            _ => breaker.word += advance,
        }
    }
    if breaker.cut.is_none() {
        breaker.settle();
    }
    let Some((end, start)) = breaker.cut else {
        return breaker.changed.then(|| Wrapped {
            text: String::from_utf8(breaker.bytes).unwrap_or_default(),
            runs: runs.to_vec(),
        });
    };
    let body = String::from_utf8(breaker.bytes[..end].to_vec()).unwrap_or_default();
    let mut shown = body[start..].trim_end().to_owned();
    let font = font_at(end.saturating_sub(1))?;
    let dots = width(window, font, font_size, '\u{2026}');
    let mut used = Pixels::ZERO;
    for (offset, ch) in shown.char_indices() {
        used += width(window, font_at(start + offset)?, font_size, ch);
    }
    while used + dots > limit
        && let Some(ch) = shown.pop()
    {
        used -= width(window, font_at(start + shown.len())?, font_size, ch);
    }
    let kept = start + shown.len();
    let mut out = body[..kept].to_owned();
    out.push('\u{2026}');
    let mut kept_runs = ending(runs, kept);
    match kept_runs.last_mut() {
        Some(last) => last.len += '\u{2026}'.len_utf8(),
        None => kept_runs.push(TextRun {
            len: '\u{2026}'.len_utf8(),
            ..runs.first()?.clone()
        }),
    }
    Some(Wrapped {
        text: out,
        runs: kept_runs,
    })
}

impl IntoElement for Prose {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Prose {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        self.clicks.as_ref().map(|clicks| clicks.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(
            style
                .line_height
                .to_pixels(font_size.into(), window.rem_size()),
        );
        let runs = runs(&self.text, &style, &self.marks);
        let text = self.text.clone();
        let korean = text.chars().any(hangul);
        let laid = self.laid.clone();
        let layout =
            window.request_measured_layout(Default::default(), move |known, space, window, _| {
                let wrap_width = if style.white_space == WhiteSpace::Normal {
                    known.width.or(match space.width {
                        AvailableSpace::Definite(x) => Some(x),
                        _ => None,
                    })
                } else {
                    None
                };
                if let Some(laid) = laid.borrow().as_ref()
                    && (wrap_width.is_none() || wrap_width == laid.wrap_width)
                {
                    return laid.size;
                }
                let wrapped = match wrap_width {
                    Some(limit) if korean || style.line_clamp.is_some() => {
                        wrap(&text, &runs, font_size, limit, style.line_clamp, window)
                    }
                    _ => None,
                };
                let (shown, used) = match wrapped {
                    Some(wrapped) => (SharedString::from(wrapped.text), wrapped.runs),
                    None => (text.clone(), runs.clone()),
                };
                let Ok(lines) = window.text_system().shape_text(
                    shown,
                    font_size,
                    &used,
                    wrap_width,
                    style.line_clamp,
                ) else {
                    return Size::default();
                };
                let mut size = Size::<Pixels>::default();
                for line in &lines {
                    let one = line.size(line_height);
                    size.height += one.height;
                    size.width = size.width.max(one.width).ceil();
                }
                laid.replace(Some(Laid {
                    lines: lines.into_iter().collect(),
                    line_height,
                    wrap_width,
                    size,
                }));
                size
            });
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        self.clicks
            .as_ref()
            .map(|_| window.insert_hitbox(bounds, HitboxBehavior::Normal))
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let (Some(clicks), Some(hitbox), Some(id)) = (self.clicks.take(), hitbox.clone(), id) {
            self.listen(clicks, hitbox, bounds, id, window);
        }
        let laid = self.laid.borrow();
        let Some(laid) = laid.as_ref() else {
            return;
        };
        let align = window.text_style().text_align;
        let mut origin = bounds.origin;
        for line in &laid.lines {
            line.paint_background(origin, laid.line_height, align, Some(bounds), window, cx)
                .ok();
            line.paint(origin, laid.line_height, align, Some(bounds), window, cx)
                .ok();
            origin.y += line.size(laid.line_height).height;
        }
    }
}

impl Prose {
    fn listen(
        &self,
        clicks: Clicks,
        hitbox: Hitbox,
        bounds: Bounds<Pixels>,
        id: &GlobalElementId,
        window: &mut Window,
    ) {
        let laid = self.laid.clone();
        window.with_element_state::<Rc<Press>, _>(id, |press, window| {
            let press = press.unwrap_or_default();
            let ranges = Rc::new(clicks.ranges);
            let over = hit(&ranges, index_at(&laid, bounds, window.mouse_position())).is_some();
            if over && hitbox.is_hovered(window) {
                window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
            }
            window.on_mouse_event({
                let (press, hitbox, laid, ranges) =
                    (press.clone(), hitbox.clone(), laid.clone(), ranges.clone());
                move |event: &MouseDownEvent, phase, window, _| {
                    if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                        let found = hit(&ranges, index_at(&laid, bounds, event.position));
                        press.down.set(found);
                    }
                }
            });
            window.on_mouse_event({
                let (press, hitbox, laid, ranges) =
                    (press.clone(), hitbox.clone(), laid.clone(), ranges.clone());
                move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                        let found = hit(&ranges, index_at(&laid, bounds, event.position));
                        if let (Some(down), Some(up)) = (press.down.take(), found)
                            && down == up
                        {
                            (clicks.call)(up, window, cx);
                        }
                    }
                }
            });
            window.on_mouse_event({
                let (press, hitbox) = (press.clone(), hitbox.clone());
                move |event: &MouseMoveEvent, phase, window, _| {
                    if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                        let now = hit(&ranges, index_at(&laid, bounds, event.position)).is_some();
                        if press.over.replace(now) != now {
                            window.refresh();
                        }
                    }
                }
            });
            ((), press)
        });
    }
}
