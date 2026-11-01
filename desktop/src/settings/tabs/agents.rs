use api::profile::{AgentAt, AgentView, Profile};
use gpui::{
    App, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, Window, div, px,
};

use crate::fonts;
use crate::settings::widgets::field_reset;
use crate::settings::{Ctx, Section};
use crate::strings::Str;
use crate::views::dock::ui::{SMALL, TEXT, colors, open_url, say, say_with};

mod limits;
pub(super) mod parts;
mod roles;
mod skills;

use parts::{Remote, Shared, badge, hint, place_label, row, rows, small};

const AGENTS: &str = "/api/agents";
const COLUMNS: u16 = 3;

#[derive(Default)]
pub struct State {
    agents: Remote<Vec<AgentView>>,
    more: Shared<bool>,
    limits: limits::State,
    roles: roles::State,
}

pub fn sections(state: &mut State, ctx: &Ctx, window: &mut Window, cx: &mut App) -> Vec<Section> {
    state.agents.ensure(AGENTS, ctx, cx);
    let loaded = state.agents.value();
    let agents: &[AgentView] = loaded.as_deref().map_or(&[], Vec::as_slice);
    vec![
        installed(state, agents, ctx, cx),
        missing(state, agents, ctx, cx),
        skills::section(ctx, cx),
        roles::section(&mut state.roles, agents, ctx, window, cx),
        limits::section(&mut state.limits, ctx, window, cx),
    ]
}

fn at_places(agent: &AgentView) -> impl Iterator<Item = &AgentAt> {
    agent
        .places
        .iter()
        .filter(|at| !matches!(at.place, api::repos::PlaceView::Ssh { .. }))
}

fn is_installed(agent: &AgentView) -> bool {
    at_places(agent).any(|at| at.path.is_some())
}

fn warning(profile: Profile) -> Str {
    match profile {
        Profile::Claude => Str::SettingsVersionWarningClaude,
        Profile::OpenClaude => Str::SettingsVersionWarningOpenclaude,
        Profile::QwenCode => Str::SettingsVersionWarningQwenCode,
        Profile::Codex => Str::SettingsVersionWarningCodex,
        Profile::Copilot => Str::SettingsVersionWarningCopilot,
        Profile::Trae => Str::SettingsVersionWarningTrae,
        Profile::OpenCode2 => Str::SettingsVersionWarningOpencode2,
        Profile::MiMoCode => Str::SettingsVersionWarningMimoCode,
        Profile::Kilo => Str::SettingsVersionWarningKilo,
        Profile::Cursor => Str::SettingsVersionWarningCursor,
        Profile::Pi => Str::SettingsVersionWarningPi,
        Profile::Omp => Str::SettingsVersionWarningOmp,
        Profile::Dsh => Str::SettingsVersionWarningDsh,
        Profile::Antigravity => Str::SettingsVersionWarningAntigravity,
        Profile::Crush => Str::SettingsVersionWarningCrush,
        Profile::Hermes => Str::SettingsVersionWarningHermes,
        Profile::Vibe => Str::SettingsVersionWarningVibe,
    }
}

fn version_text(agent: &AgentView, at: &AgentAt, cx: &App) -> Option<String> {
    at.version_warning.as_ref().map(|found| {
        say_with(
            cx,
            warning(agent.profile),
            &[("installed", &found.installed), ("pinned", &found.pinned)],
        )
    })
}

fn heading(agents: &[AgentView], multi: bool, cx: &App) -> String {
    let count = |index: usize| {
        agents
            .iter()
            .filter(|agent| {
                at_places(agent)
                    .nth(index)
                    .is_some_and(|at| at.path.is_some())
            })
            .count()
    };
    if !multi {
        let n = agents.iter().filter(|agent| is_installed(agent)).count();
        return say_with(cx, Str::SettingsInstalled, &[("n", &n.to_string())]);
    }
    let places = agents.first().map(at_places).into_iter().flatten();
    let parts: Vec<String> = places
        .enumerate()
        .map(|(index, at)| {
            say_with(
                cx,
                Str::SettingsInstalledAt,
                &[
                    ("place", &place_label(&at.place, cx)),
                    ("n", &count(index).to_string()),
                ],
            )
        })
        .collect();
    format!(
        "{} {}",
        say(cx, Str::SettingsInstalledHead),
        parts.join(" \u{00B7} ")
    )
}

fn installed(state: &State, agents: &[AgentView], ctx: &Ctx, cx: &mut App) -> Section {
    let multi = agents
        .first()
        .is_some_and(|agent| at_places(agent).count() > 1);
    let head = heading(agents, multi, cx);
    let mut section = Section::new(Some(&head), ctx);
    let reload = state.agents.reloader(AGENTS, ctx, cx);
    let rescan_label = section.text(say(cx, Str::SettingsRescan), cx);
    let bar = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .children(field_reset("agents-bypass-reset", &["bypassOff"], ctx, cx))
        .child(
            small("agents-rescan", cx)
                .child(rescan_label)
                .on_click(move |_, _, cx| reload.run(cx)),
        );
    let found: Vec<&AgentView> = agents.iter().filter(|agent| is_installed(agent)).collect();
    let body: gpui::AnyElement = if state.agents.value().is_none() {
        let text = state
            .agents
            .error()
            .unwrap_or_else(|| say(cx, Str::StatusBarLoading).to_owned());
        let styled = section.text(&text, cx);
        hint(styled, cx).into_any_element()
    } else if found.is_empty() {
        let styled = section.text(say(cx, Str::SettingsNoneInstalled), cx);
        hint(styled, cx).into_any_element()
    } else {
        let items = found
            .iter()
            .enumerate()
            .map(|(index, agent)| agent_row(&mut section, agent, index, multi, ctx, cx))
            .collect();
        rows(items, cx).into_any_element()
    };
    let bypass_hint = section.text(say(cx, Str::SettingsBypassHint), cx);
    section.child(bar).child(body).child(hint(bypass_hint, cx))
}

