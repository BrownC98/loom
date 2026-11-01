use std::collections::{HashMap, HashSet};
use std::time::Duration;

use api::graph::{NodeView, Toward};
use api::settings::EdgeStyle;
use gpui::{
    App, Bounds, Font, FontWeight, PathBuilder, Pixels, Rgba, Styled, TextAlign, TextRun, Window,
    canvas, fill, point, px, size,
};

use super::camera::Camera;
use super::hop_anim::{FLOW, Traffic, slot};
use super::hops::{HopEnd, HopEndName, HopPair, HopRoute, LabelSizes, hop_parts, sibling_lanes};
use super::layout::{
    BendCenter, Direction, EdgeEnd, Point, Rect, Side, Size, bend_center, edge_sides,
    label_heading, side_point, step_at,
};
use super::scene::Scene;
use crate::strings::{Lang, Str};
use crate::theme::{Colors, Oklch, Scheme, to_rgba};

const ARROW: f64 = 12.0;
const BEND_DROP: f64 = 16.0;
const STEP_BEND: f64 = ARROW + BEND_DROP;
const ARROW_LENGTH: f64 = 3.3;
const ARROW_HALF: f64 = 2.7;
const WIDTH: f64 = 1.5;
const LIT_WIDTH: f64 = 3.0;
const DIM: f32 = 0.4;
const HIT_WIDTH: f64 = 6.0;
const CHOSEN_WIDTH: f64 = 2.25;
const LIVE_WIDTH: f64 = 2.0;
const DASH: f64 = 6.0;
const DASH_GAP: f64 = 5.0;
const STEP_RADIUS: f64 = 5.0;
const LANE_CORNER: f64 = 5.0;
const HOP_LIFT_MIN: f64 = 40.0;
const CURVATURE: f64 = 0.25;
const CURVE_STEPS: usize = 48;
const CORNER_STEPS: usize = 6;
const LABEL_FONT: f32 = 12.0;
const LABEL_LINE: f64 = 17.0;
const LABEL_PAD_X: f64 = 8.0;
const LABEL_PAD_Y: f64 = 2.0;
const LABEL_RADIUS: f64 = 10.0;
const HOP_LIGHT: (f64, f64) = (0.55, 0.17);
const HOP_DARK: (f64, f64) = (0.72, 0.15);

pub struct Stroke {
    points: Vec<Point>,
    color: Rgba,
    width: f64,
    shift: Option<f64>,
    from_start: bool,
    to_end: bool,
    fade: f32,
}

pub fn plain(points: Vec<Point>, color: Rgba, width: f64) -> Stroke {
    Stroke {
        points,
        color,
        width,
        shift: None,
        from_start: false,
        to_end: false,
        fade: 1.0,
    }
}

pub struct Label {
    at: Point,
    text: String,
    fill: Rgba,
    ink: Rgba,
    fade: f32,
}

#[derive(Default)]
pub struct Drawing {
    pub under: Vec<Stroke>,
    pub over: Vec<Stroke>,
    pub labels: Vec<Label>,
    pub titles: Vec<(Vec<Point>, String)>,
}

pub struct Params<'a> {
    pub scene: &'a Scene,
    pub nodes: &'a [NodeView],
    pub style: EdgeStyle,
    pub selected: Option<u32>,
    pub traffic: &'a Traffic,
    pub lit: Option<&'a HashSet<String>>,
    pub colors: &'a Colors,
    pub scheme: Scheme,
    pub lang: Lang,
    pub flow: Duration,
    pub text_width: &'a dyn Fn(&str) -> f64,
}

pub fn text_width(window: &Window, text: &str) -> f64 {
    f64::from(shape(window, text, px(LABEL_FONT), Rgba::default()).width)
}

fn shape(window: &Window, text: &str, em: Pixels, color: Rgba) -> gpui::ShapedLine {
    let font = Font {
        weight: FontWeight::SEMIBOLD,
        ..window.text_style().font()
    };
    let run = TextRun {
        len: text.len(),
        font,
        color: color.into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text.to_owned().into(), em, &[run], None)
}

fn hop_color(hue: f64, scheme: Scheme) -> Rgba {
    let (l, c) = match scheme {
        Scheme::Light => HOP_LIGHT,
        Scheme::Dark => HOP_DARK,
    };
    to_rgba(Oklch::new(l, c, hue, 1.0))
}

fn pt(x: f64, y: f64) -> Point {
    Point { x, y }
}

fn distance(a: Point, b: Point) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

