use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, AppContext, Context, Div, Entity, EventEmitter, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, ScrollHandle,
    StatefulInteractiveElement, Styled, Subscription, Window, div, point, px,
};

use api::graph::{NewNode, Place};
use api::ids::AgentId;
use api::profile::AgentView;
use api::repos::PlaceView;
use api::start::{ModelChoice, Start};

use super::arrange;
use super::view::CanvasView;
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::theme::Theme;
use crate::views::field::{self, Field, FieldEvent};
use crate::views::sidebar::icons::{Glyph, icon};

const WIDTH: f32 = 288.0;
const HEIGHT: f32 = 320.0;
const EDGE: f32 = 8.0;
const GAP: f32 = 6.0;
const LIST: f32 = 200.0;
const HEAD_MIN_ZOOM: f32 = 0.3;
const HEAD_HEIGHT: f32 = 22.0;
const HEAD_INSET: f32 = 8.0;
const HEAD_TOP: f32 = 4.0;
const HEAD_TEXT: f32 = 12.0;

pub struct Popped {
    view: Entity<NewNodeView>,
    place: Place,
    at: Point<f32>,
    end: bool,
    _closed: Subscription,
}

struct Closed;

#[derive(Clone)]
struct Choice {
    agent: Option<usize>,
    installed: bool,
}

pub struct NewNodeView {
    store: Entity<Store>,
    place: Place,
    branch: String,
    at: PlaceView,
    recent: Option<Option<AgentId>>,
    agents: Option<Vec<AgentView>>,
    field: Entity<Field>,
    picked: Option<Option<AgentId>>,
    error: Option<String>,
    busy: bool,
    reveal: bool,
    scroll: ScrollHandle,
}

impl EventEmitter<Closed> for NewNodeView {}

fn blocked(agent: &AgentView, at: &PlaceView) -> bool {
    matches!(at, PlaceView::Ssh { .. }) && agent.serves
}

fn usable(agent: &AgentView, at: &PlaceView) -> bool {
    !blocked(agent, at)
        && agent
            .places
            .iter()
            .find(|p| p.place == *at)
            .is_some_and(|p| p.path.is_some())
}

fn short_place(at: &PlaceView, lang: Lang) -> String {
    match at {
        PlaceView::Pc => Str::PlacePcShort.text(lang).to_owned(),
        PlaceView::Wsl { distro } => Str::PlaceWslDistro.with(lang, &[("distro", distro)]),
        PlaceView::Ssh { host } => Str::PlaceSshHost.with(lang, &[("host", host)]),
    }
}

fn tag_place(at: &PlaceView, lang: Lang) -> String {
    match at {
        PlaceView::Pc => Str::PlacePcShort.text(lang).to_owned(),
        PlaceView::Wsl { .. } => Str::PlaceWsl.text(lang).to_owned(),
        PlaceView::Ssh { host } => host.clone(),
    }
}

