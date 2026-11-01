use std::rc::Rc;

use api::graph::errors::{Problem, Refusal};
use gpui::{
    App, CursorStyle, DispatchPhase, Div, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, Styled, canvas, div, px,
};

use super::camera::Camera;
use super::edges;
use super::layout::{self, EdgeEnd, Side, tree_sides};
use super::scene::{DIRECTION, Scene};
use crate::link::Rejected;
use crate::strings::{Lang, Str};
use crate::theme::Colors;

const PORT: f32 = 10.0;
const PORT_BORDER: f32 = 2.0;
const PORT_MIN: f32 = 8.0;
const LINE_WIDTH: f64 = 2.0;
const REFUSAL_TOP: f32 = 60.0;
const REFUSAL_SIDE: f32 = 16.0;
const REFUSAL_WIDTH: f32 = 480.0;
const REFUSAL_FONT: f32 = 13.0;

#[derive(Default)]
pub struct Connect {
    hovered: Option<u32>,
    from: Option<u32>,
    at: Point<Pixels>,
    refusal: Option<String>,
}

fn port_side() -> Side {
    tree_sides(DIRECTION).parent
}

fn port_at(scene: &Scene, node: u32) -> Option<layout::Point> {
    scene.cards.iter().find(|c| c.node == node).map(|c| {
        layout::side_point(EdgeEnd {
            rect: c.rect,
            side: port_side(),
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

    pub fn shows(&self, node: u32, selected: Option<u32>) -> bool {
        self.from == Some(node)
            || (self.from.is_none() && (self.hovered == Some(node) || selected == Some(node)))
    }

    pub fn active(&self) -> bool {
        self.from.is_some()
    }

    pub fn start(&mut self, node: u32, at: Point<Pixels>) {
        self.refusal = None;
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

    pub fn refuse(&mut self, text: String) {
        self.refusal = Some(text);
    }

    pub fn clear(&mut self) {
        self.refusal = None;
    }

    pub fn refusal(&self, colors: &Colors) -> Option<Div> {
        self.refusal.as_ref().map(|text| {
            div()
                .absolute()
                .top(px(REFUSAL_TOP))
                .right(px(REFUSAL_SIDE))
                .max_w(px(REFUSAL_WIDTH))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .bg(colors.surface)
                .text_color(colors.err)
                .text_size(px(REFUSAL_FONT))
                .child(text.clone())
        })
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
        let (points, _) = edges::curve(source, port_side(), target, opposite(port_side()));
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

pub fn refusal_text(rejected: &Rejected, lang: Lang) -> String {
    match rejected {
        Rejected::Link(error) => error.clone(),
        Rejected::Status(status) => {
            Str::ProblemStatus.with(lang, &[("status", &status.to_string())])
        }
        Rejected::Problem(Problem::Refused(refusal)) => match refusal {
            Refusal::Cycle => Str::ProblemRefusedCycle.text(lang).to_owned(),
            Refusal::TooDeep => Str::ProblemRefusedTooDeep.text(lang).to_owned(),
            Refusal::TooManyChildren(n) => {
                Str::ProblemRefusedTooManyChildren.with(lang, &[("value", &n.to_string())])
            }
            Refusal::NameTaken(name) => Str::ProblemRefusedNameTaken.with(lang, &[("value", name)]),
            other => other.to_string(),
        },
        Rejected::Problem(Problem::Launch(error)) => error.to_string(),
        Rejected::Problem(Problem::Repo(error)) => error.to_string(),
        Rejected::Problem(Problem::NoLogin) => Str::ProblemNoLogin.text(lang).to_owned(),
        Rejected::Problem(Problem::Internal(value)) => {
            Str::ProblemInternal.with(lang, &[("value", value)])
        }
    }
}