fn sample(from: Point, to: Point, at: f64) -> Point {
    pt(from.x + (to.x - from.x) * at, from.y + (to.y - from.y) * at)
}

fn quad(out: &mut Vec<Point>, a: Point, control: Point, b: Point) {
    for i in 1..=CORNER_STEPS {
        let t = i as f64 / CORNER_STEPS as f64;
        out.push(sample(sample(a, control, t), sample(control, b, t), t));
    }
}

fn cubic(a: Point, c1: Point, c2: Point, b: Point) -> Vec<Point> {
    (0..=CURVE_STEPS)
        .map(|i| {
            let t = i as f64 / CURVE_STEPS as f64;
            let u = 1.0 - t;
            pt(
                u.powi(3) * a.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t.powi(3) * b.x,
                u.powi(3) * a.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t.powi(3) * b.y,
            )
        })
        .collect()
}

fn unit(side: Side) -> Point {
    match side {
        Side::Top => pt(0.0, -1.0),
        Side::Right => pt(1.0, 0.0),
        Side::Bottom => pt(0.0, 1.0),
        Side::Left => pt(-1.0, 0.0),
    }
}

fn rounded(points: &[Point]) -> Vec<Point> {
    let mut path: Vec<Point> = Vec::new();
    for &p in points {
        if path.last().is_none_or(|&last| distance(last, p) > 1e-9) {
            path.push(p);
        }
    }
    let mut out = vec![path[0]];
    for i in 1..path.len().saturating_sub(1) {
        let (a, b, c) = (path[i - 1], path[i], path[i + 1]);
        if (a.x == b.x && b.x == c.x) || (a.y == b.y && b.y == c.y) {
            out.push(b);
            continue;
        }
        let bend = (distance(a, b) / 2.0)
            .min(distance(b, c) / 2.0)
            .min(STEP_RADIUS);
        let (enter, leave) = if a.y == b.y {
            let x_dir = if a.x < c.x { -1.0 } else { 1.0 };
            let y_dir = if a.y < c.y { 1.0 } else { -1.0 };
            (pt(b.x + bend * x_dir, b.y), pt(b.x, b.y + bend * y_dir))
        } else {
            let x_dir = if a.x < c.x { 1.0 } else { -1.0 };
            let y_dir = if a.y < c.y { -1.0 } else { 1.0 };
            (pt(b.x, b.y + bend * y_dir), pt(b.x + bend * x_dir, b.y))
        };
        out.push(enter);
        quad(&mut out, enter, b, leave);
    }
    if path.len() > 1 {
        out.push(path[path.len() - 1]);
    }
    out
}

fn step_path(
    source: Point,
    from: Side,
    target: Point,
    to: Side,
    center: BendCenter,
) -> (Vec<Point>, Point) {
    let (s, t) = (unit(from), unit(to));
    let across = matches!(from, Side::Left | Side::Right);
    let (axis_source, axis_target) = if across { (s.x, t.x) } else { (s.y, t.y) };
    if axis_source * axis_target != -1.0 {
        return (vec![source, target], sample(source, target, 0.5));
    }
    let ahead = if across {
        if source.x < target.x { 1.0 } else { -1.0 }
    } else if source.y < target.y {
        1.0
    } else {
        -1.0
    };
    let middle = pt((source.x + target.x) / 2.0, (source.y + target.y) / 2.0);
    let (cx, cy) = if across {
        (
            center.x.unwrap_or(source.x + (target.x - source.x) * 0.5),
            center.y.unwrap_or(middle.y),
        )
    } else {
        (
            center.x.unwrap_or(middle.x),
            center.y.unwrap_or(source.y + (target.y - source.y) * 0.5),
        )
    };
    let vertical = [pt(cx, source.y), pt(cx, target.y)];
    let horizontal = [pt(source.x, cy), pt(target.x, cy)];
    let same = axis_source == ahead;
    let inner = if across == same { vertical } else { horizontal };
    (rounded(&[source, inner[0], inner[1], target]), pt(cx, cy))
}

fn offset(distance: f64) -> f64 {
    if distance >= 0.0 {
        0.5 * distance
    } else {
        CURVATURE * 25.0 * (-distance).sqrt()
    }
}

fn control(side: Side, a: Point, b: Point) -> Point {
    match side {
        Side::Left => pt(a.x - offset(a.x - b.x), a.y),
        Side::Right => pt(a.x + offset(b.x - a.x), a.y),
        Side::Top => pt(a.x, a.y - offset(a.y - b.y)),
        Side::Bottom => pt(a.x, a.y + offset(b.y - a.y)),
    }
}