impl NewNodeView {
    fn new(
        store: Entity<Store>,
        place: Place,
        branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = field::new(Str::NewNodeSearch, cx);
        cx.subscribe(&field, |this, _, event: &FieldEvent, cx| match event {
            FieldEvent::Changed => cx.notify(),
            FieldEvent::Step(step) => this.step(*step, cx),
            FieldEvent::Enter => {
                let active = this.active(cx);
                if let Some(choice) = active {
                    this.create(choice, cx);
                }
            }
            FieldEvent::Escape => cx.emit(Closed),
        })
        .detach();
        let handle = field.read(cx).handle();
        window.focus(&handle, cx);
        let snapshot = store.read(cx).snapshot();
        let repo = snapshot.and_then(|s| s.repos.iter().find(|r| r.id == place.repo));
        let at = repo.map_or(PlaceView::Pc, |r| r.place.clone());
        let recent = snapshot.and_then(|s| {
            s.nodes
                .iter()
                .filter(|n| n.parent.is_none())
                .max_by_key(|n| n.id.0)
                .map(|n| n.agent.clone())
        });
        let task = store.update(cx, |store, cx| store.agents(cx));
        cx.spawn(async move |this, cx| {
            let loaded = task.await;
            let _ = this.update(cx, |this, cx| {
                match loaded {
                    Ok(agents) => this.agents = Some(agents),
                    Err(error) => {
                        this.agents = Some(Vec::new());
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        Self {
            store,
            place,
            branch,
            at,
            recent,
            agents: None,
            field,
            picked: None,
            error: None,
            busy: false,
            reveal: false,
            scroll: ScrollHandle::new(),
        }
    }

    fn place_view(&self) -> PlaceView {
        let places = self
            .agents
            .as_ref()
            .and_then(|a| a.first())
            .map(|a| a.places.iter().map(|p| p.place.clone()).collect::<Vec<_>>())
            .unwrap_or_default();
        if matches!(self.at, PlaceView::Ssh { .. }) || places.contains(&self.at) {
            self.at.clone()
        } else {
            PlaceView::Pc
        }
    }

    fn choices(&self) -> Vec<Choice> {
        let Some(agents) = &self.agents else {
            return Vec::new();
        };
        let at = self.place_view();
        let mut all = vec![Choice {
            agent: None,
            installed: true,
        }];
        for installed in [true, false] {
            all.extend(
                agents
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| usable(a, &at) == installed)
                    .map(|(i, _)| Choice {
                        agent: Some(i),
                        installed,
                    }),
            );
        }
        all
    }

    fn key(&self, choice: &Choice) -> Option<AgentId> {
        choice
            .agent
            .and_then(|i| self.agents.as_ref().map(|a| a[i].id.clone()))
    }

    fn label(&self, choice: &Choice, lang: Lang) -> String {
        match choice.agent {
            Some(i) => self
                .agents
                .as_ref()
                .map_or_else(String::new, |a| a[i].label.clone()),
            None => Str::NewNodeShell.text(lang).to_owned(),
        }
    }

    fn shown(&self, lang: Lang, cx: &gpui::App) -> Vec<Choice> {
        let needle = self.field.read(cx).text().trim().to_lowercase();
        self.choices()
            .into_iter()
            .filter(|c| self.label(c, lang).to_lowercase().contains(&needle))
            .collect()
    }

    fn active_in(&self, shown: &[Choice]) -> Option<Choice> {
        let selectable: Vec<&Choice> = shown.iter().filter(|c| c.installed).collect();
        let all = self.choices();
        let installed: Vec<&Choice> = all.iter().filter(|c| c.installed).collect();
        let fallback = installed
            .iter()
            .find(|c| {
                self.recent
                    .as_ref()
                    .is_some_and(|recent| self.key(c) == *recent)
            })
            .or_else(|| installed.iter().find(|c| c.agent.is_some()))
            .or(installed.first());
        let wanted = match &self.picked {
            Some(key) => key.clone(),
            None => fallback.and_then(|c| self.key(c)),
        };
        selectable
            .iter()
            .find(|c| self.key(c) == wanted)
            .or(selectable.first())
            .map(|c| (*c).clone())
    }

    fn active(&self, cx: &gpui::App) -> Option<Choice> {
        let lang = *cx.global::<Lang>();
        self.active_in(&self.shown(lang, cx))
    }

    fn step(&mut self, step: isize, cx: &mut Context<Self>) {
        let lang = *cx.global::<Lang>();
        let shown = self.shown(lang, cx);
        let selectable: Vec<&Choice> = shown.iter().filter(|c| c.installed).collect();
        if selectable.is_empty() {
            return;
        }
        let at = self
            .active_in(&shown)
            .and_then(|a| selectable.iter().position(|c| self.key(c) == self.key(&a)))
            .map_or(-1, |i| i as isize);
        let next = selectable[(at + step).rem_euclid(selectable.len() as isize) as usize];
        self.picked = Some(self.key(next));
        self.reveal = true;
        cx.notify();
    }

    fn create(&mut self, choice: Choice, cx: &mut Context<Self>) {
        if self.busy || !choice.installed {
            return;
        }
        self.busy = true;
        self.error = None;
        let start = match self.key(&choice) {
            Some(agent) => Start::Agent {
                agent,
                brief: String::new(),
                choice: ModelChoice::default(),
            },
            None => Start::Command {
                line: String::new(),
            },
        };
        let body = NewNode {
            start,
            place: self.place.clone(),
        };
        let store = self.store.clone();
        let task = store.update(cx, |store, cx| store.create_node(body, cx));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let created = task.await;
            let _ = this.update(cx, |this, cx| match created {
                Ok(id) => {
                    this.store
                        .update(cx, |store, cx| store.select_when_loaded(id.0, cx));
                    cx.emit(Closed);
                }
                Err(error) => {
                    this.error = Some(error);
                    this.busy = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl Render for NewNodeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.global::<Theme>().colors;
        let lang = *cx.global::<Lang>();
        let at = self.place_view();
        let multi = self
            .agents
            .as_ref()
            .and_then(|a| a.first())
            .is_some_and(|a| a.places.len() > 1)
            || !matches!(at, PlaceView::Pc);
        let shown = self.shown(lang, cx);
        let active = self.active_in(&shown);
        let count = |installed: bool| {
            shown
                .iter()
                .filter(|c| c.installed == installed && c.agent.is_some())
                .count()
        };
        let heading = |text: String| {
            div()
                .px(px(8.0))
                .pt(px(6.0))
                .pb(px(2.0))
                .text_size(px(11.0))
                .text_color(colors.ink_3)
                .child(text)
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut scroll_to = 0;
        if shown.is_empty() {
            let text = if self.agents.is_none() {
                Str::NewNodeLoading
            } else {
                Str::NewNodeNoMatch
            };
            rows.push(heading(text.text(lang).to_owned()).into_any_element());
        }
        for installed in [true, false] {
            let group: Vec<&Choice> = shown.iter().filter(|c| c.installed == installed).collect();
            if group.is_empty() {
                continue;
            }
            let n = count(installed).to_string();
            let title = match (installed, multi) {
                (true, true) => Str::NewNodeInstalledAt
                    .with(lang, &[("n", &n), ("place", &short_place(&at, lang))]),
                (true, false) => Str::NewNodeInstalled.with(lang, &[("n", &n)]),
                (false, _) => Str::NewNodeMissing.with(lang, &[("n", &count(false).to_string())]),
            };
            rows.push(heading(title).into_any_element());
            for choice in group {
                let selected = active
                    .as_ref()
                    .is_some_and(|a| self.key(a) == self.key(choice) && a.installed);
                if selected {
                    scroll_to = rows.len();
                }
                let key = self.key(choice);
                let recent =
                    choice.installed && self.recent.as_ref().is_some_and(|recent| *recent == key);
                let tag = if !choice.installed {
                    let agent = choice
                        .agent
                        .and_then(|i| self.agents.as_ref().map(|a| &a[i]));
                    Some(if agent.is_some_and(|a| blocked(a, &at)) {
                        Str::NewNodeServerOnSsh.text(lang).to_owned()
                    } else if multi
                        && agent.is_some_and(|a| a.places.iter().any(|p| p.path.is_some()))
                    {
                        Str::PlaceMissingAt.with(lang, &[("place", &tag_place(&at, lang))])
                    } else {
                        Str::NewNodeNotInstalled.text(lang).to_owned()
                    })
                } else if choice.agent.is_none() {
                    Some(Str::NewNodeTerminalOnly.text(lang).to_owned())
                } else {
                    None
                };
                let picked = choice.clone();
                let ink = if choice.installed {
                    colors.ink
                } else {
                    colors.ink_3
                };
                rows.push(
                    div()
                        .id(("choice", rows.len()))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .text_color(ink)
                        .when(choice.installed, |row| {
                            row.cursor_pointer()
                                .hover(move |style| style.bg(colors.surface_hover))
                        })
                        .when(selected, |row| row.bg(colors.accent_wash))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.create(picked.clone(), cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(self.label(choice, lang))
                        .when(recent, |row| {
                            row.child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors.accent)
                                    .child(Str::NewNodeRecent.text(lang)),
                            )
                        })
                        .children(tag.map(|tag| {
                            div()
                                .ml_auto()
                                .text_size(px(11.0))
                                .text_color(colors.ink_3)
                                .child(tag)
                        }))
                        .into_any_element(),
                );
            }
        }
        if self.reveal {
            self.reveal = false;
            self.scroll.scroll_to_item(scroll_to.saturating_sub(1));
        }
        let close = div()
            .id("close")
            .size(px(20.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(move |style| style.bg(colors.surface_hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| {
                    cx.emit(Closed);
                    cx.stop_propagation();
                }),
            )
            .child(icon(Glyph::Clear, 14.0, colors.ink_2));
        div()
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(12.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(colors.line_strong)
            .bg(colors.panel)
            .text_size(px(13.0))
            .text_color(colors.ink)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().child(Str::GraphNewNode.text(lang)))
                    .child(div().text_color(colors.ink_3).child(self.branch.clone()))
                    .child(div().ml_auto().child(close)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.ink_3)
                            .child(Str::NewNodeProfile.text(lang)),
                    )
                    .child(
                        div()
                            .h(px(28.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(colors.line_strong)
                            .bg(colors.surface)
                            .child(self.field.clone()),
                    ),
            )
            .child(
                div()
                    .id("choices")
                    .max_h(px(LIST))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .children(match (&at, self.busy) {
                (PlaceView::Wsl { distro }, true) => Some(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors.ink_3)
                        .child(Str::PlaceStarting.with(lang, &[("distro", distro)])),
                ),
                _ => None,
            })
            .children(self.error.clone().map(|error| {
                div()
                    .text_size(px(12.0))
                    .text_color(colors.err)
                    .child(error)
            }))
    }
}

fn local(canvas: &CanvasView, window: Point<Pixels>) -> Point<f32> {
    canvas.camera.to_graph(window - canvas.area.get().origin)
}

pub fn dismiss(canvas: &mut CanvasView, cx: &mut Context<CanvasView>) {
    if canvas.pop.take().is_some() {
        cx.notify();
    }
}

fn open(
    canvas: &mut CanvasView,
    place: Place,
    branch: String,
    anchor: Point<Pixels>,
    end: bool,
    window: &mut Window,
    cx: &mut Context<CanvasView>,
) {
    if canvas.pop.as_ref().is_some_and(|p| p.place == place) {
        dismiss(canvas, cx);
        return;
    }
    let store = canvas.store.clone();
    let wanted = place.clone();
    let view = cx.new(|cx| NewNodeView::new(store, wanted, branch, window, cx));
    let closed = cx.subscribe(&view, |this, _, _: &Closed, cx| dismiss(this, cx));
    canvas.pop = Some(Popped {
        view,
        place,
        at: local(canvas, anchor),
        end,
        _closed: closed,
    });
    cx.notify();
}

pub fn context(frame: Div, place: Option<Place>, label: String, cx: &Context<CanvasView>) -> Div {
    let Some(place) = place else { return frame };
    frame.on_mouse_down(
        MouseButton::Right,
        cx.listener(move |this, e: &MouseDownEvent, window, cx| {
            let at = local(this, e.position);
            let over_card = this.scene.cards.iter().any(|c| {
                let (x, y) = (c.rect.at.x as f32, c.rect.at.y as f32);
                at.x >= x
                    && at.y >= y
                    && at.x <= x + c.rect.size.width as f32
                    && at.y <= y + c.rect.size.height as f32
            });
            if !over_card {
                open(
                    this,
                    place.clone(),
                    label.clone(),
                    e.position,
                    false,
                    window,
                    cx,
                );
                cx.stop_propagation();
            }
        }),
    )
}

pub fn heads(canvas: &CanvasView, cx: &Context<CanvasView>) -> Vec<Div> {
    let zoom = canvas.camera.zoom;
    if zoom < HEAD_MIN_ZOOM {
        return Vec::new();
    }
    let lang = *cx.global::<Lang>();
    let colors = cx.global::<Theme>().colors;
    let area = canvas.area.get().origin;
    canvas
        .scene
        .groups
        .iter()
        .filter_map(|frame| {
            let place = frame.place.clone()?;
            let label = frame.label.clone();
            let at = canvas.camera.to_screen(point(
                (frame.rect.at.x + frame.rect.size.width) as f32,
                frame.rect.at.y as f32,
            ));
            let height = HEAD_HEIGHT * zoom;
            let right = at.x - px(HEAD_INSET * zoom);
            let top = at.y + px(HEAD_TOP * zoom);
            let anchor = point(area.x + right, area.y + top + px(height + GAP));
            let path = frame.key.clone();
            let new_node = div()
                .h(px(height))
                .flex()
                .items_center()
                .px(px(8.0 * zoom))
                .rounded(px(6.0 * zoom))
                .border(px(zoom.max(0.5)))
                .border_color(colors.line_strong)
                .text_size(px((HEAD_TEXT * zoom).max(1.0)))
                .text_color(colors.ink_2)
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        open(this, place.clone(), label.clone(), anchor, true, window, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(format!("+ {}", Str::GraphNewNode.text(lang)));
            Some(
                div()
                    .absolute()
                    .right(canvas.area.get().size.width - right)
                    .top(top)
                    .h(px(height))
                    .flex()
                    .items_center()
                    .gap(px(4.0 * zoom))
                    .child(arrange::button(
                        zoom,
                        colors,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.arrange_group(&path, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(new_node),
            )
        })
        .collect()
}

pub fn element(canvas: &CanvasView) -> Option<Div> {
    let pop = canvas.pop.as_ref()?;
    let area = canvas.area.get().size;
    let at = canvas.camera.to_screen(pop.at);
    let left = if pop.end { at.x - px(WIDTH) } else { at.x };
    let left = left.min(area.width - px(WIDTH + EDGE)).max(px(EDGE));
    let top = at.y.min(area.height - px(HEIGHT + EDGE)).max(px(EDGE));
    Some(
        div()
            .absolute()
            .left(left)
            .top(top)
            .child(pop.view.clone())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation()),
    )
}
