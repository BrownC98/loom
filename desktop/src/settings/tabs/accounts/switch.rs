use std::rc::Rc;
use std::time::Duration;

use api::ids::AccountId;
use api::profile::Provider;
use api::settings::SettingsView;
use gpui::{
    AnyElement, App, AppContext as _, FocusHandle, FontWeight, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::parts::{keep_focus, modal, small};
use super::{Actor, State, Switching, provider_label, slug};
use crate::link;
use crate::strings::Str;
use crate::views::dock::ui::{SMALL, TEXT, colors, say, say_with};

const WIDTH: f32 = 420.0;
const POLL: Duration = Duration::from_millis(100);
const POLLS: usize = 50;

pub fn choose(
    actor: &Actor,
    provider: Provider,
    distro: Option<String>,
    to: Option<AccountId>,
    name: String,
    cx: &mut App,
) {
    let path = format!(
        "/api/accounts/{}/nodes{}",
        slug(provider),
        link::query(&[
            ("distro", distro.as_deref()),
            ("to", to.as_ref().map(|id| id.0.as_str())),
        ])
    );
    let (base, actor) = (actor.base.clone(), actor.clone());
    cx.spawn(async move |cx| {
        let nodes = cx
            .background_spawn(async move { link::read::<Vec<serde_json::Value>>(&base, &path) })
            .await
            .unwrap_or_default();
        cx.update(|cx| {
            if nodes.is_empty() {
                set(&actor, provider, distro, to, cx);
                return;
            }
            let nodes = nodes
                .iter()
                .map(|node| {
                    (
                        node["name"].as_str().unwrap_or_default().to_owned(),
                        node["busy"].as_bool().unwrap_or_default(),
                    )
                })
                .collect();
            actor.local.change(cx, |local| {
                local.switching = Some(Switching {
                    provider,
                    distro,
                    to,
                    name,
                    nodes,
                    busy: false,
                })
            });
        });
    })
    .detach();
}

fn set(
    actor: &Actor,
    provider: Provider,
    distro: Option<String>,
    to: Option<AccountId>,
    cx: &mut App,
) {
    actor.writer.edit(cx, move |settings| {
        settings.set_account(provider, distro.as_deref(), to)
    });
}

fn move_all(actor: &Actor, cx: &mut App) {
    let Some((provider, distro, to)) = actor.local.read(|local| {
        local
            .switching
            .as_ref()
            .map(|s| (s.provider, s.distro.clone(), s.to.clone()))
    }) else {
        return;
    };
    actor.local.change(cx, |local| {
        if let Some(switching) = &mut local.switching {
            switching.busy = true;
        }
    });
    set(actor, provider, distro.clone(), to.clone(), cx);
    let path = format!(
        "/api/accounts/{}/move{}",
        slug(provider),
        link::query(&[("distro", distro.as_deref())])
    );
    let (base, actor) = (actor.base.clone(), actor.clone());
    cx.spawn(async move |cx| {
        let moved = cx
            .background_spawn(async move {
                for _ in 0..POLLS {
                    let saved = link::read::<SettingsView>(&base, "/api/settings")?;
                    if saved.account(provider, distro.as_deref()) == to.as_ref() {
                        break;
                    }
                    std::thread::sleep(POLL);
                }
                link::post::<serde_json::Value>(&base, &path, None).map(|_| ())
            })
            .await;
        cx.update(|cx| {
            actor.local.change(cx, |local| {
                local.switching = None;
                if let Err(error) = moved {
                    local.error = Some(error);
                }
            })
        });
    })
    .detach();
}

pub fn dialog(
    state: &State,
    actor: &Actor,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let (provider, name, nodes, busy, distro, to) = state.local.read(|local| {
        local.switching.as_ref().map(|s| {
            (
                s.provider,
                s.name.clone(),
                s.nodes.clone(),
                s.busy,
                s.distro.clone(),
                s.to.clone(),
            )
        })
    })?;
    keep_focus(focus, window, cx);
    let c = colors(cx);
    let label = provider_label(provider, cx);
    let title = say_with(
        cx,
        Str::SwitchDialogTitle,
        &[("provider", label), ("to", &name)],
    );
    let question = say_with(
        cx,
        Str::SwitchDialogQuestion,
        &[("provider", label), ("count", &nodes.len().to_string())],
    );
    let list = nodes.iter().map(|(node, working)| {
        div()
            .flex()
            .justify_between()
            .gap(px(12.0))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(node.clone()))
            .child(div().text_color(c.ink_3).child(say(
                cx,
                if *working {
                    Str::SwitchDialogBusy
                } else {
                    Str::SwitchDialogIdle
                },
            )))
    });
    let cancel = actor.clone();
    let keep = {
        let (actor, distro, to) = (actor.clone(), distro, to);
        let mut button = small("switch-keep", cx).child(say(cx, Str::SwitchDialogNewOnly));
        if busy {
            button = button.opacity(0.5);
        } else {
            button = button.on_click(move |_, _, cx| {
                actor.local.change(cx, |local| local.switching = None);
                set(&actor, provider, distro.clone(), to.clone(), cx);
            });
        }
        button
    };
    let all = {
        let actor = actor.clone();
        let mut button = small("switch-all", cx)
            .bg(c.accent)
            .text_color(c.accent_ink)
            .child(say(cx, Str::SwitchDialogMoveAll));
        if busy {
            button = button.opacity(0.5);
        } else {
            button = button.on_click(move |_, _, cx| move_all(&actor, cx));
        }
        button
    };
    let content = div()
        .w(px(WIDTH))
        .p(px(18.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(c.line_strong)
        .bg(c.panel)
        .text_size(px(TEXT))
        .text_color(c.ink)
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().child(question))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .text_size(px(SMALL))
                .children(list),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.0))
                .child(keep)
                .child(all),
        );
    Some(modal(
        focus,
        content,
        Rc::new(move |_, cx| cancel.local.change(cx, |local| local.switching = None)),
        window,
        cx,
    ))
}