pub fn curve(source: Point, from: Side, target: Point, to: Side) -> (Vec<Point>, Point) {
    let c1 = control(from, source, target);
    let c2 = control(to, target, source);
    let mid = pt(
        0.125 * source.x + 0.375 * c1.x + 0.375 * c2.x + 0.125 * target.x,
        0.125 * source.y + 0.375 * c1.y + 0.375 * c2.y + 0.125 * target.y,
    );
    (cubic(source, c1, c2, target), mid)
}

fn lane_path(points: &[Point; 4]) -> Vec<Point> {
    let toward = |from: f64, to: f64| {
        let d = to - from;
        if d == 0.0 {
            0.0
        } else {
            d.signum() * LANE_CORNER.min(d.abs() / 2.0)
        }
    };
    let mut out = vec![points[0]];
    for i in 1..3 {
        let (prev, p, next) = (points[i - 1], points[i], points[i + 1]);
        let enter = pt(p.x - toward(prev.x, p.x), p.y - toward(prev.y, p.y));
        let leave = pt(p.x + toward(p.x, next.x), p.y + toward(p.y, next.y));
        out.push(enter);
        quad(&mut out, enter, p, leave);
    }
    out.push(points[3]);
    out
}

fn pulse_text(lang: Lang, heading: Direction, name: &str, n: u32) -> String {
    let key = match heading {
        Direction::Right => Str::GraphMessageToRight,
        Direction::Left => Str::GraphMessageToLeft,
        Direction::Down => Str::GraphMessageToDown,
        Direction::Up => Str::GraphMessageToUp,
    };
    key.with(lang, &[("name", name), ("n", &n.to_string())])
}

struct Boxes<'a> {
    rects: HashMap<u32, (Rect, super::layout::Inset)>,
    nodes: HashMap<u32, &'a NodeView>,
}

impl Boxes<'_> {
    fn rect(&self, id: u32) -> Option<Rect> {
        self.rects.get(&id).map(|(rect, _)| *rect)
    }

    fn name(&self, id: u32) -> String {
        self.nodes
            .get(&id)
            .map_or_else(|| format!("#{id}"), |n| n.name.clone())
    }

    fn center(&self, id: u32) -> Option<Point> {
        self.rect(id)
            .map(|r| pt(r.at.x + r.size.width / 2.0, r.at.y + r.size.height / 2.0))
    }

    fn room(&self, id: u32, parent: u32) -> Rect {
        let (rect, inset) = self.rects[&id];
        let own = self.nodes.get(&id).map(|n| &n.place.worktree);
        let up = self.nodes.get(&parent).map(|n| &n.place.worktree);
        if own.is_none_or(|w| w.is_empty()) || own == up {
            return rect;
        }
        Rect {
            at: pt(rect.at.x - inset.side, rect.at.y - inset.top),
            size: Size {
                width: rect.size.width + 2.0 * inset.side,
                height: rect.size.height + inset.top + inset.side,
            },
        }
    }
}

fn flow_shift(flow: Duration, forward: bool) -> f64 {
    let turn = (flow.as_secs_f64() % FLOW.as_secs_f64()) / FLOW.as_secs_f64();
    let period = DASH + DASH_GAP;
    if forward {
        turn * period
    } else {
        (1.0 - turn) * period
    }
}

fn lit(params: &Params, a: u32, b: u32) -> (bool, f32) {
    match params.lit {
        None => (false, 1.0),
        Some(keys) if keys.contains(&a.to_string()) && keys.contains(&b.to_string()) => (true, 1.0),
        Some(_) => (false, DIM),
    }
}

