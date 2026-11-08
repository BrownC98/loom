use api::ids::RepoId;
use api::repos::RepoView;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Render, StatefulInteractiveElement,
    Styled, Subscription, Window, div, px,
};

use crate::fonts;
use crate::link::Snapshot;
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::theme::{Colors, Theme};
use crate::views::card;
use crate::views::cardlink;
use crate::views::field::{self, Field, FieldEvent, Form};
use crate::views::icons::{Lucide, icon};
use crate::views::prose::prose;
use crate::views::scrollbar::Bar;

pub const NEW_WIDTH: f32 = 380.0;
pub const DELETE_WIDTH: f32 = 420.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const ROUND: f32 = 10.0;
const BUTTON: f32 = 30.0;
const SELECT_ROW: f32 = 28.0;
const CHANGES_MAX: f32 = 160.0;
const LIST_MAX: f32 = 200.0;

pub struct Closed;

pub(crate) fn shell(c: &Colors, width: f32) -> Div {
    div()
        .w(px(width))
        .p(px(PAD))
        .flex()
        .flex_col()
        .gap(px(GAP))
        .rounded(px(ROUND))
        .bg(c.surface)
        .shadow_lg()
        .text_size(px(13.0))
        .text_color(c.ink)
        .occlude()
}

pub(crate) fn title(text: String, tail: Option<String>, c: &Colors) -> Div {
    div()
        .flex()
        .items_baseline()
        .text_size(px(14.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(text)
        .children(tail.map(|tail| {
            div()
                .ml(px(6.0))
                .text_size(px(12.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .font_family(fonts::TERMINAL_FAMILY)
                .text_color(c.ink_3)
                .child(tail)
        }))
}

pub(crate) fn hint(text: String, mono: bool, c: &Colors) -> Div {
    div()
        .text_size(px(12.0))
        .text_color(c.ink_3)
        .when(mono, |d| d.font_family(fonts::TERMINAL_FAMILY))
        .child(prose(text))
}

pub(crate) fn error(text: String, c: &Colors) -> Div {
    div()
        .text_size(px(12.0))
        .text_color(c.err)
        .child(prose(text))
}

pub(crate) fn labelled(text: &str, body: impl IntoElement, c: &Colors) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c.ink_2)
                .child(text.to_owned()),
        )
        .child(body)
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    Ghost,
    Primary,
    Danger,
}

pub(crate) fn button(
    id: &'static str,
    text: String,
    kind: Kind,
    disabled: bool,
    c: &Colors,
    press: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (fill, edge, ink, bold) = match kind {
        Kind::Ghost => (None, None, c.ink_2, false),
        Kind::Primary => (Some(c.accent), Some(c.accent), c.accent_ink, true),
        Kind::Danger => (Some(c.err), Some(c.err), c.err_ink, true),
    };
    let hover = c.surface_hover;
    div()
        .id(id)
        .h(px(BUTTON))
        .px(px(12.0))
        .flex()
        .items_center()
        .rounded(px(6.0))
        .border_1()
        .border_color(edge.unwrap_or(gpui::Rgba {
            a: 0.0,
            ..c.line_strong
        }))
        .bg(fill.unwrap_or(c.surface))
        .when(fill.is_none(), |d| {
            d.bg(gpui::Rgba {
                a: 0.0,
                ..c.surface
            })
        })
        .text_color(ink)
        .font_weight(if bold {
            gpui::FontWeight::SEMIBOLD
        } else {
            gpui::FontWeight::MEDIUM
        })
        .when(disabled, |d| d.opacity(0.6))
        .when(!disabled, |d| {
            d.cursor_pointer().when(kind == Kind::Ghost, |d| {
                d.hover(move |s| s.bg(hover).text_color(ink))
            })
        })
        .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, window, cx| {
            if !disabled {
                press(window, cx);
            }
            cx.stop_propagation();
        })
        .child(text)
}

pub(crate) fn actions(left: impl IntoElement, right: impl IntoElement) -> Div {
    div()
        .flex()
        .justify_end()
        .gap(px(8.0))
        .child(left)
        .child(right)
}

