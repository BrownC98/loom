use std::rc::Rc;
use std::time::Duration;

use api::ids::AccountId;
use api::profile::Provider;
use api::settings::accounts::AccountView;
use gpui::{
    AnyElement, App, AppContext as _, Entity, FocusHandle, FontWeight, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::agents::parts;
use crate::fonts;
use crate::link;
use crate::problem::Failure;
use crate::settings::widgets::{field_reset, switch};
use crate::settings::{Ctx, Section, Writer};
use crate::strings::{Lang, Str};
use crate::views::dock::ui::{SMALL, TEXT, colors, say, say_with};
use crate::views::terminal::TerminalView;
use parts::{Bound, Reload, Remote, Shared, badge, hint, row, rows, small};

mod login;
mod switch;
mod usage;

const ACCOUNTS: &str = "/api/accounts";
const REFRESH: Duration = Duration::from_secs(5 * 60);
const SYSTEM_DIR: [(Provider, &str); 2] = [
    (Provider::Claude, "~/.claude"),
    (Provider::Codex, "~/.codex"),
];
const PROVIDERS: [Provider; 2] = [Provider::Claude, Provider::Codex];
const WINDOWS_W: f32 = 300.0;
const LABEL_W: f32 = 64.0;
const PERCENT_W: f32 = 40.0;
const CURRENT_WASH: f32 = 0.06;

pub(super) enum Ask {
    Remove {
        provider: Provider,
        distro: Option<String>,
        id: AccountId,
        name: String,
    },
    Logout {
        provider: Provider,
        distro: Option<String>,
        name: String,
    },
}

pub(super) struct Login {
    pub id: u32,
    pub title: String,
    pub fresh: bool,
    pub failed: bool,
}

pub(super) struct Switching {
    pub provider: Provider,
    pub distro: Option<String>,
    pub to: Option<AccountId>,
    pub name: String,
    pub nodes: Vec<(String, bool)>,
    pub busy: bool,
}

#[derive(Default)]
pub(super) struct Local {
    pub ask: Option<Ask>,
    pub error: Option<String>,
    pub login: Option<Login>,
    pub switching: Option<Switching>,
}

#[derive(Default)]
pub struct State {
    accounts: Remote<Vec<AccountView>>,
    local: Shared<Local>,
    focus: Option<FocusHandle>,
    terminal: Option<(u32, Entity<TerminalView>)>,
}

#[derive(Clone)]
pub(super) struct Actor {
    pub base: String,
    pub local: Bound<Local>,
    pub reload: Reload<Vec<AccountView>>,
    pub writer: Writer,
}

impl Actor {
    pub fn run(
        &self,
        cx: &mut App,
        work: impl FnOnce(&str) -> Result<(), Failure> + Send + 'static,
    ) {
        self.local.change(cx, |local| local.error = None);
        let lang = *cx.global::<Lang>();
        let (base, actor) = (self.base.clone(), self.clone());
        cx.spawn(async move |cx| {
            let done = cx.background_spawn(async move { work(&base) }).await;
            cx.update(|cx| {
                if let Err(error) = done {
                    actor
                        .local
                        .change(cx, |local| local.error = Some(error.text(lang)));
                }
                actor.reload.run(cx);
            });
        })
        .detach();
    }
}

pub(super) fn slug(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

pub(super) fn provider_label(provider: Provider, cx: &App) -> &'static str {
    say(
        cx,
        match provider {
            Provider::Claude => Str::SwitchProviderClaude,
            Provider::Codex => Str::SwitchProviderCodex,
        },
    )
}

pub(super) fn account_name(account: &AccountView, cx: &App) -> String {
    account.usage.email.clone().unwrap_or_else(|| {
        say(
            cx,
            if account.id.is_none() {
                Str::UsageSystemDefault
            } else {
                Str::UsageNotLoggedIn
            },
        )
        .to_owned()
    })
}

fn place_name(distro: Option<&str>, cx: &App) -> String {
    match distro {
        None => say(cx, Str::PlacePc).to_owned(),
        Some(distro) => say_with(cx, Str::PlaceWslDistro, &[("distro", distro)]),
    }
}

pub fn sections(state: &mut State, ctx: &Ctx, window: &mut Window, cx: &mut App) -> Vec<Section> {
    if state.accounts.ensure(ACCOUNTS, ctx, cx) {
        state
            .accounts
            .reloader(ACCOUNTS, ctx, cx)
            .repeat(REFRESH, cx);
    }
    let loaded = state.accounts.value();
    let all: &[AccountView] = loaded.as_deref().map_or(&[], Vec::as_slice);
    let focus = state.focus.get_or_insert_with(|| cx.focus_handle()).clone();
    let local = state.local.bind(ctx);
    let reload = state.accounts.reloader(ACCOUNTS, ctx, cx);
    let actor = ctx.store.read(cx).base().map(|base| Actor {
        base: base.to_owned(),
        local: local.clone(),
        reload,
        writer: ctx.writer.clone(),
    });
    login::sync(state, &actor, window, cx);

    let mut places: Vec<Option<String>> = Vec::new();
    for account in all {
        if !places.contains(&account.distro) {
            places.push(account.distro.clone());
        }
    }
    let mut sections = vec![auto_switch(places.len() > 1, local.clone(), ctx, cx)];
    let mut ids = 0;
    if loaded.is_none() {
        let mut section = Section::new(None, ctx);
        let text = state
            .accounts
            .error()
            .unwrap_or_else(|| say(cx, Str::AccountsLoading).to_owned());
        let styled = section.text(&text, cx);
        sections.push(section.child(hint(styled, cx)));
    }
    if let (Some(actor), true) = (&actor, loaded.is_some()) {
        if places.len() == 1 {
            for provider in PROVIDERS {
                let label = provider_label(provider, cx);
                let mut section = Section::new(Some(label), ctx);
                let body = provider_body(
                    &mut section,
                    &mut ids,
                    provider,
                    &places[0],
                    all,
                    ctx,
                    actor,
                    cx,
                );
                sections.push(section.child(body));
            }
        } else {
            for distro in &places {
                let name = place_name(distro.as_deref(), cx);
                let mut section = Section::new(Some(&name), ctx);
                let uses = section.text(
                    say(
                        cx,
                        if distro.is_none() {
                            Str::PlacePcUses
                        } else {
                            Str::PlaceWslUses
                        },
                    ),
                    cx,
                );
                let mut blocks = div().flex().flex_col().gap(px(14.0)).child(hint(uses, cx));
                for provider in PROVIDERS {
                    let label = section.text(provider_label(provider, cx), cx);
                    let body = provider_body(
                        &mut section,
                        &mut ids,
                        provider,
                        distro,
                        all,
                        ctx,
                        actor,
                        cx,
                    );
                    blocks = blocks.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(SMALL))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors(cx).ink_3)
                                    .child(label),
                            )
                            .child(body),
                    );
                }
                sections.push(section.child(blocks));
            }
        }
    }
    if let Some(last) = sections.pop() {
        let overlays = overlays(state, &actor, &focus, window, cx);
        sections.push(overlays.into_iter().fold(last, Section::child));
    }
    sections
}

