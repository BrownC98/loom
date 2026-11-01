use std::rc::Rc;
use std::time::Duration;

use api::profile::Provider;
use api::settings::accounts::{LoginId, LoginState, LoginTarget};
use gpui::{
    AnyElement, App, AppContext as _, FontWeight, ParentElement, StatefulInteractiveElement,
    Styled, Window, div, px,
};

use super::parts::{modal, small};
use super::{Actor, Login, State};
use crate::link;
use crate::problem::Failure;
use crate::strings::{Lang, Str};
use crate::views::dock::ui::{SMALL, TEXT, colors, say};
use crate::views::terminal;
use crate::views::usage::slug;

const POLL: Duration = Duration::from_secs(1);
const LOST: u16 = 404;
const TRIES: u32 = 5;
const WIDTH: f32 = 640.0;
const TERMINAL_H: f32 = 320.0;

pub fn sync(state: &mut State, actor: &Option<Actor>, window: &mut Window, cx: &mut App) {
    let wanted = state
        .local
        .read(|local| local.login.as_ref().map(|login| login.id));
    match (wanted, &state.terminal, actor) {
        (None, _, _) => state.terminal = None,
        (Some(id), Some((open, _)), _) if *open == id => {}
        (Some(id), _, Some(actor)) => {
            let view = terminal::open_login(&actor.base, id, window, cx);
            view.update(cx, |view, cx| view.focus(window, cx));
            state.terminal = Some((id, view));
        }
        _ => {}
    }
}

pub fn open(
    actor: &Actor,
    provider: Provider,
    distro: Option<String>,
    target: LoginTarget,
    title: String,
    fresh: bool,
    cx: &mut App,
) {
    actor.local.change(cx, |local| local.error = None);
    let lang = *cx.global::<Lang>();
    let path = format!(
        "/api/accounts/{}{}",
        slug(provider),
        link::query(&[("distro", distro.as_deref())])
    );
    let body = serde_json::to_string(&target).unwrap_or_default();
    let (base, actor) = (actor.base.clone(), actor.clone());
    cx.spawn(async move |cx| {
        let started = cx
            .background_spawn(async move { link::post::<LoginId>(&base, &path, Some(body)) })
            .await;
        cx.update(|cx| match started {
            Ok(LoginId(id)) => {
                actor.local.change(cx, |local| {
                    local.login = Some(Login {
                        id,
                        title,
                        fresh,
                        failed: false,
                    })
                });
                watch(&actor, id, cx);
            }
            Err(error) => actor
                .local
                .change(cx, |local| local.error = Some(error.text(lang))),
        });
    })
    .detach();
}

fn watch(actor: &Actor, id: u32, cx: &mut App) {
    let actor = actor.clone();
    cx.spawn(async move |cx| {
        let mut errors = 0;
        loop {
            cx.background_executor().timer(POLL).await;
            let open = actor.local.alive()
                && actor
                    .local
                    .read(|local| local.login.as_ref().is_some_and(|login| login.id == id));
            if !open {
                break;
            }
            let base = actor.base.clone();
            let state = cx
                .background_spawn(async move {
                    link::read::<LoginState>(&base, &format!("/api/logins/{id}"))
                })
                .await;
            let failed = match state {
                Ok(LoginState::Succeeded) => {
                    cx.update(|cx| close(&actor, id, cx));
                    break;
                }
                Ok(LoginState::Failed) => true,
                Err(Failure::Refused { status: LOST, .. }) => true,
                Err(_) => {
                    errors += 1;
                    errors >= TRIES
                }
                Ok(LoginState::Running) => {
                    errors = 0;
                    false
                }
            };
            if failed {
                cx.update(|cx| {
                    actor.local.change(cx, |local| {
                        if let Some(login) = local.login.as_mut().filter(|l| l.id == id) {
                            login.failed = true;
                        }
                    })
                });
                break;
            }
        }
    })
    .detach();
}

pub fn close(actor: &Actor, id: u32, cx: &mut App) {
    actor.local.change(cx, |local| {
        if local.login.as_ref().is_some_and(|login| login.id == id) {
            local.login = None;
        }
    });
    actor.run(cx, move |base| {
        link::delete(base, &format!("/api/logins/{id}"))
    });
}

pub fn dialog(state: &State, actor: &Actor, window: &Window, cx: &App) -> Option<AnyElement> {
    let (id, title, fresh, failed) = state.local.read(|local| {
        local
            .login
            .as_ref()
            .map(|login| (login.id, login.title.clone(), login.fresh, login.failed))
    })?;
    let (open, view) = state.terminal.as_ref()?;
    if *open != id {
        return None;
    }
    let focus = state.focus.as_ref()?;
    let c = colors(cx);
    let closer = actor.clone();
    let button_label = say(
        cx,
        if failed {
            Str::CommonClose
        } else {
            Str::CommonCancel
        },
    );
    let button = small("login-close", cx)
        .border_color(gpui::transparent_black())
        .bg(c.panel)
        .child(button_label)
        .on_click({
            let actor = actor.clone();
            move |_, _, cx| close(&actor, id, cx)
        });
    let foot = if failed {
        div()
            .text_size(px(SMALL))
            .text_color(c.err)
            .child(say(cx, Str::AccountsLoginFailed))
    } else {
        div().text_size(px(SMALL)).text_color(c.ink_3).child(say(
            cx,
            if fresh {
                Str::AccountsLoginHint
            } else {
                Str::AccountsReloginHint
            },
        ))
    };
    let content = div()
        .w(px(WIDTH))
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(c.line_strong)
        .bg(c.panel)
        .text_size(px(TEXT))
        .text_color(c.ink)
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(button),
        )
        .child(
            div()
                .w_full()
                .h(px(TERMINAL_H))
                .rounded(px(6.0))
                .overflow_hidden()
                .child(view.clone()),
        )
        .child(foot);
    Some(modal(
        focus,
        content,
        Rc::new(move |_, cx| close(&closer, id, cx)),
        window,
        cx,
    ))
}