pub struct NewWorktreeView {
    store: Entity<Store>,
    repo: RepoView,
    dir: String,
    branch: Entity<Field>,
    base: usize,
    open: bool,
    scroll: Bar,
    error: Option<String>,
    busy: bool,
    _sub: Subscription,
}

impl EventEmitter<Closed> for NewWorktreeView {}

impl NewWorktreeView {
    pub fn new(
        store: Entity<Store>,
        repo: RepoView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let branch = field::form("", false, false, Form::Mono, cx);
        let handle = branch.read(cx).handle();
        window.focus(&handle, cx);
        let sub = cx.subscribe(&branch, |this, _, event: &FieldEvent, cx| match event {
            FieldEvent::Enter => this.create(cx),
            FieldEvent::Escape => cx.emit(Closed),
            FieldEvent::Changed => {
                this.error = None;
                cx.notify();
            }
            FieldEvent::Step(_) => {}
        });
        let dir = store
            .read(cx)
            .settings()
            .workspace_dir
            .clone()
            .unwrap_or_default();
        let base = repo
            .worktrees
            .iter()
            .find(|w| w.original)
            .and_then(|w| w.branch.as_ref())
            .and_then(|name| repo.branches.iter().position(|b| b == name))
            .unwrap_or(0);
        Self {
            store,
            repo,
            dir,
            branch,
            base,
            open: false,
            scroll: Bar::new(),
            error: None,
            busy: false,
            _sub: sub,
        }
    }

    fn leaf(&self, cx: &App) -> String {
        self.branch
            .read(cx)
            .text()
            .trim()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let branch = self.branch.read(cx).text().to_owned();
        if self.busy || self.leaf(cx).is_empty() {
            return;
        }
        let from = self
            .repo
            .branches
            .get(self.base)
            .cloned()
            .unwrap_or_else(|| "HEAD".to_owned());
        let repo = self.repo.id.0;
        self.busy = true;
        cardlink::run(
            &self.store,
            cx,
            move |base| cardlink::create_worktree(base, repo, &branch, &from),
            |this, result, cx| match result {
                Ok(_) => {
                    this.store.update(cx, |store, cx| store.reload(cx));
                    cx.emit(Closed);
                }
                Err(text) => {
                    this.error = Some(text);
                    this.busy = false;
                    cx.notify();
                }
            },
        )
        .detach();
        cx.notify();
    }
}

impl Render for NewWorktreeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.global::<Theme>().colors;
        let lang = *cx.global::<Lang>();
        let leaf = self.leaf(cx);
        let separator = if self.dir.contains('\\') { '\\' } else { '/' };
        let folder = [
            self.dir.trim_end_matches(['/', '\\']),
            &crate::canvas::repos::folder_name(&self.repo.path),
            &leaf,
        ]
        .join(&separator.to_string());
        let current = self
            .repo
            .branches
            .get(self.base)
            .cloned()
            .unwrap_or_else(|| "HEAD".to_owned());
        let select = div()
            .id("worktree-base")
            .h(px(30.0))
            .px(px(9.0))
            .flex()
            .items_center()
            .justify_between()
            .rounded(px(6.0))
            .border_1()
            .border_color(c.line_strong)
            .bg(c.bg)
            .cursor_pointer()
            .child(current)
            .child(icon(Lucide::ChevronDown, 14.0, c.ink_3))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.open = !this.open;
                    cx.notify();
                    cx.stop_propagation();
                }),
            );
        let list = self.open.then(|| {
            let rows = div()
                .id("worktree-base-list")
                .max_h(px(LIST_MAX))
                .p(px(4.0))
                .overflow_y_scroll()
                .track_scroll(self.scroll.handle())
                .flex()
                .flex_col()
                .children(self.repo.branches.iter().enumerate().map(|(i, name)| {
                    div()
                        .id(("worktree-base-row", i))
                        .flex_none()
                        .h(px(SELECT_ROW))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .when(i == self.base, |d| d.bg(c.accent_wash))
                        .hover(move |s| s.bg(c.surface_hover))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.base = i;
                                this.open = false;
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        )
                        .child(name.clone())
                }));
            self.scroll
                .on_overflow(rows)
                .rounded(px(6.0))
                .border_1()
                .border_color(c.line_strong)
                .bg(c.bg)
                .overflow_hidden()
        });
        let (create, cancel) = (cx.entity(), cx.entity());
        shell(&c, NEW_WIDTH)
            .child(title(Str::RepoNewWorktree.text(lang).to_owned(), None, &c))
            .child(labelled(
                Str::RepoBranch.text(lang),
                div().w_full().child(self.branch.clone()),
                &c,
            ))
            .child(labelled(
                Str::RepoBase.text(lang),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(select)
                    .children(list),
                &c,
            ))
            .when(!leaf.is_empty(), |d| {
                d.child(hint(
                    Str::RepoFolder.with(lang, &[("path", &folder)]),
                    true,
                    &c,
                ))
            })
            .children(self.error.clone().map(|text| error(text, &c)))
            .child(actions(
                button(
                    "worktree-cancel",
                    Str::CommonCancel.text(lang).to_owned(),
                    Kind::Ghost,
                    false,
                    &c,
                    move |_, cx| cancel.update(cx, |_, cx| cx.emit(Closed)),
                ),
                button(
                    "worktree-create",
                    Str::RepoCreate.text(lang).to_owned(),
                    Kind::Primary,
                    self.busy || leaf.is_empty(),
                    &c,
                    move |_, cx| create.update(cx, |this, cx| this.create(cx)),
                ),
            ))
    }
}

