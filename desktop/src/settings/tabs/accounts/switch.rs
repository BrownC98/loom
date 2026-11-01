use std::rc::Rc;

use api::ids::AccountId;
use api::profile::Provider;
use gpui::{AnyElement, App, AppContext as _, FocusHandle, Window};

use super::parts::{keep_focus, modal};
use super::{Actor, State};
use crate::strings::Lang;
use crate::views::usage::{self, Switching};

pub fn choose(
    actor: &Actor,
    provider: Provider,
    distro: Option<String>,
    to: Option<AccountId>,
    name: String,
    cx: &mut App,
) {
    let (base, actor) = (actor.base.clone(), actor.clone());
    cx.spawn(async move |cx| {
        let (found, with, id) = (distro.clone(), distro.clone(), to.clone());
        let nodes = cx
            .background_spawn(async move {
                usage::movable_nodes(&base, provider, found.as_deref(), id.as_ref())
            })
            .await;
        cx.update(|cx| {
            if nodes.is_empty() {
                set(&actor, provider, with, to, cx);
                return;
            }
            actor.local.change(cx, |local| {
                local.switching = Some(Switching {
                    provider,
                    distro: with,
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
    let lang = *cx.global::<Lang>();
    let (base, actor) = (actor.base.clone(), actor.clone());
    let saved = actor.clone();
    let place = distro.clone();
    actor.writer.edit_then(
        cx,
        move |settings| settings.set_account(provider, distro.as_deref(), to),
        move |saved_ok, cx| {
            if let Err(error) = saved_ok {
                let text = error.text(lang);
                saved.local.change(cx, |local| {
                    local.switching = None;
                    local.error = Some(text);
                });
                return;
            }
            cx.spawn(async move |cx| {
                let moved = cx
                    .background_spawn(async move {
                        usage::move_nodes(&base, provider, place.as_deref())
                    })
                    .await;
                cx.update(|cx| {
                    saved.local.change(cx, |local| {
                        local.switching = None;
                        if let Err(error) = moved {
                            local.error = Some(error.text(lang));
                        }
                    })
                });
            })
            .detach();
        },
    );
}

pub fn dialog(
    state: &State,
    actor: &Actor,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let switching = state.local.read(|local| local.switching.clone())?;
    let (provider, distro, to) = (
        switching.provider,
        switching.distro.clone(),
        switching.to.clone(),
    );
    keep_focus(focus, window, cx);
    let (cancel, keeper, mover) = (actor.clone(), actor.clone(), actor.clone());
    let content = usage::switch_content(
        &switching,
        move |_, cx| {
            keeper.local.change(cx, |local| local.switching = None);
            set(&keeper, provider, distro.clone(), to.clone(), cx);
        },
        move |_, cx| move_all(&mover, cx),
        cx,
    );
    Some(modal(
        focus,
        content,
        Rc::new(move |_, cx| cancel.local.change(cx, |local| local.switching = None)),
        window,
        cx,
    ))
}
