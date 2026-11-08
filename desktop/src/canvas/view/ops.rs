use std::rc::Rc;

use api::graph::{CardView, Place, RelaunchOffer};
use api::ids::RepoId;
use gpui::{
    App, Bounds, Context, Div, Entity, MouseDownEvent, Pixels, Point, Subscription, Window, point,
    px,
};

use super::{CanvasView, popover};
use crate::strings::Str;
use crate::views::card::{self, Actions, FrameActions, Keyed, Press};
use crate::views::cardlink;
use crate::views::field::{self, Field, FieldEvent};
use crate::views::icons::Lucide;
use crate::views::menu::{self, Entry, Item};

const MENU_GAP: f32 = 4.0;
const MENU_EDGE: f32 = 8.0;
const BUTTON: f32 = 26.0;

pub(super) struct Opened {
    pub node: u32,
    pub at: Point<Pixels>,
    pub focus: usize,
}

pub(super) struct Renaming {
    pub node: u32,
    pub field: Entity<Field>,
    pub ticket: u64,
    pub busy: bool,
    pub error: Option<String>,
    pub focused: bool,
    _subs: Vec<Subscription>,
}

impl CanvasView {
    pub(super) fn actions(&self, id: u32, collapse: bool, cx: &Context<Self>) -> Actions {
        let press =
            |run: fn(&mut Self, u32, &MouseDownEvent, &mut Window, &mut Context<Self>)| -> Press {
                Rc::new(cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    run(this, id, e, window, cx)
                }))
            };
        Actions {
            anchors: self.anchors.clone(),
            steer: press(|this, id, _, _, cx| this.steer(id, cx)),
            bookmark: press(|this, id, _, _, cx| this.bookmark(id, cx)),
            wake: press(|this, id, _, _, cx| this.wake(id, cx)),
            expand: if collapse {
                press(|this, _, _, _, cx| this.collapse(cx))
            } else {
                press(|this, id, _, _, cx| this.expand(id, cx))
            },
            menu: press(|this, id, e, _, cx| this.open_menu(id, e.position, None, cx)),
            rename: press(|this, id, _, window, cx| this.begin_rename(id, window, cx)),
        }
    }

    fn report(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        if let Err(text) = result {
            self.store.update(cx, |store, cx| store.tell(text, cx));
        }
    }

    fn call(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce(&str) -> Result<(), crate::problem::Failure> + Send + 'static,
    ) {
        cardlink::run(&self.store, cx, work, |this, result, cx| {
            this.report(result, cx)
        })
        .detach();
    }

    fn steer(&mut self, id: u32, cx: &mut Context<Self>) {
        self.call(cx, move |base| cardlink::steer_queued(base, id));
    }

    pub(super) fn wake(&mut self, id: u32, cx: &mut Context<Self>) {
        self.call(cx, move |base| cardlink::wake(base, id));
    }

    fn bookmark(&mut self, id: u32, cx: &mut Context<Self>) {
        let on = self
            .store
            .read(cx)
            .snapshot()
            .and_then(|s| s.nodes.iter().find(|n| n.id.0 == id))
            .is_some_and(|n| n.bookmark.is_none());
        self.call(cx, move |base| crate::link::set_bookmark(base, id, on));
    }

    pub(super) fn open_menu(
        &mut self,
        id: u32,
        click: Point<Pixels>,
        button: Option<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        if self.menu.as_ref().is_some_and(|m| m.node == id) {
            return self.close_menu(cx);
        }
        let area = self.area.get();
        let (right, bottom) = match button.or_else(|| self.anchors.get(&format!("menu:{id}"))) {
            Some(button) => (
                button.right() - area.origin.x,
                button.bottom() - area.origin.y,
            ),
            None => (
                click.x - area.origin.x + px(BUTTON / 2.0),
                click.y - area.origin.y + px(BUTTON / 2.0),
            ),
        };
        let edge = if button.is_some() {
            Pixels::MAX
        } else {
            area.size.width - px(menu::WIDTH + MENU_EDGE)
        };
        let left = (right - px(menu::WIDTH)).min(edge).max(px(MENU_EDGE));
        self.menu = Some(Opened {
            node: id,
            at: point(left, bottom + px(MENU_GAP)),
            focus: 0,
        });
        cx.notify();
    }

    pub(super) fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    fn menu_entries(&self, cx: &Context<Self>) -> Option<Vec<Entry>> {
        let opened = self.menu.as_ref()?;
        let store = self.store.read(cx);
        let snapshot = store.snapshot()?;
        let node = snapshot.nodes.iter().find(|n| n.id.0 == opened.node)?;
        let lang = *cx.global::<crate::strings::Lang>();
        let id = node.id.0;
        let current = card::view_of(node, store.settings().card_view);
        let base = card::base_view(node, store.settings().card_view);
        let next = if current == CardView::Simple {
            CardView::Standard
        } else {
            CardView::Simple
        };
        let clear = next == base;
        let sized = snapshot.sizes.iter().any(|z| z.node.0 == id);
        let this = cx.entity();
        let run = |act: fn(&mut CanvasView, u32, &mut Context<CanvasView>)| -> menu::Run {
            let this = this.clone();
            Rc::new(move |_: &mut Window, app: &mut App| {
                this.update(app, |this, cx| {
                    this.close_menu(cx);
                    act(this, id, cx);
                })
            })
        };
        let mut entries = vec![Entry::Item(Item {
            icon: if next == CardView::Standard {
                Lucide::SquareTerminal
            } else {
                Lucide::PanelTop
            },
            label: if next == CardView::Standard {
                Str::NodeToStandard
            } else {
                Str::NodeToSimple
            }
            .text(lang)
            .to_owned(),
            danger: false,
            disabled: false,
            run: if clear {
                run(|this, id, cx| this.call(cx, move |b| cardlink::clear_card_view(b, id)))
            } else if next == CardView::Simple {
                run(|this, id, cx| {
                    this.call(cx, move |b| {
                        cardlink::set_card_view(b, id, CardView::Simple)
                    })
                })
            } else {
                run(|this, id, cx| {
                    this.call(cx, move |b| {
                        cardlink::set_card_view(b, id, CardView::Standard)
                    })
                })
            },
        })];
        if sized && current == CardView::Standard {
            entries.push(Entry::Item(Item {
                icon: Lucide::RotateCcw,
                label: Str::NodeResetSize.text(lang).to_owned(),
                danger: false,
                disabled: false,
                run: run(|this, id, cx| this.reset_size(id, cx)),
            }));
        }
        if let Some(offer) = node.relaunch {
            let queued = offer == RelaunchOffer::Queued;
            entries.push(Entry::Item(Item {
                icon: Lucide::RefreshCw,
                label: if queued {
                    Str::NodeRelaunchQueued
                } else {
                    Str::NodeRelaunch
                }
                .text(lang)
                .to_owned(),
                danger: false,
                disabled: queued,
                run: run(|this, id, cx| this.call(cx, move |b| cardlink::relaunch(b, id))),
            }));
        }
        if node.relaunch.is_some() || (sized && current == CardView::Standard) {
            entries.push(Entry::Rule);
        }
        entries.push(Entry::Item(Item {
            icon: Lucide::Trash2,
            label: Str::NodeRemove.text(lang).to_owned(),
            danger: true,
            disabled: false,
            run: run(|this, id, cx| {
                if this.expanded == Some(id) {
                    this.collapse(cx);
                }
                this.call(cx, move |b| cardlink::remove_node(b, id));
            }),
        }));
        Some(entries)
    }

    pub(super) fn menu_element(&self, cx: &Context<Self>) -> Option<Div> {
        let opened = self.menu.as_ref()?;
        let entries = self.menu_entries(cx)?;
        let colors = cx.global::<crate::theme::Theme>().colors;
        Some(menu::menu(
            "node-menu",
            entries,
            opened.at,
            opened.focus,
            &colors,
        ))
    }

    pub(super) fn menu_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if key == "escape" {
            self.close_menu(cx);
            return true;
        }
        let Some(entries) = self.menu_entries(cx) else {
            return false;
        };
        let live: Vec<&Item> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(item) if !item.disabled => Some(item),
                _ => None,
            })
            .collect();
        let Some(opened) = self.menu.as_mut() else {
            return false;
        };
        if live.is_empty() {
            return false;
        }
        match key {
            "down" => opened.focus = (opened.focus + 1) % live.len(),
            "up" => opened.focus = (opened.focus + live.len() - 1) % live.len(),
            "enter" | "space" => {
                let run = live[opened.focus.min(live.len() - 1)].run.clone();
                cx.defer_in(window, move |_, window, cx| run(window, cx));
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn begin_rename(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self
            .store
            .read(cx)
            .snapshot()
            .and_then(|s| s.nodes.iter().find(|n| n.id.0 == id))
            .map(|n| n.name.clone())
        else {
            return;
        };
        let field = field::new(Str::RenameLabel, cx);
        field.update(cx, |field, cx| field.set_text(&name, cx));
        let handle = field.read(cx).handle();
        let mut subs = vec![
            cx.subscribe(&field, |this, _, event: &FieldEvent, cx| match event {
                FieldEvent::Enter => this.end_rename(true, cx),
                FieldEvent::Escape => this.end_rename(false, cx),
                FieldEvent::Changed => {
                    if let Some(r) = &mut this.renaming {
                        r.error = None;
                    }
                    cx.notify();
                }
                FieldEvent::Step(_) => {}
            }),
        ];
        subs.push(cx.on_blur(&handle, window, |this, _, cx| this.end_rename(true, cx)));
        window.focus(&handle, cx);
        self.edits += 1;
        self.renaming = Some(Renaming {
            node: id,
            ticket: self.edits,
            field,
            busy: false,
            error: None,
            focused: false,
            _subs: subs,
        });
        cx.notify();
    }

    pub(super) fn end_rename(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(r) = &mut self.renaming else {
            return;
        };
        if r.busy {
            return;
        }
        let id = r.node;
        let value = r.field.read(cx).text().to_owned();
        let name = self
            .store
            .read(cx)
            .snapshot()
            .and_then(|s| s.nodes.iter().find(|n| n.id.0 == id))
            .map(|n| n.name.clone());
        if !commit || name.as_deref() == Some(value.trim()) || name.is_none() {
            self.renaming = None;
            self.focused = None;
            cx.notify();
            return;
        }
        r.busy = true;
        let ticket = r.ticket;
        cardlink::run(
            &self.store,
            cx,
            move |base| cardlink::rename(base, id, &value),
            move |this, result, cx| {
                if this.renaming.as_ref().is_none_or(|r| r.ticket != ticket) {
                    return;
                }
                match result {
                    Ok(()) => {
                        this.renaming = None;
                        this.focused = None;
                    }
                    Err(text) => {
                        if let Some(r) = &mut this.renaming {
                            r.busy = false;
                            r.error = Some(text);
                        }
                    }
                }
                cx.notify();
            },
        )
        .detach();
        cx.notify();
    }

    pub(super) fn focus_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = &mut self.renaming else {
            return;
        };
        if !r.focused {
            r.focused = true;
            let handle = r.field.read(cx).handle();
            window.focus(&handle, cx);
        }
    }
}