pub struct Member {
    pub name: String,
    pub running: bool,
}

pub struct Doomed {
    pub repo: RepoId,
    pub path: String,
    pub label: String,
    pub branch: Option<String>,
    pub members: Vec<Member>,
}

impl Doomed {
    pub fn of(snapshot: &Snapshot, repo: RepoId, path: String, label: String) -> Doomed {
        let branch = snapshot
            .repos
            .iter()
            .find(|r| r.id == repo)
            .and_then(|r| r.worktrees.iter().find(|w| w.path == path))
            .and_then(|w| w.branch.clone());
        let members = snapshot
            .nodes
            .iter()
            .filter(|n| n.place.repo == repo && n.place.worktree == path)
            .map(|n| Member {
                name: n.name.clone(),
                running: card::active(n),
            })
            .collect();
        Doomed {
            repo,
            path,
            label,
            branch,
            members,
        }
    }
}

pub struct DeleteWorktreeView {
    store: Entity<Store>,
    repo: RepoId,
    path: String,
    label: String,
    branch: Option<String>,
    members: Vec<Member>,
    changes: Option<Vec<String>>,
    delete_branch: bool,
    error: Option<String>,
    busy: bool,
    focus: FocusHandle,
    scroll: Bar,
}

impl EventEmitter<Closed> for DeleteWorktreeView {}

impl DeleteWorktreeView {
    pub fn new(
        store: Entity<Store>,
        doomed: Doomed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Doomed {
            repo,
            path,
            label,
            branch,
            members,
        } = doomed;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let asked = path.clone();
        cardlink::run(
            &store,
            cx,
            move |base| cardlink::worktree_changes(base, repo.0, &asked),
            |this, result, cx| {
                match result {
                    Ok(changes) => this.changes = Some(changes),
                    Err(text) => this.error = Some(text),
                }
                cx.notify();
            },
        )
        .detach();
        Self {
            store,
            repo,
            path,
            label,
            branch,
            members,
            changes: None,
            delete_branch: false,
            error: None,
            busy: false,
            focus,
            scroll: Bar::new(),
        }
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let (repo, path, flag) = (self.repo.0, self.path.clone(), self.delete_branch);
        cardlink::run(
            &self.store,
            cx,
            move |base| cardlink::delete_worktree(base, repo, &path, flag),
            |this, result, cx| match result {
                Ok(()) => {
                    this.store.update(cx, |store, cx| store.reload(cx));
                    cx.emit(Closed);
                }
                Err(text) => {
                    this.error = Some(text);
                    this.busy = false;
                    cx.notify();
                }
            },
        )
        .detach();
        cx.notify();
    }
}

