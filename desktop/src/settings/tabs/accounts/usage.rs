use std::time::{SystemTime, UNIX_EPOCH};

use api::settings::usage::{Limits, UsageProblem};
use gpui::{App, Div, ParentElement, Styled, div, px, relative};

use crate::strings::Str;
use crate::theme::Colors;
use crate::views::dock::ui::{say, say_with};

const HIGH_USED: u8 = 75;
const MINUTE_MS: i64 = 60 * 1000;
const HOUR_MIN: i64 = 60;
const DAY_HOUR: i64 = 24;
const METER_H: f32 = 6.0;

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
    let minutes = ((now_ms() - read_at * 1000) / MINUTE_MS).max(0);
    let ago = if minutes < 1 {
        say(cx, Str::UsageAgoNow).to_owned()
    } else if minutes < HOUR_MIN {
        count(cx, Str::UsageAgoMinute, minutes)
    } else if minutes < HOUR_MIN * DAY_HOUR {
        count(cx, Str::UsageAgoHour, minutes / HOUR_MIN)
    } else {
        count(cx, Str::UsageAgoDay, minutes / HOUR_MIN / DAY_HOUR)
    };
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

pub fn lines(limits: &Limits, cx: &App) -> Vec<Line> {
    let Limits::Ready {
        session,
        weekly,
        models,
        plan,
    } = limits
    else {
        return Vec::new();
    };
    let named = [
        (say(cx, Str::UsageWindowSession).to_owned(), session),
        (say(cx, Str::UsageWindowWeekly).to_owned(), weekly),
    ]
    .into_iter()
    .filter_map(|(label, window)| window.as_ref().map(|window| (label, window)));
    let by_model = models.iter().map(|m| (m.model.clone(), &m.window));
    let plan = plan
        .as_ref()
        .map(|window| (say(cx, Str::UsageWindowPlan).to_owned(), window));
    named
        .chain(by_model)
        .chain(plan)
        .map(|(label, window)| Line {
            label,
            used: window.used,
            left: remaining(window.resets_at, cx),
        })
        .collect()
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
