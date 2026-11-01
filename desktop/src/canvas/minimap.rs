use gpui::{
    Context, Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Point, Styled, div, point, px, rgb,
};

use api::graph::waits::WaitTone;
use api::graph::{NodeState, NodeView};

use super::layout::Rect;
use super::view::CanvasView;
use crate::theme::{Colors, Theme};

pub const WIDTH: f32 = 200.0;
pub const HEIGHT: f32 = 150.0;
const MARGIN: f32 = 15.0;
const BORDER: f32 = 1.0;
const PADDING: f32 = 5.0;
const CLICK: f32 = 3.0;
const ROUND: f32 = 8.0;
const TILE_ROUND: f32 = 5.0;
const IDLE: u32 = 0xe2e2e2;
const MASK_ALPHA: f32 = 0.7;
const REPO_LINE: f32 = 1.5;
const GROUP_LINE: f32 = 1.0;

#[derive(Clone, Copy)]
pub struct Plan {
    x: f32,
    y: f32,
    scale: f32,
}

impl Plan {
    fn to_map(self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.x) * self.scale, (y - self.y) * self.scale)
    }

    fn to_graph(self, x: f32, y: f32) -> (f32, f32) {
        (x / self.scale + self.x, y / self.scale + self.y)
    }
}

#[derive(Default)]
pub struct Pan {
    plan: Option<Plan>,
    down: bool,
    last: Point<Pixels>,
    travel: f32,
}

enum Mark {
    Repo,
    Group,
    Card(Option<Hsla>),
}

fn edges(rect: Rect) -> (f32, f32, f32, f32) {
    (
        rect.at.x as f32,
        rect.at.y as f32,
        (rect.at.x + rect.size.width) as f32,
        (rect.at.y + rect.size.height) as f32,
    )
}

fn plan_of(canvas: &CanvasView) -> Plan {
    let camera = canvas.camera;
    let area = canvas.area.get().size;
    let view_left = -camera.x / camera.zoom;
    let view_top = -camera.y / camera.zoom;
    let (mut left, mut top) = (view_left, view_top);
    let mut right = view_left + f32::from(area.width) / camera.zoom;
    let mut bottom = view_top + f32::from(area.height) / camera.zoom;
    let rects = canvas
        .scene
        .repos
        .iter()
        .chain(&canvas.scene.groups)
        .map(|f| f.rect)
        .chain(canvas.scene.cards.iter().map(|c| c.rect));
    for rect in rects {
        let (l, t, r, b) = edges(rect);
        left = left.min(l);
        top = top.min(t);
        right = right.max(r);
        bottom = bottom.max(b);
    }
    let (width, height) = (right - left, bottom - top);
    let reach = (width / WIDTH).max(height / HEIGHT);
    let pad = PADDING * reach;
    Plan {
        x: left - (reach * WIDTH - width) / 2.0 - pad,
        y: top - (reach * HEIGHT - height) / 2.0 - pad,
        scale: WIDTH / (reach * WIDTH + 2.0 * pad),
    }
}

fn origin(canvas: &CanvasView) -> Point<Pixels> {
    let area = canvas.area.get();
    point(
        area.origin.x + area.size.width - px(MARGIN + BORDER + WIDTH),
        area.origin.y + area.size.height - px(MARGIN + BORDER + HEIGHT),
    )
}

fn centre_on(canvas: &mut CanvasView, at: Point<Pixels>, cx: &mut Context<CanvasView>) {
    let Some(plan) = canvas.map.plan else { return };
    let local = at - origin(canvas);
    let (x, y) = plan.to_graph(f32::from(local.x), f32::from(local.y));
    let area = canvas.area.get().size;
    let mut to = canvas.camera;
    to.x = f32::from(area.width) / 2.0 - x * to.zoom;
    to.y = f32::from(area.height) / 2.0 - y * to.zoom;
    canvas.fly(to, cx);
}

pub fn press(canvas: &mut CanvasView, event: &MouseDownEvent, cx: &mut Context<CanvasView>) {
    canvas.map.down = true;
    canvas.map.last = event.position;
    canvas.map.travel = 0.0;
    canvas.glide = None;
    cx.stop_propagation();
}

pub fn drag(canvas: &mut CanvasView, event: &MouseMoveEvent, cx: &mut Context<CanvasView>) -> bool {
    if !canvas.map.down {
        return false;
    }
    if event.pressed_button != Some(MouseButton::Left) {
        canvas.map.down = false;
        return false;
    }
    let delta = event.position - canvas.map.last;
    canvas.map.last = event.position;
    canvas.map.travel += f32::from(delta.x).abs() + f32::from(delta.y).abs();
    if canvas.map.travel >= CLICK
        && let Some(plan) = canvas.map.plan
    {
        let zoom = canvas.camera.zoom;
        canvas.camera.pan_by(
            -f32::from(delta.x) / plan.scale * zoom,
            -f32::from(delta.y) / plan.scale * zoom,
        );
        cx.notify();
    }
    true
}

