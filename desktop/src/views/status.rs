use api::graph::waits::{ScreenWait, WaitKind, WaitTone};
use api::graph::{NodeView, Status};
use chrono::Local;
use gpui::{Div, FontWeight, ParentElement, Rgba, Styled, div, px};

use crate::strings::{Lang, Str};
use crate::theme::Colors;
use crate::views::ui;

pub mod pop;

const REWORK_FROM: u32 = 3;
const PILL_HEIGHT: f32 = 18.0;
const PILL_TEXT: f32 = 11.0;
const SHA_LEN: usize = 7;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ask,
    Problem,
    Working,
    Result,
    Waiting,
    Done,
    Rework,
}

pub struct Chip {
    pub text: String,
    pub tone: Tone,
}

pub fn wash(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

pub fn asking(wait: &ScreenWait) -> bool {
    matches!(wait.tone, WaitTone::Ask) && wait.wait.kind != WaitKind::Stalled
}

pub fn problem(status: &Status) -> bool {
    matches!(
        status,
        Status::SetupFailed { .. } | Status::Stopped { .. } | Status::Limited | Status::Stalled
    )
}

fn text(key: Str, lang: Lang) -> String {
    key.text(lang).to_owned()
}

pub fn chip(status: &Status, lang: Lang) -> Option<Chip> {
    let (text, tone) = match status {
        Status::Human => return None,
        Status::SetupFailed { .. } => (text(Str::NodeStatusChipSetupFailed, lang), Tone::Problem),
        Status::Stopped { code: None } => (text(Str::NodeStatusChipStopped, lang), Tone::Problem),
        Status::Stopped { code: Some(code) } => (
            Str::NodeStatusChipStoppedCode.with(lang, &[("code", &code.to_string())]),
            Tone::Problem,
        ),
        Status::Limited => (text(Str::NodeStatusChipLimited, lang), Tone::Problem),
        Status::Stalled => (text(Str::NodeStatusChipStalled, lang), Tone::Problem),
        Status::Working { .. } => (text(Str::NodeStatusChipWorking, lang), Tone::Working),
        Status::Children { names } => (
            Str::NodeStatusChipChildren.with(lang, &[("n", &names.len().to_string())]),
            Tone::Waiting,
        ),
        Status::Review { .. } => (text(Str::NodeStatusChipReview, lang), Tone::Waiting),
        Status::Merge { .. } => (text(Str::NodeStatusChipMerge, lang), Tone::Waiting),
        Status::Answer => (text(Str::NodeStatusChipAnswer, lang), Tone::Waiting),
        Status::Rereview { .. } => (text(Str::NodeStatusChipRereview, lang), Tone::Waiting),
        Status::Job => (text(Str::NodeStatusChipJob, lang), Tone::Waiting),
        Status::Deploy { .. } => (text(Str::NodeStatusChipDeploy, lang), Tone::Waiting),
        Status::Mail => (text(Str::NodeStatusChipMail, lang), Tone::Waiting),
        Status::Idle => (text(Str::NodeStatusChipIdle, lang), Tone::Waiting),
        Status::Done => (text(Str::NodeStatusChipDone, lang), Tone::Result),
        Status::Merged { .. } => (text(Str::NodeStatusChipMerged, lang), Tone::Result),
    };
    Some(Chip { text, tone })
}

pub fn rework(node: &NodeView, lang: Lang) -> Option<Chip> {
    (node.rework >= REWORK_FROM).then(|| Chip {
        text: Str::NodeStatusChipRework.with(lang, &[("n", &node.rework.to_string())]),
        tone: Tone::Rework,
    })
}

fn sha(sha: &str) -> String {
    sha.chars().take(SHA_LEN).collect()
}

pub fn line(node: &NodeView, parent: Option<&str>, lang: Lang) -> Option<String> {
    let reply = || node.reply.clone();
    match &node.status {
        Status::Human => node
            .waits
            .iter()
            .filter(|w| asking(w))
            .min_by_key(|w| w.wait.since)
            .map(|w| w.summary.clone()),
        Status::SetupFailed { code } => {
            Some(Str::NodeFootSetupFailed.with(lang, &[("code", &code.to_string())]))
        }
        Status::Stopped { code: None } => Some(text(Str::NodeFootDaemonRestarted, lang)),
        Status::Stopped { code: Some(code) } => {
            let code = code.to_string();
            Some(match parent {
                Some(parent) => {
                    Str::NodeFootStoppedSent.with(lang, &[("code", &code), ("parent", parent)])
                }
                None => Str::NodeFootStopped.with(lang, &[("code", &code)]),
            })
        }
        Status::Limited => Some(text(Str::NodeStatusLineLimited, lang)),
        Status::Stalled => Some(text(Str::NodeStatusLineStalled, lang)),
        Status::Working { tool } => tool.clone(),
        Status::Children { names } => Some(names.join(" \u{b7} ")),
        Status::Review {
            branch,
            judged,
            reviewers,
        } => Some(Str::NodeStatusLineReview.with(
            lang,
            &[
                ("reviewers", &reviewers.to_string()),
                ("judged", &judged.to_string()),
                ("branch", branch),
            ],
        )),
        Status::Merge { branch } => Some(branch.clone()),
        Status::Answer => Some(text(Str::NodeStatusLineAnswer, lang)),
        Status::Rereview { branch } => {
            Some(Str::NodeStatusLineRereview.with(lang, &[("branch", branch)]))
        }
        Status::Job | Status::Idle | Status::Done => reply(),
        Status::Deploy { target, sha: id } => {
            Some(Str::NodeStatusLineDeploy.with(lang, &[("target", target), ("sha", &sha(id))]))
        }
        Status::Mail => None,
        Status::Merged { target, sha: id } => {
            Some(Str::NodeStatusLineMerged.with(lang, &[("target", target), ("sha", &sha(id))]))
        }
    }
}

pub fn since_short(since: i64, lang: Lang) -> Option<String> {
    (since > 0).then(|| short(Local::now().timestamp() - since, lang))
}

fn short(seconds: i64, lang: Lang) -> String {
    let minutes = seconds.max(0) / 60;
    let (count, unit) = match (minutes, lang) {
        (0, _) => return ui::ago(0, lang),
        (1..=59, Lang::Ko) => (minutes, "분"),
        (1..=59, Lang::En) => (minutes, "m"),
        (60..=1439, Lang::Ko) => (minutes / 60, "시간"),
        (60..=1439, Lang::En) => (minutes / 60, "h"),
        (_, Lang::Ko) => (minutes / 1440, "일"),
        (_, Lang::En) => (minutes / 1440, "d"),
    };
    format!("{count}{unit}")
}

pub fn pill(text: String, tone: Tone, zoom: f32, c: &Colors) -> Div {
    let (fill, edge, ink) = match tone {
        Tone::Ask => (c.warn, c.warn, c.warn_ink),
        Tone::Problem => (c.err, c.err, c.err_ink),
        Tone::Done => (c.ok, c.ok, c.accent_ink),
        Tone::Working => (wash(c.run, 0.1), wash(c.run, 0.45), c.run),
        Tone::Result => (wash(c.ok, 0.1), wash(c.ok, 0.45), c.ok),
        Tone::Rework => (wash(c.err, 0.1), wash(c.err, 0.45), c.err),
        Tone::Waiting => (c.surface, c.line_strong, c.ink_2),
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(PILL_HEIGHT * zoom))
        .px(px(7.0 * zoom))
        .rounded(px(PILL_HEIGHT / 2.0 * zoom))
        .border(px(zoom.max(0.5)))
        .border_color(edge)
        .bg(fill)
        .text_size(px((PILL_TEXT * zoom).round().max(1.0)))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink)
        .whitespace_nowrap()
        .child(text)
}
