use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::ids::AccountId;
use api::profile::Provider;
use api::settings::accounts::{AccountView, PlaceUsage};
use api::settings::usage::{Limits, UsageProblem, Window as Span};
use chrono::{Datelike, Local, TimeZone};
use gpui::{
    App, Div, FontWeight, Global, ParentElement, Stateful, StatefulInteractiveElement, Styled,
    Window, div, px, relative,
};

use crate::link;
use crate::strings::{Lang, Str};
use crate::theme::Colors;
use crate::views::dock::ui::{SMALL, TEXT, button, colors, say, say_with};

const HIGH_USED: u8 = 75;
const MINUTE_MS: i64 = 60 * 1000;
const HOUR_MIN: i64 = 60;
const DAY_HOUR: i64 = 24;
const METER_H: f32 = 6.0;
const DIALOG_W: f32 = 420.0;
const WEEKDAYS: [(&str, &str); 7] = [
    ("일", "Sun"),
    ("월", "Mon"),
    ("화", "Tue"),
    ("수", "Wed"),
    ("목", "Thu"),
    ("금", "Fri"),
    ("토", "Sat"),
];
const MONTHS_EN: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
pub const ACCOUNTS: &str = "/api/accounts";
pub const REFRESH: Duration = Duration::from_secs(5 * 60);
pub const PROVIDERS: [Provider; 2] = [Provider::Claude, Provider::Codex];

pub struct Line {
    pub label: String,
    pub used: u8,
    pub left: String,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

fn count(cx: &App, unit: Str, n: i64) -> String {
    say_with(cx, unit, &[("n", &n.to_string())])
}

fn joined(cx: &App, big: (Str, i64), small: (Str, i64)) -> String {
    let head = count(cx, big.0, big.1);
    if small.1 == 0 {
        head
    } else {
        format!("{head} {}", count(cx, small.0, small.1))
    }
}

pub fn duration(ms: i64, cx: &App) -> String {
    let minutes = (ms / MINUTE_MS).max(0);
    if minutes < HOUR_MIN {
        return count(cx, Str::UsageUnitMinute, minutes);
    }
    let hours = minutes / HOUR_MIN;
    if hours < DAY_HOUR {
        return joined(
            cx,
            (Str::UsageUnitHour, hours),
            (Str::UsageUnitMinute, minutes % HOUR_MIN),
        );
    }
    joined(
        cx,
        (Str::UsageUnitDay, hours / DAY_HOUR),
        (Str::UsageUnitHour, hours % DAY_HOUR),
    )
}

pub fn fetched(read_at: i64, cx: &App) -> String {
    let seconds = ((now_ms() - read_at * 1000) / 1000).max(0);
    let ago = crate::views::ui::ago(seconds, *cx.global::<Lang>());
    say_with(cx, Str::UsageFetched, &[("ago", &ago)])
}

fn remaining(resets_at: Option<i64>, cx: &App) -> String {
    let Some(at) = resets_at else {
        return String::new();
    };
    let left = at * 1000 - now_ms();
    if left < MINUTE_MS {
        say(cx, Str::UsageSoon).to_owned()
    } else {
        say_with(cx, Str::UsageRemaining, &[("left", &duration(left, cx))])
    }
}

pub fn problem_text(problem: &UsageProblem, cx: &App) -> String {
    let value = |text: Str, value: &str| say_with(cx, text, &[("value", value)]);
    match problem {
        UsageProblem::NotInstalled(v) => value(Str::UsageProblemNotInstalled, v),
        UsageProblem::CliFailed { command, detail } => say_with(
            cx,
            Str::UsageProblemCliFailed,
            &[("command", command), ("detail", detail)],
        ),
        UsageProblem::CliFormat(v) => value(Str::UsageProblemCliFormat, v),
        UsageProblem::NoReply(v) => value(Str::UsageProblemNoReply, v),
        UsageProblem::LoginNotSupported => say(cx, Str::UsageProblemLoginNotSupported).to_owned(),
        UsageProblem::NoToken(v) => value(Str::UsageProblemNoToken, v),
        UsageProblem::ResponseFormat => say(cx, Str::UsageProblemResponseFormat).to_owned(),
        UsageProblem::RateLimited { retry_at } => {
            let wait = (retry_at * 1000 - now_ms()).max(MINUTE_MS);
            value(Str::UsageProblemRateLimited, &duration(wait, cx))
        }
        UsageProblem::TokenExpired(v) => value(Str::UsageProblemTokenExpired, v),
        UsageProblem::Unavailable(v) => value(Str::UsageProblemUnavailable, v),
    }
}

pub fn problem(limits: &Limits, cx: &App) -> Option<String> {
    match limits {
        Limits::Ready { .. } => None,
        Limits::NeedsLogin => Some(say(cx, Str::UsageNeedsLogin).to_owned()),
        Limits::Failed { problem } => Some(problem_text(problem, cx)),
    }
}

pub fn cli_missing(limits: &Limits) -> bool {
    matches!(
        limits,
        Limits::Failed {
            problem: UsageProblem::NotInstalled(_)
        }
    )
}

fn account_windows<'a>(limits: &'a Limits, cx: &App) -> Vec<(String, &'a Span)> {
    let Limits::Ready {
        session, weekly, ..
    } = limits
    else {
        return Vec::new();
    };
    [
        (say(cx, Str::UsageWindowSession), session),
        (say(cx, Str::UsageWindowWeekly), weekly),
    ]
    .into_iter()
    .filter_map(|(label, window)| window.as_ref().map(|window| (label.to_owned(), window)))
    .collect()
}