fn repo_of(key: &str) -> Option<RepoId> {
    key.strip_prefix("repo:")?.parse().ok().map(RepoId)
}

impl CanvasView {
    pub(super) fn frame_actions(&self, cx: &Context<Self>) -> FrameActions {
        let this = cx.entity();
        let keyed =
            |run: fn(&mut Self, &str, &MouseDownEvent, &mut Window, &mut Context<Self>)| -> Keyed {
                let this = this.clone();
                Rc::new(
                    move |key: &str, e: &MouseDownEvent, window: &mut Window, app: &mut App| {
                        this.update(app, |this, cx| run(this, key, e, window, cx))
                    },
                )
            };
        FrameActions {
            anchors: self.anchors.clone(),
            arrange: keyed(|this, key, _, _, cx| {
                if key.starts_with("repo:") {
                    this.arrange_repo(key, cx);
                } else {
                    this.arrange_group(key, cx);
                }
            }),
            new_node: keyed(|this, path, _, window, cx| this.open_new_node(path, window, cx)),
            delete: keyed(|this, path, _, window, cx| this.open_delete(path, window, cx)),
            new_worktree: keyed(|this, key, _, window, cx| this.open_new_worktree(key, window, cx)),
        }
    }

    fn worktree_at(&self, path: &str) -> Option<(Place, String)> {
        if let Some(frame) = self.scene.groups.iter().find(|f| f.key == path) {
            return Some((frame.place.clone()?, frame.label.clone()));
        }
        let (host, label) = self.scene.chip_rows.iter().find_map(|(host, rows)| {
            rows.iter()
                .find(|(chip, _)| chip == path)
                .map(|(_, label)| (host, label.clone()))
        })?;
        let repo = self
            .scene
            .groups
            .iter()
            .find(|f| f.key == *host)?
            .place
            .as_ref()?
            .repo;
        Some((
            Place {
                repo,
                worktree: path.to_owned(),
            },
            label,
        ))
    }

    fn button_below(&self, key: &str) -> Option<popover::Anchor> {
        self.anchors.get(key).map(popover::Anchor::Below)
    }

    fn open_new_node(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((place, label)) = self.worktree_at(path) else {
            return;
        };
        if let Some(anchor) = self.button_below(&format!("new:{path}")) {
            popover::new_node(self, place, label, anchor, window, cx);
        }
    }

    fn open_delete(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((place, label)) = self.worktree_at(path) else {
            return;
        };
        if let Some(button) = self.anchors.get(&format!("del:{path}")) {
            popover::delete_worktree(self, place.repo, path.to_owned(), label, button, window, cx);
        }
    }

    fn open_new_worktree(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(repo), Some(button)) = (repo_of(key), self.anchors.get(&format!("wt:{key}"))) {
            popover::new_worktree(self, repo, button, window, cx);
        }
    }
}