pub fn release(canvas: &mut CanvasView, cx: &mut Context<CanvasView>) {
    if !canvas.map.down {
        return;
    }
    canvas.map.down = false;
    if canvas.map.travel < CLICK {
        centre_on(canvas, canvas.map.last, cx);
    }
}

fn fill(view: &NodeView, colors: &Colors) -> Option<Hsla> {
    if let Some(wait) = view.waits.iter().min_by_key(|w| w.wait.since) {
        return Some(match wait.tone {
            WaitTone::Ask => colors.warn.into(),
            WaitTone::Done => colors.ok.into(),
        });
    }
    match view.state {
        NodeState::Exited { .. } | NodeState::SetupFailed { .. } => Some(colors.err.into()),
        _ => None,
    }
}

pub fn view(
    canvas: &mut CanvasView,
    cx: &mut Context<CanvasView>,
) -> Option<impl IntoElement + use<>> {
    let folded = canvas.store.read(cx).settings().minimap_folded;
    let area = canvas.area.get().size;
    if folded || area.width <= px(0.0) {
        canvas.map.plan = None;
        canvas.map.down = false;
        return None;
    }
    let plan = plan_of(canvas);
    canvas.map.plan = Some(plan);
    let colors = cx.global::<Theme>().colors;
    let idle = Hsla::from(rgb(IDLE));
    let camera = canvas.camera;
    let snapshot = canvas.store.read(cx).snapshot();
    let marks: Vec<(Rect, Mark)> = canvas
        .scene
        .repos
        .iter()
        .map(|f| (f.rect, Mark::Repo))
        .chain(canvas.scene.groups.iter().map(|f| (f.rect, Mark::Group)))
        .chain(canvas.scene.cards.iter().map(|c| {
            let view = snapshot.and_then(|s| s.nodes.iter().find(|n| n.id.0 == c.node));
            (c.rect, Mark::Card(view.and_then(|v| fill(v, &colors))))
        }))
        .collect();
    let tiles = marks.into_iter().map(|(rect, mark)| {
        let (l, t, r, b) = edges(rect);
        let (x, y) = plan.to_map(l, t);
        let (x2, y2) = plan.to_map(r, b);
        let tile = div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px((x2 - x).max(1.0)))
            .h(px((y2 - y).max(1.0)))
            .rounded(px(TILE_ROUND * plan.scale));
        match mark {
            Mark::Repo => tile.border(px(REPO_LINE)).border_color(colors.ink_3),
            Mark::Group => tile
                .border(px(GROUP_LINE))
                .border_dashed()
                .border_color(colors.ink_3),
            Mark::Card(fill) => tile.bg(fill.unwrap_or(idle)),
        }
    });
    let (vx, vy) = plan.to_map(-camera.x / camera.zoom, -camera.y / camera.zoom);
    let (vx2, vy2) = plan.to_map(
        (f32::from(area.width) - camera.x) / camera.zoom,
        (f32::from(area.height) - camera.y) / camera.zoom,
    );
    let shade = Hsla::from(colors.bg).opacity(MASK_ALPHA);
    let (left, top) = (vx.clamp(0.0, WIDTH), vy.clamp(0.0, HEIGHT));
    let (right, bottom) = (vx2.clamp(left, WIDTH), vy2.clamp(top, HEIGHT));
    let band = |x: f32, y: f32, w: f32, h: f32| {
        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .bg(shade)
    };
    let mask = [
        band(0.0, 0.0, WIDTH, top),
        band(0.0, bottom, WIDTH, HEIGHT - bottom),
        band(0.0, top, left, bottom - top),
        band(right, top, WIDTH - right, bottom - top),
    ];
    let window = div()
        .absolute()
        .left(px(vx))
        .top(px(vy))
        .w(px(vx2 - vx))
        .h(px(vy2 - vy))
        .border_1()
        .border_color(colors.line_strong);
    Some(
        div()
            .absolute()
            .right(px(MARGIN))
            .bottom(px(MARGIN))
            .w(px(WIDTH + 2.0 * BORDER))
            .h(px(HEIGHT + 2.0 * BORDER))
            .rounded(px(ROUND))
            .border(px(BORDER))
            .border_color(colors.line)
            .bg(colors.panel)
            .overflow_hidden()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| press(this, e, cx)),
            )
            .children(tiles)
            .children(mask)
            .child(window),
    )
}