impl Render for DeleteWorktreeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.global::<Theme>().colors;
        let lang = *cx.global::<Lang>();
        let running: Vec<&str> = self
            .members
            .iter()
            .filter(|m| m.running)
            .map(|m| m.name.as_str())
            .collect();
        let status = match &self.changes {
            None => self
                .error
                .is_none()
                .then(|| hint(Str::DeleteWorktreeChecking.text(lang).to_owned(), false, &c)),
            Some(list) if list.is_empty() => Some(hint(
                Str::DeleteWorktreeClean.text(lang).to_owned(),
                false,
                &c,
            )),
            Some(_) => Some(error(Str::DeleteWorktreeLost.text(lang).to_owned(), &c)),
        };
        let changes = self.changes.as_ref().filter(|l| !l.is_empty()).map(|list| {
            let area = div()
                .id("worktree-changes")
                .max_h(px(CHANGES_MAX))
                .overflow_y_scroll()
                .track_scroll(self.scroll.handle())
                .px(px(8.0))
                .py(px(6.0))
                .text_size(px(11.0))
                .line_height(px(16.5))
                .font_family(fonts::TERMINAL_FAMILY)
                .text_color(c.ink_2)
                .child(list.join("\n"));
            self.scroll
                .on_overflow(area)
                .rounded(px(6.0))
                .border_1()
                .border_color(c.line)
                .bg(c.bg)
                .overflow_hidden()
        });
        let notes = (!running.is_empty() || !self.members.is_empty()).then(|| {
            let line = |text: String| {
                div()
                    .relative()
                    .pl(px(40.0))
                    .child(div().absolute().left(px(24.0)).child("\u{2022}"))
                    .child(text)
            };
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .children((!running.is_empty()).then(|| {
                    line(Str::DeleteWorktreeStop.with(
                        lang,
                        &[
                            ("n", &running.len().to_string()),
                            ("names", &running.join(", ")),
                        ],
                    ))
                }))
                .children((!self.members.is_empty()).then(|| {
                    line(
                        Str::DeleteWorktreeRemove
                            .with(lang, &[("n", &self.members.len().to_string())]),
                    )
                }))
        });
        let check = self.branch.clone().map(|branch| {
            let on = self.delete_branch;
            div()
                .id("worktree-branch")
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_color(c.ink_2)
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.delete_branch = !this.delete_branch;
                        cx.notify();
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .size(px(14.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(3.0))
                        .border_1()
                        .border_color(if on { c.accent } else { c.line_strong })
                        .bg(if on { c.accent } else { c.bg })
                        .text_color(c.accent_ink)
                        .text_size(px(10.0))
                        .child(if on { "\u{2713}" } else { "" }),
                )
                .child(Str::DeleteWorktreeBranch.with(lang, &[("branch", &branch)]))
        });
        let (cancel, remove) = (cx.entity(), cx.entity());
        shell(&c, DELETE_WIDTH)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.emit(Closed);
                    cx.stop_propagation();
                }
            }))
            .child(title(
                Str::DeleteWorktreeAction.text(lang).to_owned(),
                Some(self.label.clone()),
                &c,
            ))
            .child(hint(
                Str::DeleteWorktreeFolder.with(lang, &[("path", &self.path)]),
                true,
                &c,
            ))
            .children(status)
            .children(changes)
            .children(notes)
            .children(check)
            .children(self.error.clone().map(|text| error(text, &c)))
            .child(actions(
                button(
                    "worktree-delete-cancel",
                    Str::CommonCancel.text(lang).to_owned(),
                    Kind::Ghost,
                    false,
                    &c,
                    move |_, cx| cancel.update(cx, |_, cx| cx.emit(Closed)),
                ),
                button(
                    "worktree-delete-run",
                    Str::DeleteWorktreeDelete.text(lang).to_owned(),
                    Kind::Danger,
                    self.busy,
                    &c,
                    move |_, cx| remove.update(cx, |this, cx| this.remove(cx)),
                ),
            ))
    }
}