fn inline_windows<'a>(limits: &'a Limits, cx: &App) -> Vec<(String, &'a Span)> {
    let Limits::Ready { models, .. } = limits else {
        return Vec::new();
    };
    let mut all = account_windows(limits, cx);
    all.extend(models.iter().map(|m| (m.model.clone(), &m.window)));
    all
}

pub fn inline_marks(limits: &Limits, cx: &App) -> Vec<(String, u8)> {
    inline_windows(limits, cx)
        .into_iter()
        .map(|(label, window)| (label, window.used))
        .collect()
}

pub fn lines(limits: &Limits, cx: &App) -> Vec<Line> {
    let mut all = inline_windows(limits, cx);
    if let Limits::Ready {
        plan: Some(window), ..
    } = limits
    {
        all.push((say(cx, Str::UsageWindowPlan).to_owned(), window));
    }
    all.into_iter()
        .map(|(label, window)| Line {
            label,
            used: window.used,
            left: remaining(window.resets_at, cx),
        })
        .collect()
}

fn local(at: i64) -> Option<chrono::DateTime<Local>> {
    Local.timestamp_opt(at, 0).single()
}

pub fn weekday_time(at: i64, lang: Lang) -> String {
    let Some(time) = local(at) else {
        return String::new();
    };
    let (ko, en) = WEEKDAYS[time.weekday().num_days_from_sunday() as usize];
    let day = if lang == Lang::Ko { ko } else { en };
    format!("{day} {}", time.format("%H:%M"))
}

fn month_day(at: i64, lang: Lang) -> String {
    let Some(time) = local(at) else {
        return String::new();
    };
    match lang {
        Lang::Ko => format!("{}월 {}일", time.month(), time.day()),
        Lang::En => format!("{} {}", MONTHS_EN[time.month0() as usize], time.day()),
    }
}

pub fn reset_text(limits: &Limits, cx: &App) -> String {
    let lang = *cx.global::<Lang>();
    let windows = account_windows(limits, cx)
        .into_iter()
        .filter_map(|(label, window)| {
            let time = weekday_time(window.resets_at?, lang);
            Some(say_with(
                cx,
                Str::UsageReset,
                &[("window", &label), ("time", &time)],
            ))
        });
    let plan = match limits {
        Limits::Ready {
            plan: Some(Span {
                resets_at: Some(at),
                ..
            }),
            ..
        } => Some(say_with(
            cx,
            Str::UsageResetsOn,
            &[("date", &month_day(*at, lang))],
        )),
        _ => None,
    };
    windows.chain(plan).collect::<Vec<_>>().join(" \u{00B7} ")
}

pub fn oldest_read(accounts: &[AccountView], cursor: &[PlaceUsage]) -> Option<i64> {
    accounts
        .iter()
        .map(|a| a.usage.read_at)
        .chain(cursor.iter().map(|c| c.usage.read_at))
        .min()
}

