use std::rc::Rc;

use api::graph::NodeState;
use api::graph::waits::{PromptKey, ScreenWait, WaitKind, WaitTone};
use api::ids::NodeId;
use gpui::{
    App, Context, Div, Entity, FontWeight, InteractiveElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::answers::{self, Answers};
use super::body::{self, Target};
use super::entry;
use super::ui::{self, GAP};
use super::{Dock, Inbox};
use crate::link::Snapshot;
use crate::settings::widgets::{Label, segmented_wide};
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

pub(crate) type Visit = Rc<dyn Fn(NodeId, &mut Window, &mut App)>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mood {
    Waiting,
    Failed,
    Plain,
}

pub(crate) struct Group {
    pub node: NodeId,
    pub name: String,
    pub mood: Mood,
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
            let mood = if waits.iter().any(|w| matches!(w.tone, WaitTone::Ask)) {
                Mood::Waiting
            } else if matches!(
                n.state,
                NodeState::Exited { .. } | NodeState::SetupFailed { .. }
            ) {
                Mood::Failed
            } else {
                Mood::Plain
            };
            Group {
                node: n.id,
                name: n.name.clone(),
                mood,
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
        self.drop_pick(cx);
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
        self.store.update(cx, |store, cx| store.go(node.0, cx));
    }

    pub(super) fn questions(&self, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let groups = self.groups(cx);
        let count: usize = groups.iter().map(|g| g.waits.len()).sum();
        let items = vec![
            (
                Inbox::New,
                Label::counted(ui::say(cx, Str::InboxNew).into(), count),
            ),
            (
                Inbox::History,
                Label::counted(ui::say(cx, Str::InboxHistory).into(), 0),
            ),
        ];
        let owner = cx.entity();
        let switch = div()
            .flex_none()
            .px(px(12.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(c.line)
            .child(segmented_wide(
                "inbox-tabs",
                items,
                self.inbox,
                move |value, _, cx| {
                    owner.update(cx, |dock, cx| {
                        dock.inbox = value;
                        if value == Inbox::History {
                            dock.load_history(cx);
                        }
                        cx.notify();
                    })
                },
                cx,
            ));
        let list = match self.inbox {
            Inbox::History => ui::scroller(&self.panel_bar, self.history_list(cx)),
            Inbox::New if groups.is_empty() => {
                ui::empty(ui::say(cx, Str::InboxEmpty).to_owned(), false, cx)
            }
            Inbox::New => {
                let (store, inputs) = (self.store.clone(), self.shared.clone());
                let go: Visit =
                    Rc::new(move |node, _, cx| store.update(cx, |store, cx| store.go(node.0, cx)));
                let (store, focus) = (self.store.clone(), inputs);
                let jump: Visit = Rc::new(move |node, window, cx| {
                    store.update(cx, |store, cx| store.go(node.0, cx));
                    focus.update(cx, |inputs, cx| inputs.focus(node.0, window, cx));
                });
                let list = wait_list(&self.answers, &groups, 1.0, go, jump, cx);
                ui::scroller(&self.panel_bar, div().p(px(6.0)).child(list))
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(switch)
            .child(list)
    }
}

pub(crate) fn wait_list(
    answers: &Entity<Answers>,
    groups: &[Group],
    z: f32,
    go: Visit,
    focus: Visit,
    cx: &mut App,
) -> Div {
    let c = ui::colors(cx);
    let list = div().flex().flex_col();
    groups.iter().enumerate().fold(list, |list, (at, group)| {
        let node = group.node;
        let dot = match group.mood {
            Mood::Waiting => c.ask,
            Mood::Failed => c.err,
            Mood::Plain => c.ink_3,
        };
        let open = go.clone();
        let hover = c.surface_hover;
        let head = div()
            .id(("go", node.0 as usize))
            .flex()
            .items_center()
            .gap(px(GAP * z))
            .h(px(28.0 * z))
            .px(px(8.0 * z))
            .rounded(px(6.0 * z))
            .overflow_hidden()
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .tooltip(crate::views::tooltip::tip(ui::say_with(
                cx,
                Str::InboxJumpLabel,
                &[("name", &group.name)],
            )))
            .child(div().flex_none().size(px(7.0 * z)).rounded_full().bg(dot))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(ui::TEXT * z))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.ink)
                    .child(group.name.clone()),
            )
            .children({
                let asks = group
                    .waits
                    .iter()
                    .filter(|w| matches!(w.tone, WaitTone::Ask))
                    .count();
                entry::notice_badges(asks, group.waits.len() - asks, z, &c)
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| open(node, window, cx));
        let rows = group.waits.iter().enumerate().map(|(row, w)| {
            let jump = focus.clone();
            let mut item = div()
                .id(("wait-row", w.wait.id.0 as usize))
                .flex()
                .flex_col()
                .gap(px(GAP * z))
                .ml(px(-8.0 * z))
                .pt(px(6.0 * z))
                .px(px(8.0 * z))
                .pb(px(10.0 * z))
                .cursor_pointer()
                .hover(move |style| style.bg(hover).rounded(px(6.0 * z)))
                .on_click(move |_, window, cx| jump(node, window, cx));
            if row > 0 {
                item = item.border_t_1().border_dashed().border_color(c.line);
            }
            item.child(answers::entry(answers, node, w, z, cx))
        });
        let mut item = div()
            .flex()
            .flex_col()
            .gap(px(2.0 * z))
            .py(px(4.0 * z))
            .child(head)
            .child(div().flex().flex_col().pl(px(21.0 * z)).children(rows));
        if at > 0 {
            item = item.border_t_1().border_color(c.line);
        }
        list.child(item)
    })
}
