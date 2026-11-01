use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    PinchEvent, Pixels, Point, Render, ScrollEvent, StatefulInteractiveElement, Styled, Window,
    canvas, div, point, px, rgb, size,
};

use api::graph::pty::Pace;
use api::graph::{CardView, GraphEvent};
use api::ids::NodeId;
use api::settings::ArrangeDirection;

use super::arrange;
use super::camera::Camera;
use super::connect::{self, Connect};
use super::drag::{Arrange, Job, Moves, Scope};
use super::edges;
use super::frames::Probe;
use super::grid;
use super::hop_anim::Traffic;
use super::keys;
use super::layout::{self, Rect};
use super::minimap;
use super::popover;
use super::resize::{self, Resizer};
use super::scene::{self, Scene};
use crate::link;
use crate::store::{Store, ZoomTo};
use crate::strings::Lang;
use crate::theme::Theme;
use crate::views::card;
use crate::views::expanded;
use crate::views::notice;
use crate::views::terminal::{self, TerminalView};

const WHEEL_LINE: f32 = 25.0;
const WHEEL_RATE: f32 = 0.002;
const WHEEL_PINCH_BOOST: f32 = 10.0;
const TIP_OFFSET: f32 = 12.0;
const TIP_FONT: f32 = 12.0;
const TERMINAL_STEP: f32 = 1.03;
const HIDDEN_KEEP: Duration = Duration::from_secs(3);
const GRAB_SLOP: f32 = 3.0;
const REPO_HEAD: f32 = 46.0;
const GROUP_HEAD: f32 = 44.0;

struct Live {
    view: Entity<TerminalView>,
    hidden: Option<Instant>,
}

struct Carry {
    from: Point<Pixels>,
    started: bool,
}

pub struct CanvasView {
    pub(super) store: Entity<Store>,
    focus: FocusHandle,
    moves: Moves,
    carry: Option<Carry>,
    seen: u64,
    pub(super) scene: Scene,
    resizer: Resizer,
    live: HashMap<u32, Live>,
    focused: Option<u32>,
    expanded: Option<u32>,
    overlay: FocusHandle,
    fit_pending: bool,
    pub(super) camera: Camera,
    drag: Option<Point<Pixels>>,
    pub(super) area: Rc<Cell<Bounds<Pixels>>>,
    pub(super) map: minimap::Pan,
    pub(super) pop: Option<popover::Popped>,
    probe: Option<Probe>,
    auto: bool,
    started: Instant,
    connect: Connect,
    traffic: Traffic,
    titles: Vec<(Vec<layout::Point>, String)>,
    tip: Option<(String, Point<Pixels>)>,
}

fn snapped(zoom: f32) -> f32 {
    TERMINAL_STEP.powf((zoom.ln() / TERMINAL_STEP.ln()).round())
}

fn bounds_of(rect: Rect) -> Bounds<f32> {
    Bounds::new(
        point(rect.at.x as f32, rect.at.y as f32),
        size(rect.size.width as f32, rect.size.height as f32),
    )
}