pub fn slug(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

pub fn provider_label(provider: Provider, cx: &App) -> &'static str {
    say(
        cx,
        match provider {
            Provider::Claude => Str::SwitchProviderClaude,
            Provider::Codex => Str::SwitchProviderCodex,
        },
    )
}

pub fn unnamed(system: bool, cx: &App) -> String {
    say(
        cx,
        if system {
            Str::UsageSystemDefault
        } else {
            Str::UsageNotLoggedIn
        },
    )
    .to_owned()
}

pub fn account_name(account: &AccountView, cx: &App) -> String {
    account
        .usage
        .email
        .clone()
        .unwrap_or_else(|| unnamed(account.id.is_none(), cx))
}

pub fn active_index(own: &[&AccountView], chosen: Option<&AccountId>) -> Option<usize> {
    own.iter()
        .position(|a| a.id.is_some() && a.id.as_ref() == chosen)
        .or_else(|| own.iter().position(|a| a.id.is_none()))
}

pub struct AccountsChanged(u64);

impl Global for AccountsChanged {}

pub fn accounts_changed(cx: &mut App) {
    let next = cx.try_global::<AccountsChanged>().map_or(1, |g| g.0 + 1);
    cx.set_global(AccountsChanged(next));
}

#[derive(Clone)]
pub struct Switching {
    pub provider: Provider,
    pub distro: Option<String>,
    pub to: Option<AccountId>,
    pub name: String,
    pub nodes: Vec<(String, bool)>,
    pub busy: bool,
}

pub fn movable_nodes(
    base: &str,
    provider: Provider,
    distro: Option<&str>,
    to: Option<&AccountId>,
) -> Vec<(String, bool)> {
    let path = format!(
        "/api/accounts/{}/nodes{}",
        slug(provider),
        link::query(&[("distro", distro), ("to", to.map(|id| id.0.as_str()))])
    );
    link::read::<Vec<serde_json::Value>>(base, &path)
        .unwrap_or_default()
        .iter()
        .map(|node| {
            (
                node["name"].as_str().unwrap_or_default().to_owned(),
                node["busy"].as_bool().unwrap_or_default(),
            )
        })
        .collect()
}

pub fn move_nodes(
    base: &str,
    provider: Provider,
    distro: Option<&str>,
) -> Result<(), crate::problem::Failure> {
    let path = format!(
        "/api/accounts/{}/move{}",
        slug(provider),
        link::query(&[("distro", distro)])
    );
    link::post::<serde_json::Value>(base, &path, None).map(|_| ())
}

pub fn switch_content(
    switching: &Switching,
    on_keep: impl Fn(&mut Window, &mut App) + 'static,
    on_all: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let c = colors(cx);
    let label = provider_label(switching.provider, cx);
    let title = say_with(
        cx,
        Str::SwitchDialogTitle,
        &[("provider", label), ("to", &switching.name)],
    );
    let question = say_with(
        cx,
        Str::SwitchDialogQuestion,
        &[
            ("provider", label),
            ("count", &switching.nodes.len().to_string()),
        ],
    );
    let list = switching.nodes.iter().map(|(node, working)| {
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
    let action = |id: &'static str, text: Str, primary: bool| -> Stateful<Div> {
        let mut action = button(id, cx).min_h(px(26.0)).px(px(8.0));
        if primary {
            action = action.bg(c.accent).text_color(c.accent_ink);
        }
        let action = action.child(say(cx, text));
        if switching.busy {
            action.opacity(0.5)
        } else {
            action
        }
    };
    let keep = action("switch-keep", Str::SwitchDialogNewOnly, false);
    let all = action("switch-all", Str::SwitchDialogMoveAll, true);
    let (keep, all) = if switching.busy {
        (keep, all)
    } else {
        (
            keep.on_click(move |_, window, cx| on_keep(window, cx)),
            all.on_click(move |_, window, cx| on_all(window, cx)),
        )
    };
    div()
        .w(px(DIALOG_W))
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
        )
}

pub fn meter(used: u8, c: &Colors) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .h(px(METER_H))
        .rounded(px(METER_H / 2.0))
        .bg(c.surface_hover)
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(relative(f32::from(used.min(100)) / 100.0))
                .bg(if used >= HIGH_USED { c.warn } else { c.accent }),
        )
}
