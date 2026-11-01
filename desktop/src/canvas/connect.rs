use std::rc::Rc;

use gpui::{
    App, CursorStyle, DispatchPhase, Div, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Styled, canvas, div, px,
};

use super::camera::Camera;
use super::edges;
use super::layout::{self, EdgeEnd, Side, tree_sides};
use super::scene::Scene;
use crate::theme::Colors;

const PORT: f32 = 10.0;
const PORT_BORDER: f32 = 2.0;
const PORT_MIN: f32 = 8.0;
const LINE_WIDTH: f64 = 2.0;

#[derive(Default)]
pub struct Connect {
    hovered: Option<u32>,
    from: Option<u32>,
    at: Point<Pixels>,
}

fn port_side(scene: &Scene) -> Side {
    tree_sides(scene.direction).parent
}

fn port_at(scene: &Scene, node: u32) -> Option<layout::Point> {
    scene.cards.iter().find(|c| c.node == node).map(|c| {
        layout::side_point(EdgeEnd {
            rect: c.rect,
            side: port_side(scene),
        })
    })
}

fn inside(rect: layout::Rect, p: Point<f32>) -> bool {
    let (x, y) = (f64::from(p.x), f64::from(p.y));
    x >= rect.at.x
        && x <= rect.at.x + rect.size.width
        && y >= rect.at.y
        && y <= rect.at.y + rect.size.height
}

fn reach(zoom: f32) -> f64 {
    f64::from(PORT.max(PORT_MIN / zoom))
}

pub fn card_at(scene: &Scene, graph: Point<f32>) -> Option<u32> {
    scene
        .cards
        .iter()
        .rev()
        .find(|c| inside(c.rect, graph))
        .map(|c| c.node)
}

fn port_near(scene: &Scene, graph: Point<f32>, zoom: f32) -> Option<u32> {
    scene
        .cards
        .iter()
        .rev()
        .find(|c| {
            port_at(scene, c.node).is_some_and(|p| {
                (p.x - f64::from(graph.x)).hypot(p.y - f64::from(graph.y)) <= reach(zoom)
            })
        })
        .map(|c| c.node)
}

impl Connect {
    pub fn hover(&mut self, scene: &Scene, camera: Camera, local: Point<Pixels>) -> bool {
        let graph = camera.to_graph(local);
        let next = card_at(scene, graph).or_else(|| port_near(scene, graph, camera.zoom));
        let changed = next != self.hovered;
        self.hovered = next;
        changed
    }

    pub fn leave(&mut self) -> bool {
        self.hovered.take().is_some()
    }

    pub fn shows(&self, node: u32, selected: Option<u32>) -> bool {
        self.from == Some(node)
            || (self.from.is_none() && (self.hovered == Some(node) || selected == Some(node)))
    }

    pub fn active(&self) -> bool {
        self.from.is_some()
    }

    pub fn start(&mut self, node: u32, at: Point<Pixels>) {
        self.from = Some(node);
        self.at = at;
    }

    pub fn drag(&mut self, at: Point<Pixels>) {
        self.at = at;
    }

    pub fn finish(
        &mut self,
        scene: &Scene,
        camera: Camera,
        local: Point<Pixels>,
    ) -> Option<(u32, u32)> {
        let from = self.from.take()?;
        card_at(scene, camera.to_graph(local))
            .filter(|&target| target != from)
            .map(|target| (target, from))
    }

    pub fn line(
        &self,
        scene: &Scene,
        camera: Camera,
        colors: &Colors,
        origin: Point<Pixels>,
    ) -> Option<impl IntoElement> {
        let from = self.from?;
        let source = port_at(scene, from)?;
        let pointer = camera.to_graph(self.at - origin);
        let target = layout::Point {
            x: f64::from(pointer.x),
            y: f64::from(pointer.y),
        };
        let (points, _) =
            edges::curve(source, port_side(scene), target, opposite(port_side(scene)));
        Some(edges::over(
            vec![edges::plain(
                points,
                colors.accent,
                LINE_WIDTH / f64::from(camera.zoom),
            )],
            Vec::new(),
            camera,
        ))
    }
}

fn opposite(side: Side) -> Side {
    match side {
        Side::Top => Side::Bottom,
        Side::Bottom => Side::Top,
        Side::Left => Side::Right,
        Side::Right => Side::Left,
    }
}

pub fn port(scene: &Scene, node: u32, camera: Camera, colors: &Colors) -> Option<Div> {
    let at = camera.to_screen(gpui::point(
        port_at(scene, node)?.x as f32,
        port_at(scene, node)?.y as f32,
    ));
    let side = px((PORT * camera.zoom).max(PORT_MIN));
    Some(
        div()
            .absolute()
            .left(at.x - side / 2.0)
            .top(at.y - side / 2.0)
            .w(side)
            .h(side)
            .rounded_full()
            .bg(colors.surface)
            .border(px(PORT_BORDER * camera.zoom.max(0.5)))
            .border_color(colors.accent)
            .cursor(CursorStyle::Crosshair),
    )
}

pub fn tracker(
    on_move: impl Fn(Point<Pixels>, &mut App) + 'static,
    on_up: impl Fn(Point<Pixels>, &mut App) + 'static,
) -> impl IntoElement {
    let (on_move, on_up) = (Rc::new(on_move), Rc::new(on_up));
    canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            let on_move = on_move.clone();
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture {
                    on_move(e.position, cx);
                }
            });
            let on_up = on_up.clone();
            window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture && e.button == MouseButton::Left {
                    on_up(e.position, cx);
                }
            });
        },
    )
    .absolute()
    .size(px(0.0))
}