impl CanvasView {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mode = std::env::var("LOOM_DESKTOP_FRAMES").ok();
        cx.observe(&store, |this, _, cx| {
            this.sync(cx);
            cx.notify();
        })
        .detach();
        cx.subscribe(&store, |this, _, event: &GraphEvent, cx| {
            this.traffic.event(event, Instant::now());
            cx.notify();
        })
        .detach();
        cx.subscribe(&store, |this, _, event: &ZoomTo, cx| {
            this.zoom_to(event.0, cx)
        })
        .detach();
        cx.subscribe(&store, |this, _, event: &Arrange, cx| {
            this.arrange(event.direction, Scope::All, cx);
        })
        .detach();
        cx.observe_window_activation(window, |_, window, _| {
            link::pace(if window.is_window_active() {
                Pace::Front
            } else {
                Pace::Back
            });
        })
        .detach();
        Self {
            store,
            focus: cx.focus_handle(),
            moves: Moves::default(),
            carry: None,
            seen: 0,
            scene: Scene::default(),
            resizer: Resizer::default(),
            live: HashMap::new(),
            focused: None,
            expanded: None,
            overlay: cx.focus_handle(),
            fit_pending: true,
            camera: Camera::HOME,
            drag: None,
            area: Rc::new(Cell::new(Bounds::default())),
            map: minimap::Pan::default(),
            pop: None,
            probe: mode.is_some().then(Probe::default),
            auto: mode.as_deref() == Some("auto"),
            started: Instant::now(),
            connect: Connect::default(),
            traffic: Traffic::default(),
            titles: Vec::new(),
            tip: None,
        }
    }

    fn sync(&mut self, cx: &App) {
        let store = self.store.read(cx);
        let retired = self.moves.retire(store.loaded());
        let revision = store.revision();
        if revision != self.seen || retired {
            self.seen = revision;
            self.rebuild(cx);
        }
    }

    fn rebuild(&mut self, cx: &App) {
        let store = self.store.read(cx);
        let Some(snapshot) = store.snapshot() else {
            return;
        };
        self.resizer.settle(snapshot);
        let overlay = self
            .moves
            .overlay(store.settings(), self.scene.outers.clone());
        let mut next = scene::build(snapshot, &self.resizer.sizes(), &overlay);
        self.resizer.place(&mut next);
        if !self.moves.dragging()
            && !self.resizer.busy()
            && let Some(id) = store.selected()
            && let Some(before) = self.scene.cards.iter().find(|c| c.node == id)
            && let Some(after) = next.cards.iter().find(|c| c.node == id)
        {
            let zoom = self.camera.zoom;
            self.camera.pan_by(
                -(after.rect.at.x - before.rect.at.x) as f32 * zoom,
                -(after.rect.at.y - before.rect.at.y) as f32 * zoom,
            );
        }
        self.scene = next;
        if self
            .expanded
            .is_some_and(|id| !snapshot.nodes.iter().any(|n| n.id.0 == id))
        {
            self.expanded = None;
        }
    }

    fn arrange(&mut self, direction: ArrangeDirection, scope: Scope, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let Some(snapshot) = store.snapshot() else {
            return;
        };
        let settings = store.settings();
        let overlay = self
            .moves
            .arranging(&self.scene, settings, direction, scope);
        let cleared = scene::build(snapshot, &self.resizer.sizes(), &overlay);
        let job = self.moves.arranged(
            &self.scene,
            &cleared,
            snapshot,
            direction,
            scope,
            settings.always_arrange,
        );
        self.persist(job, cx);
        self.rebuild(cx);
        cx.notify();
    }

    pub(super) fn arrange_repo(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(id) = key.strip_prefix("repo:").and_then(|id| id.parse().ok()) else {
            return;
        };
        let direction = self.store.read(cx).settings().arrange_direction;
        self.arrange(direction, Scope::Repo(id), cx);
    }

    pub(super) fn arrange_group(&mut self, path: &str, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let Some(snapshot) = store.snapshot() else {
            return;
        };
        let job = self
            .moves
            .arrange_group(&self.scene, snapshot, store.settings(), path);
        if let Some(job) = job {
            self.persist(job, cx);
        }
        self.rebuild(cx);
        cx.notify();
    }

    fn persist(&mut self, job: Job, cx: &mut Context<Self>) {
        let Some(base) = self.store.read(cx).base().map(str::to_owned) else {
            return;
        };
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let id = job.id();
            let saved = cx.background_spawn(async move { job.run(&base) }).await;
            let _ = this.update(cx, |view, cx| {
                view.moves.finish(id);
                cx.notify();
            });
            store.update(cx, |store, cx| {
                if let Err(error) = saved {
                    store.tell(error, cx);
                }
                store.reload(cx)
            });
        })
        .detach();
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        let Some(carry) = self.carry.take() else {
            return;
        };
        let store = self.store.read(cx);
        let job = match store.snapshot() {
            Some(snapshot) if carry.started => {
                self.moves
                    .drop(&self.scene, snapshot, store.settings().always_arrange)
            }
            _ => {
                self.moves.cancel();
                None
            }
        };
        if let Some(job) = job {
            self.persist(job, cx);
        }
        self.rebuild(cx);
        cx.notify();
    }

    fn grab(&mut self, key: &str, at: Point<Pixels>, cx: &App) {
        if self
            .moves
            .begin(key, &self.scene, self.store.read(cx).settings())
        {
            self.carry = Some(Carry {
                from: at,
                started: false,
            });
        }
    }

    fn frame_head_at(&self, window_pos: Point<Pixels>) -> Option<String> {
        let at = self.camera.to_graph(self.local(window_pos));
        let hit = |frame: &scene::Frame, head: f32| {
            let r = frame.rect;
            let (x, y) = (f64::from(at.x), f64::from(at.y));
            x >= r.at.x && x < r.at.x + r.size.width && y >= r.at.y && y < r.at.y + f64::from(head)
        };
        self.scene
            .groups
            .iter()
            .rev()
            .find(|f| hit(f, GROUP_HEAD))
            .or_else(|| self.scene.repos.iter().rev().find(|f| hit(f, REPO_HEAD)))
            .map(|f| f.key.clone())
    }

    fn grab_card(&mut self, node: u32, e: &MouseDownEvent, simple: bool, cx: &App) {
        let Some(card) = self.scene.cards.iter().find(|c| c.node == node) else {
            return;
        };
        let top = self.camera.screen_box(bounds_of(card.rect)).origin.y;
        let inside = f32::from(self.local(e.position).y - top);
        if simple || inside < card::HEAD * self.camera.zoom {
            self.grab(&node.to_string(), e.position, cx);
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        let plain = self.focus.is_focused(window);
        let Some(direction) = keys::direction_of(&event.keystroke, plain, cx) else {
            return;
        };
        cx.stop_propagation();
        if let Some(from) = self.store.read(cx).selected()
            && let Some(to) = keys::neighbor(&self.scene, from, direction)
        {
            self.select(to, cx);
        }
    }

    fn zoomed(&self, cx: &App) -> Option<u32> {
        let store = self.store.read(cx);
        let snapshot = store.snapshot()?;
        let present = |id: u32| snapshot.nodes.iter().any(|n| n.id.0 == id);
        let held = self.expanded.filter(|&id| present(id))?;
        Some(store.selected().filter(|&id| present(id)).unwrap_or(held))
    }

    fn expand(&mut self, node: u32, cx: &mut Context<Self>) {
        self.expanded = Some(node);
        self.focused = None;
        self.select(node, cx);
        cx.notify();
    }

    fn collapse(&mut self, cx: &mut Context<Self>) {
        self.expanded = None;
        self.focused = None;
        cx.notify();
    }

    fn begin_resize(
        &mut self,
        node: u32,
        edge: resize::Edge,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.scene.cards.iter().find(|c| c.node == node) else {
            return;
        };
        if let Some(snapshot) = self.store.read(cx).snapshot() {
            self.resizer.begin(card, edge, at, snapshot);
        }
    }

    fn end_resize(&mut self, cx: &mut Context<Self>) {
        let Some(saved) = self.resizer.end() else {
            return;
        };
        let node = saved.size.node;
        let task = self
            .store
            .update(cx, |store, cx| store.save_size(saved.size, saved.spot, cx));
        self.finish_save(node, task, cx);
    }

    fn reset_size(&mut self, node: u32, cx: &mut Context<Self>) {
        let saved = self
            .store
            .read(cx)
            .snapshot()
            .is_some_and(|s| s.sizes.iter().any(|z| z.node.0 == node));
        if !saved {
            return;
        }
        self.resizer.reset(&[node]);
        self.rebuild(cx);
        let task = self
            .store
            .update(cx, |store, cx| store.clear_sizes(vec![NodeId(node)], cx));
        self.finish_save(NodeId(node), task, cx);
        cx.notify();
    }

    fn finish_save(
        &mut self,
        node: NodeId,
        task: gpui::Task<Result<(), String>>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            if let Err(error) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.resizer.fail(&[node.0]);
                    this.store.update(cx, |store, cx| store.tell(error, cx));
                    this.rebuild(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn select(&mut self, node: u32, cx: &mut Context<Self>) {
        self.focused = None;
        self.store.update(cx, |store, cx| store.select(node, cx));
    }

    fn mount(&mut self, zoomed: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let (Some(base), selected) = (store.base().map(str::to_owned), store.selected()) else {
            return;
        };
        let area = self.area.get().size;
        if area.width <= px(0.0) {
            return;
        }
        let camera = self.camera;
        let reach = Bounds::new(
            point(-area.width, -area.height),
            size(area.width * 3.0, area.height * 3.0),
        );
        let mut near: Vec<u32> = self
            .scene
            .cards
            .iter()
            .filter(|c| camera.screen_box(bounds_of(c.rect)).intersects(&reach))
            .map(|c| c.node)
            .collect();
        if let Some(id) = zoomed
            && !near.contains(&id)
        {
            near.push(id);
        }
        let now = Instant::now();
        for id in near.iter().copied() {
            let entry = self.live.entry(id).or_insert_with(|| Live {
                view: terminal::open(&base, id, window, cx),
                hidden: None,
            });
            entry.hidden = None;
        }
        let mut waiting = false;
        let mut focused = self.focused;
        self.live.retain(|id, live| {
            if near.contains(id) {
                return true;
            }
            let since = *live.hidden.get_or_insert(now);
            waiting = true;
            let keep = now - since < HIDDEN_KEEP;
            if !keep && focused == Some(*id) {
                focused = None;
            }
            keep
        });
        self.focused = focused;
        if let Some(id) = zoomed.or(selected)
            && self.focused != Some(id)
            && let Some(live) = self.live.get(&id)
        {
            live.view.update(cx, |view, cx| view.focus(window, cx));
            self.focused = Some(id);
        }
        if waiting {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(HIDDEN_KEEP).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    fn local(&self, window_pos: Point<Pixels>) -> Point<Pixels> {
        window_pos - self.area.get().origin
    }

    fn scripted(&mut self) {
        let t = self.started.elapsed().as_secs_f32();
        self.camera = Camera {
            x: 200.0 * (t * 0.9).sin() - 60.0,
            y: 120.0 * (t * 0.7).cos() - 40.0,
            zoom: 0.45 + 0.4 * (t * 0.5).sin().abs() + 0.25,
        };
    }

    fn fit(&mut self) {
        let area = self.area.get().size;
        let Some(first) = self.scene.cards.first() else {
            return;
        };
        if area.width <= px(0.0) {
            return;
        }
        let mut content = bounds_of(first.rect);
        for rect in self
            .scene
            .repos
            .iter()
            .chain(&self.scene.groups)
            .map(|f| f.rect)
            .chain(self.scene.cards.iter().map(|c| c.rect))
        {
            content = content.union(&bounds_of(rect));
        }
        self.camera = Camera::fit(area, content);
        self.fit_pending = false;
    }

    fn zoom_to(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let area = self.area.get().size;
        let centre = point(area.width / 2.0, area.height / 2.0);
        self.camera.zoom_about(centre, zoom);
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let dy = f32::from(event.delta.pixel_delta(px(WHEEL_LINE)).y);
        let boost = if event.modifiers.control {
            WHEEL_PINCH_BOOST
        } else {
            1.0
        };
        let zoom = self.camera.zoom * 2f32.powf(dy * WHEEL_RATE * boost);
        let at = self.local(event.position);
        self.camera.zoom_about(at, zoom);
        cx.notify();
    }

    fn on_pinch(&mut self, event: &PinchEvent, cx: &mut Context<Self>) {
        let at = self.local(event.position);
        self.camera
            .zoom_about(at, self.camera.zoom * (1.0 + event.delta));
        cx.notify();
    }

    fn connect_move(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.connect.drag(at);
        cx.notify();
    }

    fn connect_end(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let local = self.local(at);
        cx.notify();
        let Some((child, parent)) = self.connect.finish(&self.scene, self.camera, local) else {
            return;
        };
        let task = self
            .store
            .update(cx, |store, cx| store.adopt(child, parent, cx));
        cx.spawn(async move |this, cx| {
            if let Err(rejected) = task.await {
                let _ = this.update(cx, |this, cx| {
                    let lang = *cx.global::<Lang>();
                    this.connect.refuse(connect::refusal_text(&rejected, lang));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn on_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        popover::dismiss(self, cx);
        self.connect.clear();
        window.focus(&self.focus, cx);
        if let Some(key) = self.frame_head_at(event.position) {
            self.grab(&key, event.position, cx);
            return;
        }
        self.drag = Some(event.position);
    }

    fn on_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if minimap::drag(self, event, cx) {
            return;
        }
        if self
            .connect
            .hover(&self.scene, self.camera, self.local(event.position))
        {
            cx.notify();
        }
        let graph = self.camera.to_graph(self.local(event.position));
        let tip = edges::title_at(
            &self.titles,
            layout::Point {
                x: f64::from(graph.x),
                y: f64::from(graph.y),
            },
            f64::from(self.camera.zoom),
        )
        .map(|title| (title.to_owned(), self.local(event.position)));
        if tip != self.tip {
            self.tip = tip;
            cx.notify();
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            self.end_resize(cx);
            self.release(cx);
            return;
        }
        if self.resizer.drag(event.position, self.camera.zoom) {
            self.rebuild(cx);
            cx.notify();
            return;
        }
        if let Some(carry) = &mut self.carry {
            let delta = event.position - carry.from;
            let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
            if !carry.started && dx.hypot(dy) < GRAB_SLOP {
                return;
            }
            carry.started = true;
            let zoom = f64::from(self.camera.zoom);
            let always = self.store.read(cx).settings().always_arrange;
            let moved = layout::Point {
                x: f64::from(dx) / zoom,
                y: f64::from(dy) / zoom,
            };
            if self.moves.drag(&self.scene, moved, always) {
                self.rebuild(cx);
                cx.notify();
            }
            return;
        }
        let Some(last) = self.drag else { return };
        let delta = event.position - last;
        self.camera.pan_by(f32::from(delta.x), f32::from(delta.y));
        self.drag = Some(event.position);
        cx.notify();
    }

    fn on_up(&mut self, cx: &mut Context<Self>) {
        self.drag = None;
        self.end_resize(cx);
        self.release(cx);
        minimap::release(self, cx);
    }
}

impl Render for CanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(probe) = &self.probe {
            probe.begin();
        }
        if self.fit_pending {
            self.fit();
        }
        if self.auto {
            self.scripted();
            window.request_animation_frame();
        }
        let zoomed = self.zoomed(cx);
        self.mount(zoomed, window, cx);
        self.traffic.prune(Instant::now());
        if !self.traffic.is_empty() {
            window.request_animation_frame();
        }
        let camera = self.camera;
        let percent = (camera.zoom * 100.0).round() as u32;
        if self.store.read(cx).zoom_percent() != percent {
            let store = self.store.clone();
            cx.defer(move |cx| store.update(cx, |store, cx| store.set_zoom_percent(percent, cx)));
        }
        let map = minimap::view(self, cx);
        let heads = popover::heads(self, cx);
        let repo_heads = arrange::repo_heads(self, cx);
        let pop = popover::element(self);
        let area = self.area.clone();
        let frame_end = self.probe.clone();
        for (id, live) in &self.live {
            let scale = if zoomed == Some(*id) {
                1.0
            } else {
                snapped(camera.zoom)
            };
            live.view.update(cx, |view, _| view.set_scale(scale));
        }
        let viewport = Bounds::from_corners(
            point(px(0.0), px(0.0)),
            point(self.area.get().size.width, self.area.get().size.height),
        );
        let theme = cx.global::<Theme>();
        let (colors, scheme) = (theme.colors, theme.scheme);
        let lang = *cx.global::<Lang>();
        let store = self.store.read(cx);
        let selected = store.selected();
        let snapshot = store.snapshot();
        let drawing = snapshot.map(|snapshot| {
            edges::build(&edges::Params {
                scene: &self.scene,
                nodes: &snapshot.nodes,
                style: store.settings().edge_style,
                selected,
                traffic: &self.traffic,
                lit: store.lit(),
                colors: &colors,
                scheme,
                lang,
                flow: self.started.elapsed(),
                text_width: &|text| edges::text_width(window, text),
            })
        });
        let drawing = drawing.unwrap_or_default();
        self.titles = drawing.titles;
        let (under, over, labels) = (drawing.under, drawing.over, drawing.labels);
        let tip = self.tip.as_ref().map(|(text, at)| {
            div()
                .absolute()
                .left(at.x + px(TIP_OFFSET))
                .top(at.y + px(TIP_OFFSET))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .bg(colors.surface)
                .text_color(colors.ink)
                .text_size(px(TIP_FONT))
                .child(text.clone())
        });
        let handles = self
            .scene
            .cards
            .iter()
            .filter(|c| self.connect.shows(c.node, selected))
            .filter_map(|c| {
                let id = c.node;
                connect::port(&self.scene, id, camera, &colors).map(|port| {
                    port.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            this.connect.start(id, e.position);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                })
            });
        let line = self
            .connect
            .line(&self.scene, camera, &colors, self.area.get().origin);
        let tracker = self.connect.active().then(|| {
            let (moved, released) = (cx.entity(), cx.entity());
            connect::tracker(
                move |at, cx| moved.update(cx, |this, cx| this.connect_move(at, cx)),
                move |at, cx| released.update(cx, |this, cx| this.connect_end(at, cx)),
            )
        });
        let refusal = self.connect.refusal(&colors);
        let notice = store
            .notice()
            .map(|n| notice::element(n, cx.global::<crate::theme::Theme>()));
        let repos = self.scene.repos.iter().map(|f| {
            card::frame(
                &f.label,
                camera.screen_box(bounds_of(f.rect)),
                camera.zoom,
                true,
            )
        });
        let groups = self.scene.groups.iter().map(|f| {
            let frame = card::frame(
                &f.label,
                camera.screen_box(bounds_of(f.rect)),
                camera.zoom,
                false,
            );
            popover::context(frame, f.place.clone(), f.label.clone(), cx)
        });
        let grabbed = self.moves.grabbed().and_then(|k| k.parse::<u32>().ok());
        let ordered = self
            .scene
            .cards
            .iter()
            .filter(|c| Some(c.node) != grabbed)
            .chain(self.scene.cards.iter().filter(|c| Some(c.node) == grabbed));
        let cards = ordered.filter_map(|placed| {
            let node = snapshot?.nodes.iter().find(|n| n.id.0 == placed.node)?;
            let id = node.id.0;
            let screen = camera.screen_box(bounds_of(placed.rect));
            let body = self
                .live
                .get(&id)
                .filter(|_| screen.intersects(&viewport) && zoomed != Some(id))
                .map(|live| {
                    let scale = snapped(camera.zoom);
                    div()
                        .w(px((placed.rect.size.width as f32) * scale))
                        .h(px((placed.rect.size.height as f32 - card::HEAD) * scale))
                        .child(live.view.clone())
                        .into_any_element()
                });
            let standard = node.card_view != Some(CardView::Simple);
            let handles = standard.then(|| {
                resize::handles(camera.zoom, |edge| {
                    cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        if e.click_count >= 2 {
                            this.reset_size(id, cx);
                        } else {
                            this.begin_resize(id, edge, e.position, cx);
                        }
                        cx.stop_propagation();
                    })
                })
            });
            Some(
                card::card(node, screen, camera.zoom, selected == Some(id), body)
                    .capture_any_mouse_down(cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.select(id, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                            let head = f32::from(screen.origin.y) + card::HEAD * camera.zoom;
                            let top = f32::from(this.local(e.position).y);
                            if e.click_count == 2 && top <= head && standard {
                                this.expand(id, cx);
                            } else {
                                this.grab_card(id, e, !standard, cx);
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .children(handles.into_iter().flatten()),
            )
        });
        let overlay = zoomed.and_then(|id| {
            let live = self.live.get(&id)?;
            let node = snapshot?.nodes.iter().find(|n| n.id.0 == id)?;
            let this = cx.entity();
            let collapse =
                Rc::new(move |cx: &mut App| this.update(cx, |this, cx| this.collapse(cx)));
            Some(expanded::overlay(
                node,
                live.view.clone(),
                &self.overlay,
                collapse,
                cx,
            ))
        });
        div()
            .id("canvas")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(0x14161a))
            .track_focus(&self.focus)
            .capture_key_down(
                cx.listener(|this, e: &KeyDownEvent, window, cx| this.on_key(e, window, cx)),
            )
            .on_scroll(cx.listener(|this, e: &ScrollEvent, _, cx| this.on_scroll(e, cx)))
            .on_pinch(cx.listener(|this, e: &PinchEvent, _, cx| this.on_pinch(e, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, window, cx| this.on_down(e, window, cx)),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.on_move(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.on_up(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.on_up(cx)),
            )
            .child(
                canvas(
                    move |bounds, _, _| area.set(bounds),
                    move |bounds, _, window, _| grid::paint(window, bounds, camera),
                )
                .absolute()
                .size_full(),
            )
            .children(repos)
            .children(groups)
            .child(edges::under(under, camera))
            .children(cards)
            .child(edges::over(over, labels, camera))
            .children(handles)
            .children(line)
            .children(tracker)
            .children(refusal)
            .children(tip)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && (this.connect.leave() | this.tip.take().is_some()) {
                    cx.notify();
                }
            }))
            .children(heads)
            .children(repo_heads)
            .children(map)
            .children(pop)
            .children(notice)
            .children(overlay)
            .children(frame_end.map(|probe| {
                canvas(|_, _, _| (), move |_, _, _, _| probe.end())
                    .absolute()
                    .size(px(1.0))
            }))
    }
}