fn auto_switch(multi: bool, local: Bound<Local>, ctx: &Ctx, cx: &mut App) -> Section {
    let c = colors(cx);
    let mut section = Section::new(Some(say(cx, Str::AccountsAutoSwitchHeading)), ctx);
    let label = section.text(say(cx, Str::AccountsAutoSwitch), cx);
    let note = section.text(
        say(
            cx,
            if multi {
                Str::AccountsAutoSwitchHintPlaces
            } else {
                Str::AccountsAutoSwitchHint
            },
        ),
        cx,
    );
    let error = local.read(|local| local.error.clone());
    let error = error.map(|text| section.text(&text, cx));
    let writer = ctx.writer.clone();
    let control = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(px(TEXT))
        .child(switch(
            "accounts-auto-switch",
            ctx.view.auto_switch,
            move |next, _, cx| writer.edit(cx, move |settings| settings.auto_switch = next),
            cx,
        ))
        .child(label);
    let boxed = div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .p(px(12.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(c.line)
        .bg(c.surface)
        .child(
            div()
                .flex()
                .children(field_reset("accounts-auto-reset", &["autoSwitch"], ctx, cx)),
        )
        .child(control)
        .child(hint(note, cx));
    let section = section.child(boxed);
    match error {
        Some(error) => section.child(div().text_size(px(SMALL)).text_color(c.err).child(error)),
        None => section,
    }
}

#[allow(clippy::too_many_arguments)]
fn provider_body(
    section: &mut Section,
    ids: &mut usize,
    provider: Provider,
    distro: &Option<String>,
    all: &[AccountView],
    ctx: &Ctx,
    actor: &Actor,
    cx: &mut App,
) -> AnyElement {
    let own: Vec<&AccountView> = all
        .iter()
        .filter(|a| a.provider == provider && &a.distro == distro)
        .collect();
    let chosen = ctx.view.account(provider, distro.as_deref());
    let active = own
        .iter()
        .position(|a| a.id.is_some() && a.id.as_ref() == chosen)
        .or_else(|| own.iter().position(|a| a.id.is_none()));
    let missing = own
        .iter()
        .any(|a| a.id.is_none() && usage::cli_missing(&a.usage.limits));
    let label = provider_label(provider, cx);
    let add_label = section.text(say(cx, Str::AccountsAdd), cx);
    *ids += 1;
    let mut add = small(("account-add", *ids), cx).child(add_label);
    let mut add_row = div().flex().items_center().gap(px(8.0));
    if missing {
        add = add.opacity(0.5);
        let reason = say_with(cx, Str::AccountsAddNeedsCli, &[("provider", label)]);
        add_row = add_row
            .child(add)
            .child(hint(section.text(&reason, cx), cx));
    } else {
        let (actor, distro) = (actor.clone(), distro.clone());
        let title = say_with(cx, Str::AccountsAddTitle, &[("provider", label)]);
        add_row = add_row.child(add.on_click(move |_, _, cx| {
            login::open(
                &actor,
                provider,
                distro.clone(),
                api::settings::accounts::LoginTarget::New,
                title.clone(),
                true,
                cx,
            )
        }));
    }
    let items = own
        .iter()
        .enumerate()
        .map(|(index, account)| {
            *ids += 1;
            account_row(
                section,
                *ids,
                provider,
                distro,
                account,
                Some(index) == active,
                actor,
                cx,
            )
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(add_row)
        .child(rows(items, cx))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn account_row(
    section: &mut Section,
    id: usize,
    provider: Provider,
    distro: &Option<String>,
    account: &AccountView,
    current: bool,
    actor: &Actor,
    cx: &mut App,
) -> AnyElement {
    let c = colors(cx);
    let limits = &account.usage.limits;
    let system = account.id.is_none();
    let needs_login = matches!(limits, api::settings::usage::Limits::NeedsLogin);
    let absent = usage::cli_missing(limits);
    let name = account_name(account, cx);

    let mut head = div().flex().flex_wrap().items_center().gap(px(6.0)).child(
        div()
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(TEXT))
            .child(section.text(&name, cx)),
    );
    if system && account.usage.email.is_some() {
        head = head.child(badge(
            section.text(say(cx, Str::UsageSystemDefault), cx),
            false,
            &c,
        ));
    }
    if current {
        head = head.child(badge(
            section.text(say(cx, Str::AccountsCurrent), cx),
            false,
            &c,
        ));
    }
    if needs_login {
        let text = say(
            cx,
            if system {
                Str::UsageLoginRequired
            } else {
                Str::AccountsNeedsLogin
            },
        );
        head = head.child(badge(section.text(text, cx), true, &c));
    }
    if absent {
        head = head.child(badge(
            section.text(say(cx, Str::AccountsNotInstalled), cx),
            true,
            &c,
        ));
    }
    let mut meta: Vec<String> = Vec::new();
    if system {
        meta.extend(
            SYSTEM_DIR
                .iter()
                .find(|(p, _)| *p == provider)
                .map(|(_, dir)| (*dir).to_owned()),
        );
    }
    meta.extend(account.usage.plan.clone());
    if !absent {
        meta.push(usage::fetched(account.usage.read_at, cx));
    }
    let meta = section.text(&meta.join(" \u{00B7} "), cx);
    let main = div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .flex_1()
        .min_w_0()
        .child(head)
        .child(
            div()
                .font_family(fonts::TERMINAL_FAMILY)
                .text_size(px(SMALL))
                .text_color(c.ink_3)
                .child(meta),
        );

    let mut windows = div()
        .w(px(WINDOWS_W))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(6.0));
    if needs_login {
        let text = section.text(say(cx, Str::StatusBarLoginToSee), cx);
        windows = windows.child(hint(text, cx));
    } else if let Some(problem) = usage::problem(limits, cx) {
        windows = windows.child(hint(section.text(&problem, cx), cx));
    } else {
        for line in usage::lines(limits, cx) {
            let label = section.text(&line.label, cx);
            let percent = section.text(&format!("{}%", line.used), cx);
            let left = section.text(&line.left, cx);
            windows = windows.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(SMALL))
                    .text_color(c.ink_2)
                    .child(
                        div()
                            .w(px(LABEL_W))
                            .flex_none()
                            .overflow_hidden()
                            .child(label),
                    )
                    .child(usage::meter(line.used, &c))
                    .child(
                        div()
                            .w(px(PERCENT_W))
                            .flex_none()
                            .font_family(fonts::TERMINAL_FAMILY)
                            .child(percent),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(fonts::TERMINAL_FAMILY)
                            .text_color(c.ink_3)
                            .child(left),
                    ),
            );
        }
    }

    let mut actions = div().flex_none().flex().items_center().gap(px(4.0));
    let mut n = 0;
    let mut button = |label: Str, ghost: bool, cx: &mut App| {
        n += 1;
        let mut button =
            small(("account-action", id * 10 + n), cx).child(section.text(say(cx, label), cx));
        if ghost {
            button = button.border_color(gpui::transparent_black()).bg(c.panel);
        }
        button
    };
    let provider_text = provider_label(provider, cx);
    if !current && !needs_login && !absent {
        let (actor, distro, to, name) = (
            actor.clone(),
            distro.clone(),
            account.id.clone(),
            name.clone(),
        );
        actions = actions.child(
            button(Str::AccountsUse, false, cx).on_click(move |_, _, cx| {
                switch::choose(
                    &actor,
                    provider,
                    distro.clone(),
                    to.clone(),
                    name.clone(),
                    cx,
                )
            }),
        );
    }
    if needs_login && let Some(account_id) = &account.id {
        let (actor, distro, target) = (actor.clone(), distro.clone(), account_id.clone());
        let title = say_with(cx, Str::AccountsReloginTitle, &[("name", &name)]);
        actions = actions.child(button(Str::AccountsRelogin, false, cx).on_click(
            move |_, _, cx| {
                let target = api::settings::accounts::LoginTarget::Account(target.clone());
                login::open(
                    &actor,
                    provider,
                    distro.clone(),
                    target,
                    title.clone(),
                    false,
                    cx,
                )
            },
        ));
    }
    if needs_login && system {
        let (actor, distro) = (actor.clone(), distro.clone());
        let title = say_with(
            cx,
            Str::AccountsSystemLoginTitle,
            &[("provider", provider_text)],
        );
        actions = actions.child(
            button(Str::AccountsLogin, false, cx).on_click(move |_, _, cx| {
                let target = api::settings::accounts::LoginTarget::System;
                login::open(
                    &actor,
                    provider,
                    distro.clone(),
                    target,
                    title.clone(),
                    false,
                    cx,
                )
            }),
        );
    }
    if system && !needs_login && !absent {
        let (local, distro, name) = (actor.local.clone(), distro.clone(), name.clone());
        actions = actions.child(
            button(Str::AccountsLogout, true, cx).on_click(move |_, _, cx| {
                let ask = Ask::Logout {
                    provider,
                    distro: distro.clone(),
                    name: name.clone(),
                };
                local.change(cx, |local| local.ask = Some(ask));
            }),
        );
    }
    if let Some(account_id) = &account.id {
        let (local, distro, account_id, name) = (
            actor.local.clone(),
            distro.clone(),
            account_id.clone(),
            name.clone(),
        );
        actions = actions.child(
            button(Str::AccountsRemove, true, cx).on_click(move |_, _, cx| {
                let ask = Ask::Remove {
                    provider,
                    distro: distro.clone(),
                    id: account_id.clone(),
                    name: name.clone(),
                };
                local.change(cx, |local| local.ask = Some(ask));
            }),
        );
    }

    let mut line = row()
        .gap(px(16.0))
        .child(main)
        .child(windows)
        .child(actions);
    if current {
        line = line.bg(gpui::Hsla::from(c.accent).opacity(CURRENT_WASH));
    }
    line.into_any_element()
}

fn overlays(
    state: &mut State,
    actor: &Option<Actor>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement> {
    let Some(actor) = actor else {
        return Vec::new();
    };
    let mut shown = Vec::new();
    if let Some((message, accept, path, post)) = state
        .local
        .read(|local| local.ask.as_ref().map(|ask| plan(ask, cx)))
    {
        parts::keep_focus(focus, window, cx);
        let (accepted, cancelled) = (actor.clone(), actor.clone());
        shown.push(parts::ask(
            focus,
            message,
            say(cx, accept),
            Rc::new(move |_, cx| {
                accepted.local.change(cx, |local| local.ask = None);
                let path = path.clone();
                accepted.run(cx, move |base| {
                    if post {
                        link::post::<()>(base, &path, None)
                    } else {
                        link::delete(base, &path)
                    }
                });
            }),
            Rc::new(move |_, cx| cancelled.local.change(cx, |local| local.ask = None)),
            window,
            cx,
        ));
    }
    shown.extend(switch::dialog(state, actor, focus, window, cx));
    shown.extend(login::dialog(state, actor, window, cx));
    shown
}

fn plan(ask: &Ask, cx: &App) -> (String, Str, String, bool) {
    match ask {
        Ask::Remove {
            provider,
            distro,
            id,
            name,
        } => (
            say_with(cx, Str::AccountsConfirmRemove, &[("name", name)]),
            Str::AccountsRemove,
            format!(
                "/api/accounts/{}/{}{}",
                slug(*provider),
                id.0,
                link::query(&[("distro", distro.as_deref())])
            ),
            false,
        ),
        Ask::Logout {
            provider,
            distro,
            name,
        } => (
            say_with(
                cx,
                Str::AccountsConfirmLogout,
                &[("provider", provider_label(*provider, cx)), ("name", name)],
            ),
            Str::AccountsLogout,
            format!(
                "/api/accounts/{}/logout{}",
                slug(*provider),
                link::query(&[("distro", distro.as_deref())])
            ),
            true,
        ),
    }
}
