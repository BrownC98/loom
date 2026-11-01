use api::graph::waits::{PromptKey, ScreenWait, WaitKind};
use api::ids::NodeId;
use gpui::{Context, Div, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, px};

use super::answers;
use super::body::{self, Target};
use super::ui::{self, GAP, PAD};
use super::{Dock, Inbox};
use crate::link::Snapshot;
use crate::strings::Str;

pub fn kind_label(kind: WaitKind) -> Str {
    match kind {
        WaitKind::Question => Str::WaitQuestion,
        WaitKind::Approval => Str::WaitApproval,
        WaitKind::Trust => Str::WaitTrust,
        WaitKind::Auth => Str::WaitAuth,
        WaitKind::Input => Str::WaitInput,
        WaitKind::Done => Str::WaitDone,
    }
}

pub fn key_label(key: &PromptKey, index: usize) -> String {
    match key {
        PromptKey::Char(c) => c.to_string(),
        PromptKey::Line(text) => text.chars().next().map(String::from).unwrap_or_default(),
        PromptKey::Tab => "tab".to_owned(),
        PromptKey::BackTab => "\u{21e7}tab".to_owned(),
        PromptKey::Move(_) | PromptKey::Tick(_) | PromptKey::Reply(_) => (index + 1).to_string(),
        PromptKey::Enter => "\u{23ce}".to_owned(),
    }
}

pub(crate) struct Group {
    pub node: NodeId,
    pub name: String,
    pub waits: Vec<ScreenWait>,
}

pub(crate) fn groups_of(snapshot: &Snapshot) -> Vec<Group> {
    let mut groups: Vec<Group> = snapshot
        .nodes
        .iter()
        .filter(|n| !n.waits.is_empty())
        .map(|n| {
            let mut waits = n.waits.clone();
            waits.sort_by_key(|w| w.wait.since);
            Group {
                node: n.id,
                name: n.name.clone(),
                waits,
            }
        })
        .collect();
    groups.sort_by_key(|g| g.waits[0].wait.since);
    groups
}

impl Dock {
    fn groups(&self, cx: &Context<Self>) -> Vec<Group> {
        self.store
            .read(cx)
            .snapshot()
            .map(groups_of)
            .unwrap_or_default()
    }

    pub(super) fn sync(&mut self, cx: &mut Context<Self>) {
        let mut ids: Vec<u32> = self
            .store
            .read(cx)
            .snapshot()
            .map(|s| {
                s.nodes
                    .iter()
                    .flat_map(|n| n.waits.iter().map(|w| w.wait.id.0))
                    .collect()
            })
            .unwrap_or_default();
        ids.sort_unstable();
        let key = format!("{ids:?}");
        if key != self.open_key {
            self.open_key = key;
            if self.history.loaded {
                self.load_history(cx);
            }
        }
        cx.notify();
    }

    pub(super) fn follow(&mut self, target: Target, cx: &mut Context<Self>) {
        let task = body::open(&self.store, target, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |dock, cx| {
                dock.fault = result.err();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn jump(&mut self, node: NodeId, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| store.select(node.0, cx));
    }

    pub(super) fn questions(&self, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let groups = self.groups(cx);
        let count: usize = groups.iter().map(|g| g.waits.len()).sum();
        let segment = |id: &'static str, label: String, on: bool, cx: &mut Context<Self>| {
            let button = ui::button(id, cx);
            if on {
                button.bg(c.accent_wash).border_color(c.accent)
            } else {
                button
            }
            .child(label)
        };
        let new_label = if count > 0 {
            format!("{} {count}", ui::say(cx, Str::InboxNew))
        } else {
            ui::say(cx, Str::InboxNew).to_owned()
        };
        let switch = div()
            .flex()
            .gap(px(GAP))
            .p(px(PAD))
            .child(
                segment("inbox-new", new_label, self.inbox == Inbox::New, cx).on_click(
                    cx.listener(|dock, _, _, cx| {
                        dock.inbox = Inbox::New;
                        cx.notify();
                    }),
                ),
            )
            .child(
                segment(
                    "inbox-history",
                    ui::say(cx, Str::InboxHistory).to_owned(),
                    self.inbox == Inbox::History,
                    cx,
                )
                .on_click(cx.listener(|dock, _, _, cx| {
                    dock.inbox = Inbox::History;
                    dock.load_history(cx);
                    cx.notify();
                })),
            );
        let list = match self.inbox {
            Inbox::History => self.history_list(cx).into_any_element(),
            Inbox::New if groups.is_empty() => {
                ui::note(ui::say(cx, Str::InboxEmpty).to_owned(), false, cx).into_any_element()
            }
            Inbox::New => div()
                .flex()
                .flex_col()
                .children(groups.iter().map(|g| self.group(g, cx)))
                .into_any_element(),
        };
        div().flex().flex_col().child(switch).child(list)
    }

    fn group(&self, group: &Group, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let node = group.node;
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .py(px(4.0))
            .border_t_1()
            .border_color(c.line)
            .child(
                ui::link(("go", node.0 as usize), group.name.clone(), cx)
                    .px(px(PAD))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .on_click(cx.listener(move |dock, _, _, cx| dock.jump(node, cx))),
            )
            .children(
                group
                    .waits
                    .iter()
                    .map(|w| answers::entry(&self.answers, node, w, 1.0, cx)),
            )
    }
}