pub fn build(params: &Params) -> Drawing {
    let Params {
        scene,
        nodes,
        style,
        traffic,
        ..
    } = *params;
    let direction = scene.direction;
    let boxes = Boxes {
        rects: scene
            .cards
            .iter()
            .map(|c| (c.node, (c.rect, c.inset)))
            .collect(),
        nodes: nodes.iter().map(|n| (n.id.0, n)).collect(),
    };
    let step_of = |parent: Rect, child: Rect| {
        (style == EdgeStyle::Step)
            .then(|| step_at(parent, child, direction, STEP_BEND))
            .flatten()
    };
    let mut drawing = Drawing::default();
    let ink = params.colors.accent_ink;
    let mut parents: Vec<&NodeView> = nodes.iter().filter(|n| n.parent.is_some()).collect();
    parents.sort_by_key(|n| n.id.0);
    for view in parents {
        let (Some(parent), child) = (view.parent.map(|p| p.0), view.id.0) else {
            continue;
        };
        let (Some(p), Some(c)) = (boxes.rect(parent), boxes.rect(child)) else {
            continue;
        };
        let sides = edge_sides(p, c, direction);
        let step = step_of(p, c);
        let from = EdgeEnd {
            rect: p,
            side: sides.parent,
        };
        let to = EdgeEnd {
            rect: c,
            side: sides.child,
        };
        let (source, target) = (side_point(from), side_point(to));
        let (points, middle) = match step {
            Some(bend) => step_path(
                source,
                sides.parent,
                target,
                sides.child,
                bend_center(p, bend, direction),
            ),
            None => curve(source, sides.parent, target, sides.child),
        };
        let chosen = params.selected == Some(child) || params.selected == Some(parent);
        let (marked, fade) = lit(params, parent, child);
        match traffic.pulses.get(&child) {
            None => drawing.under.push(Stroke {
                points,
                color: if chosen {
                    params.colors.ink_2
                } else if marked {
                    params.colors.accent
                } else {
                    params.colors.edge
                },
                width: if chosen {
                    CHOSEN_WIDTH
                } else if marked {
                    LIT_WIDTH
                } else {
                    WIDTH
                },
                shift: None,
                from_start: false,
                to_end: false,
                fade,
            }),
            Some(pulse) => {
                let color = hop_color(pulse.hue, params.scheme);
                let forward = pulse.last == Toward::Child;
                drawing.over.push(Stroke {
                    points,
                    color,
                    width: LIVE_WIDTH,
                    shift: Some(flow_shift(params.flow, forward)),
                    from_start: pulse.counts[slot(Toward::Parent)] > 0,
                    to_end: pulse.counts[slot(Toward::Child)] > 0,
                    fade,
                });
                let text = [(Toward::Parent, parent), (Toward::Child, child)]
                    .into_iter()
                    .filter(|(toward, _)| pulse.counts[slot(*toward)] > 0)
                    .map(|(toward, id)| {
                        let heading = if toward == Toward::Child {
                            label_heading(from, to, step)
                        } else {
                            label_heading(to, from, step)
                        };
                        pulse_text(
                            params.lang,
                            heading,
                            &boxes.name(id),
                            pulse.counts[slot(toward)],
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" · ");
                drawing.labels.push(Label {
                    at: middle,
                    text,
                    fill: color,
                    ink,
                    fade,
                });
            }
        }
    }
    hops(params, &boxes, &step_of, &mut drawing);
    drawing
}

fn hops(
    params: &Params,
    boxes: &Boxes,
    step_of: &dyn Fn(Rect, Rect) -> Option<f64>,
    drawing: &mut Drawing,
) {
    let turned = matches!(params.scene.direction, Direction::Right | Direction::Left);
    let mut keys: Vec<&String> = params.traffic.hops.keys().collect();
    keys.sort();
    let texts: HashMap<&String, (String, String)> = keys
        .iter()
        .map(|&key| {
            let hop = &params.traffic.hops[key];
            let parts = hop_parts(
                hop.counts,
                boxes.center(hop.source),
                boxes.center(hop.target),
                turned,
            );
            let end = |name: HopEndName| match name {
                HopEndName::Source => hop.source,
                HopEndName::Target => hop.target,
            };
            let full = parts
                .iter()
                .map(|part| {
                    let key = if part.back {
                        Str::GraphHopBack
                    } else {
                        Str::GraphHop
                    };
                    key.with(
                        params.lang,
                        &[
                            ("from", &boxes.name(end(part.from))),
                            ("to", &boxes.name(end(part.to))),
                            ("n", &hop.counts[part.to as usize].to_string()),
                        ],
                    )
                })
                .collect::<Vec<_>>()
                .join(" / ");
            let short = parts
                .iter()
                .map(|part| hop.counts[part.to as usize].to_string())
                .collect::<Vec<_>>()
                .join(" / ");
            (key, (full, short))
        })
        .collect();
    let pill = |text: &str| Size {
        width: (params.text_width)(text) + 2.0 * LABEL_PAD_X,
        height: LABEL_LINE + 2.0 * LABEL_PAD_Y,
    };
    let parent_of = |id: u32| boxes.nodes.get(&id).and_then(|n| n.parent).map(|p| p.0);
    let mut siblings: HashMap<u32, Vec<HopPair>> = HashMap::new();
    for &key in &keys {
        let hop = &params.traffic.hops[key];
        let Some(parent) = parent_of(hop.source) else {
            continue;
        };
        let (Some(source), Some(target), Some(up)) = (
            boxes.rect(hop.source),
            boxes.rect(hop.target),
            boxes.rect(parent),
        ) else {
            continue;
        };
        if parent_of(hop.target) != Some(parent) {
            continue;
        }
        let end = |id: u32, rect: Rect| HopEnd {
            id,
            rect,
            room: boxes.room(id, parent),
            bend: step_of(up, rect),
        };
        let (full, short) = &texts[key];
        siblings.entry(parent).or_default().push(HopPair {
            key: key.clone(),
            source: end(hop.source, source),
            target: end(hop.target, target),
            label: LabelSizes {
                full: pill(full),
                short: pill(short),
            },
        });
    }
    let routes: HashMap<String, HopRoute> = siblings
        .iter()
        .filter_map(|(&parent, pairs)| {
            boxes
                .rect(parent)
                .map(|up| sibling_lanes(up, pairs, params.scene.direction))
        })
        .flatten()
        .collect();
    for &key in &keys {
        let hop = &params.traffic.hops[key];
        let (Some(a), Some(b)) = (boxes.rect(hop.source), boxes.rect(hop.target)) else {
            continue;
        };
        let (full, short) = &texts[key];
        let color = hop_color(hop.hue, params.scheme);
        let route = routes.get(key);
        let (points, label) = match route {
            Some(route) => (
                lane_path(&route.points),
                route
                    .label
                    .map(|l| (l.at, if l.short { short.clone() } else { full.clone() })),
            ),
            None => {
                let (s, t) = (
                    side_point(EdgeEnd {
                        rect: a,
                        side: Side::Top,
                    }),
                    side_point(EdgeEnd {
                        rect: b,
                        side: Side::Top,
                    }),
                );
                let top = s.y.min(t.y) - HOP_LIFT_MIN.max((t.x - s.x).abs() / 4.0);
                (
                    cubic(s, pt(s.x, top), pt(t.x, top), t),
                    Some((
                        pt((s.x + t.x) / 2.0, (s.y + 6.0 * top + t.y) / 8.0),
                        full.clone(),
                    )),
                )
            }
        };
        let (_, fade) = lit(params, hop.source, hop.target);
        let points_for_title = points.clone();
        drawing.over.push(Stroke {
            points,
            color,
            width: LIVE_WIDTH,
            shift: Some(flow_shift(params.flow, hop.last == HopEndName::Target)),
            from_start: hop.counts[HopEndName::Source as usize] > 0,
            to_end: hop.counts[HopEndName::Target as usize] > 0,
            fade,
        });
        drawing.titles.push((points_for_title, full.clone()));
        if let Some((at, text)) = label {
            drawing.labels.push(Label {
                at,
                text,
                fill: color,
                ink: params.colors.accent_ink,
                fade,
            });
        }
    }
}

fn slice(points: &[Point], lo: f64, hi: f64) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::new();
    let mut walked = 0.0;
    for pair in points.windows(2) {
        let length = distance(pair[0], pair[1]);
        let end = walked + length;
        if length > 0.0 && end > lo && walked < hi {
            let at = |along: f64| sample(pair[0], pair[1], (along - walked) / length);
            if out.is_empty() {
                out.push(at(lo.max(walked)));
            }
            out.push(at(hi.min(end)));
        }
        walked = end;
    }
    out
}

fn dashes(points: &[Point], shift: f64) -> Vec<Vec<Point>> {
    let period = DASH + DASH_GAP;
    let total: f64 = points.windows(2).map(|p| distance(p[0], p[1])).sum();
    let mut out = Vec::new();
    let mut start = shift - period;
    while start < total {
        let (lo, hi) = (start.max(0.0), (start + DASH).min(total));
        if hi > lo {
            out.push(slice(points, lo, hi));
        }
        start += period;
    }
    out
}

fn arrow(tip: Point, behind: Point) -> [Point; 3] {
    let length = distance(tip, behind).max(1e-9);
    let d = pt((tip.x - behind.x) / length, (tip.y - behind.y) / length);
    let base = pt(tip.x - d.x * ARROW_LENGTH, tip.y - d.y * ARROW_LENGTH);
    let n = pt(-d.y, d.x);
    [
        tip,
        pt(base.x + n.x * ARROW_HALF, base.y + n.y * ARROW_HALF),
        pt(base.x - n.x * ARROW_HALF, base.y - n.y * ARROW_HALF),
    ]
}

fn other(points: &[Point], tip: Point, from_end: bool) -> Point {
    let near = |p: &&Point| distance(**p, tip) > 0.5;
    if from_end {
        points.iter().rev().find(near)
    } else {
        points.iter().find(near)
    }
    .copied()
    .unwrap_or(tip)
}

struct Frame {
    bounds: Bounds<Pixels>,
    camera: Camera,
}

impl Frame {
    fn at(&self, p: Point) -> gpui::Point<Pixels> {
        self.bounds.origin + self.camera.to_screen(point(p.x as f32, p.y as f32))
    }
}

fn stroke(window: &mut Window, frame: &Frame, lines: &[Vec<Point>], width: f64, color: Rgba) {
    let mut builder = PathBuilder::stroke(px(width as f32 * frame.camera.zoom));
    for line in lines.iter().filter(|l| l.len() > 1) {
        builder.move_to(frame.at(line[0]));
        for &p in &line[1..] {
            builder.line_to(frame.at(p));
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn head(window: &mut Window, frame: &Frame, corners: [Point; 3], color: Rgba) {
    let mut builder = PathBuilder::fill();
    builder.move_to(frame.at(corners[0]));
    builder.line_to(frame.at(corners[1]));
    builder.line_to(frame.at(corners[2]));
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn paint_stroke(window: &mut Window, frame: &Frame, s: &Stroke) {
    if s.points.len() < 2 {
        return;
    }
    let lines = match s.shift {
        Some(shift) => dashes(&s.points, shift),
        None => vec![s.points.clone()],
    };
    let color = faded(s.color, s.fade);
    stroke(window, frame, &lines, s.width, color);
    let (first, last) = (s.points[0], s.points[s.points.len() - 1]);
    if s.from_start {
        head(
            window,
            frame,
            arrow(first, other(&s.points, first, false)),
            s.color,
        );
    }
    if s.to_end {
        head(
            window,
            frame,
            arrow(last, other(&s.points, last, true)),
            s.color,
        );
    }
}

fn faded(color: Rgba, fade: f32) -> Rgba {
    Rgba {
        a: color.a * fade,
        ..color
    }
}

fn paint_label(window: &mut Window, cx: &mut App, frame: &Frame, label: &Label) {
    let zoom = frame.camera.zoom;
    let line = shape(
        window,
        &label.text,
        px(LABEL_FONT * zoom),
        faded(label.ink, label.fade),
    );
    let center = frame.at(label.at);
    let pad = px(LABEL_PAD_X as f32 * zoom);
    let height = px((LABEL_LINE + 2.0 * LABEL_PAD_Y) as f32 * zoom);
    let width = line.width + pad * 2.0;
    let pill = Bounds::new(
        point(center.x - width / 2.0, center.y - height / 2.0),
        size(width, height),
    );
    window.paint_quad(
        fill(pill, faded(label.fill, label.fade)).corner_radii(px(LABEL_RADIUS as f32 * zoom)),
    );
    let line_height = px(LABEL_LINE as f32 * zoom);
    let _ = line.paint(
        point(center.x - line.width / 2.0, center.y - line_height / 2.0),
        line_height,
        TextAlign::Left,
        None,
        window,
        cx,
    );
}

pub fn under(strokes: Vec<Stroke>, camera: Camera) -> impl gpui::IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let frame = Frame { bounds, camera };
            for s in &strokes {
                paint_stroke(window, &frame, s);
            }
        },
    )
    .absolute()
    .size_full()
}

pub fn over(strokes: Vec<Stroke>, labels: Vec<Label>, camera: Camera) -> impl gpui::IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let frame = Frame { bounds, camera };
            for s in &strokes {
                paint_stroke(window, &frame, s);
            }
            for label in &labels {
                paint_label(window, cx, &frame, label);
            }
        },
    )
    .absolute()
    .size_full()
}

fn near(a: Point, b: Point, p: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / length).clamp(0.0, 1.0)
    };
    distance(p, pt(a.x + t * dx, a.y + t * dy))
}

pub fn title_at(titles: &[(Vec<Point>, String)], graph: Point, zoom: f64) -> Option<&str> {
    let reach = HIT_WIDTH / 2.0 / zoom;
    titles
        .iter()
        .find(|(points, _)| points.windows(2).any(|w| near(w[0], w[1], graph) <= reach))
        .map(|(_, title)| title.as_str())
}