fn agent_row(
    section: &mut Section,
    agent: &AgentView,
    index: usize,
    multi: bool,
    ctx: &Ctx,
    cx: &App,
) -> gpui::AnyElement {
    let c = colors(cx);
    let messaging = say(cx, Str::SettingsMessaging);
    let name = section.text(&agent.label, cx);
    let mut head = div().flex().items_center().gap(px(6.0)).child(
        div()
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(TEXT))
            .child(name),
    );
    let mut main = div().flex().flex_col().gap(px(2.0)).flex_1().min_w_0();
    let mono = |text: gpui::StyledText, tone: gpui::Rgba| {
        div()
            .font_family(fonts::TERMINAL_FAMILY)
            .text_size(px(SMALL))
            .text_color(tone)
            .child(text)
    };
    let mut lines = Vec::new();
    if multi {
        for at in at_places(agent) {
            let place = place_label(&at.place, cx);
            let meta = match &at.path {
                Some(path) => format!("{place} {path} \u{00B7} {messaging}"),
                None => format!("{place} {}", say(cx, Str::NewNodeNotInstalled)),
            };
            lines.push(mono(section.text(&meta, cx), c.ink_3));
            if let Some(found) = version_text(agent, at, cx) {
                let warn = format!("{place} {found}");
                lines.push(mono(section.text(&warn, cx), c.warn));
            }
        }
    } else if let Some(at) = at_places(agent).next() {
        if let Some(found) = version_text(agent, at, cx) {
            head = head.child(badge(section.text(&found, cx), true, &c));
        }
        let path = at.path.clone().unwrap_or_default();
        let meta = format!("{path} \u{00B7} {messaging}");
        lines.push(mono(section.text(&meta, cx), c.ink_3));
    }
    main = main.child(head).children(lines);

    let docs_text = section.text(say(cx, Str::SettingsDocs), cx);
    let homepage = agent.homepage.clone();
    let docs = div()
        .id(("agent-docs", index))
        .text_size(px(SMALL))
        .text_color(c.accent)
        .underline()
        .cursor_pointer()
        .child(docs_text)
        .on_click(move |_, _, _| {
            let _ = open_url(&homepage);
        });

    let control = if agent.can_bypass {
        let on = !ctx.view.bypass_off.contains(&agent.id);
        let writer = ctx.writer.clone();
        let id = agent.id.clone();
        let label = section.text(say(cx, Str::SettingsBypass), cx);
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .text_size(px(SMALL))
            .text_color(c.ink_2)
            .child(crate::settings::widgets::switch(
                ("agent-bypass", index),
                on,
                move |next, _, cx| {
                    let id = id.clone();
                    writer.edit(cx, move |settings| {
                        settings.bypass_off.retain(|off| off != &id);
                        if !next {
                            settings.bypass_off.push(id);
                        }
                    });
                },
                cx,
            ))
            .child(label)
            .into_any_element()
    } else {
        let text = section.text(say(cx, Str::SettingsBypassUnsupported), cx);
        div()
            .text_size(px(SMALL))
            .text_color(c.ink_3)
            .child(text)
            .into_any_element()
    };
    row()
        .child(main)
        .child(docs)
        .child(control)
        .into_any_element()
}

fn missing(state: &State, agents: &[AgentView], ctx: &Ctx, cx: &mut App) -> Section {
    let c = colors(cx);
    let mut section = Section::new(None, ctx);
    let absent: Vec<&AgentView> = agents.iter().filter(|agent| !is_installed(agent)).collect();
    let summary = say_with(
        cx,
        Str::SettingsNotInstalled,
        &[("n", &absent.len().to_string())],
    );
    let summary = section.text(&summary, cx);
    let names: Vec<_> = absent
        .iter()
        .map(|agent| section.text(&agent.label, cx))
        .collect();
    let open = state.more.read(|open| *open) || !ctx.query.is_empty();
    let more = state.more.bind(ctx);
    let toggle = div()
        .id("agents-not-installed")
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(TEXT))
        .text_color(c.ink_2)
        .cursor_pointer()
        .child(if open { "\u{25BE}" } else { "\u{25B8}" })
        .child(summary)
        .on_click(move |_, _, cx| more.change(cx, |open| *open = !*open));
    let grid =
        open.then(|| {
            div()
                .grid()
                .grid_cols(COLUMNS)
                .gap_x(px(16.0))
                .gap_y(px(6.0))
                .text_size(px(TEXT))
                .children(names.into_iter().zip(&absent).enumerate().map(
                    |(index, (name, agent))| {
                        let homepage = agent.homepage.clone();
                        div()
                            .id(("agent-missing", index))
                            .text_color(c.accent)
                            .cursor_pointer()
                            .child(name)
                            .on_click(move |_, _, _| {
                                let _ = open_url(&homepage);
                            })
                    },
                ))
        });
    let section = section.child(toggle);
    match grid {
        Some(grid) => section.child(grid),
        None => section,
    }
}
