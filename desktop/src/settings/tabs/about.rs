use std::time::Duration;

use api::settings::update::{UpdateStep, UpdateView};
use gpui::{
    App, AppContext, Div, Entity, FontWeight, InteractiveElement, ParentElement, Stateful,
    StatefulInteractiveElement, Styled, StyledText, WeakEntity, Window, div, px,
};

use crate::link;
use crate::link::VERSION;
use crate::settings::{Ctx, Dialog, Section};
use crate::strings::{Lang, Str};
use crate::views::dock::ui::{GAP, SMALL, button, colors, say, say_with};

const CHECK_LIMIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(1);
const METER: f32 = 200.0;

enum Outcome {
    Latest,
    New,
    Failed(String),
}

#[derive(Clone, Copy)]
enum Call {
    Get,
    Check,
    Install,
}

#[derive(Default)]
struct Model {
    view: Option<UpdateView>,
    checking: bool,
    outcome: Option<Outcome>,
    started: bool,
    waiting: bool,
    issued: u64,
    applied: u64,
}

#[derive(Default)]
pub struct State {
    model: Option<Entity<Model>>,
}

fn busy(view: &UpdateView) -> bool {
    matches!(
        view.step,
        Some(UpdateStep::Downloading { .. } | UpdateStep::Restarting)
    )
}

fn request(
    model: Entity<Model>,
    dialog: WeakEntity<Dialog>,
    base: String,
    call: Call,
    cx: &mut App,
) {
    let lang = *cx.global::<Lang>();
    let ticket = model.update(cx, |model, _| {
        model.issued += 1;
        model.checking |= matches!(call, Call::Check);
        model.issued
    });
    cx.spawn(async move |cx| {
        let sent = base.clone();
        let reply = cx
            .background_spawn(async move {
                match call {
                    Call::Get => link::read::<UpdateView>(&sent, "/api/update"),
                    Call::Check => link::post_within::<UpdateView>(
                        &sent,
                        "/api/update/check",
                        None,
                        CHECK_LIMIT,
                    ),
                    Call::Install => link::post::<UpdateView>(&sent, "/api/update/install", None),
                }
            })
            .await;
        let poll = model.update(cx, |model, cx| {
            if matches!(call, Call::Check) {
                model.checking = false;
                model.outcome = Some(match &reply {
                    Ok(view) if view.latest.is_some() => Outcome::New,
                    Ok(_) => Outcome::Latest,
                    Err(error) => Outcome::Failed(error.text(lang)),
                });
            }
            if ticket > model.applied {
                match reply {
                    Ok(view) => {
                        model.applied = ticket;
                        model.view = Some(view);
                    }
                    Err(error) if matches!(call, Call::Install) => {
                        if let Some(view) = &mut model.view {
                            view.step = Some(UpdateStep::Failed {
                                message: error.text(lang),
                            });
                        }
                    }
                    Err(_) => {}
                }
            }
            cx.notify();
            let active = model.view.as_ref().is_some_and(busy);
            let poll = active && !model.waiting;
            model.waiting |= poll;
            poll
        });
        let _ = dialog.update(cx, |_, cx| cx.notify());
        if poll {
            cx.background_executor().timer(POLL).await;
            cx.update(|cx| {
                model.update(cx, |model, _| model.waiting = false);
                request(model, dialog, base, Call::Get, cx);
            });
        }
    })
    .detach();
}

fn action(
    id: &'static str,
    label: StyledText,
    enabled: bool,
    accent: bool,
    cx: &App,
) -> Stateful<Div> {
    let c = colors(cx);
    let item = if accent {
        div()
            .id(id)
            .flex()
            .items_center()
            .min_h(px(26.0))
            .px(px(10.0))
            .py(px(3.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(c.accent)
            .bg(c.accent)
            .text_size(px(SMALL))
            .text_color(c.accent_ink)
            .cursor_pointer()
            .child(label)
    } else {
        button(id, cx).child(label)
    };
    if enabled {
        item
    } else {
        item.opacity(0.5).cursor_default()
    }
}

pub fn sections(state: &mut State, ctx: &Ctx, _: &mut Window, cx: &mut App) -> Vec<Section> {
    let model = match &state.model {
        Some(model) => model.clone(),
        None => {
            let model = cx.new(|_| Model::default());
            state.model = Some(model.clone());
            model
        }
    };
    let base = ctx.store.read(cx).base().map(str::to_owned);
    if let Some(base) = &base
        && !model.read(cx).started
    {
        model.update(cx, |model, _| model.started = true);
        request(
            model.clone(),
            ctx.dialog.clone(),
            base.clone(),
            Call::Get,
            cx,
        );
    }

    let c = colors(cx);
    let mut section = Section::new(Some(say(cx, Str::AboutVersion)), ctx);
    let app = section.text(say(cx, Str::AboutApp), cx);
    let versions = div()
        .flex()
        .gap(px(12.0))
        .text_size(px(SMALL))
        .child(div().text_color(c.ink_3).child(app))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("loom v{VERSION}")),
        );
    section = section.child(versions);

    let Some(view) = model.read(cx).view.clone().filter(|view| view.checks) else {
        return vec![section];
    };
    let (checking, outcome) = {
        let model = model.read(cx);
        (
            model.checking,
            match &model.outcome {
                Some(Outcome::Latest) => Some(Ok(false)),
                Some(Outcome::New) => Some(Ok(true)),
                Some(Outcome::Failed(error)) => Some(Err(error.clone())),
                None => None,
            },
        )
    };
    let working = busy(&view);
    let call = |id: &'static str, label: StyledText, enabled: bool, accent: bool, call: Call| {
        let item = action(id, label, enabled, accent, cx);
        match (&base, enabled) {
            (Some(base), true) => {
                let (model, dialog, base) = (model.clone(), ctx.dialog.clone(), base.clone());
                item.on_click(move |_, _, cx| {
                    request(model.clone(), dialog.clone(), base.clone(), call, cx)
                })
            }
            _ => item,
        }
    };

    let check_label = section.text(say(cx, Str::AboutCheck), cx);
    let check = call("about-check", check_label, !checking, false, Call::Check);
    let mut actions = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(GAP))
        .text_size(px(SMALL))
        .child(check);
    match outcome {
        Some(Ok(false)) if view.latest.is_none() => {
            let text = section.text(say(cx, Str::AboutUpToDate), cx);
            actions = actions.child(div().text_color(c.ink_3).child(text));
        }
        Some(Err(error)) => actions = actions.child(div().text_color(c.err).child(error)),
        _ => {}
    }
    if let Some(latest) = &view.latest {
        let text = say_with(cx, Str::AboutNewVersion, &[("version", latest)]);
        let text = section.text(&text, cx);
        let label = section.text(say(cx, Str::UpdateUpdate), cx);
        actions = actions
            .child(div().text_color(c.ink_3).child(text))
            .child(call("about-install", label, !working, true, Call::Install));
    }
    section = section.child(actions);

    match &view.step {
        Some(UpdateStep::Downloading { percent }) => {
            let percent = *percent;
            let text = say_with(cx, Str::UpdatePercent, &[("percent", &percent.to_string())]);
            let text = section.text(&text, cx);
            section = section
                .child(
                    div()
                        .w(px(METER))
                        .h(px(6.0))
                        .rounded(px(3.0))
                        .bg(c.line)
                        .child(
                            div()
                                .h_full()
                                .rounded(px(3.0))
                                .bg(c.accent)
                                .w(px(METER * f32::from(percent) / 100.0)),
                        ),
                )
                .child(div().text_size(px(SMALL)).text_color(c.ink_3).child(text));
        }
        Some(UpdateStep::Restarting) => {
            let text = section.text(say(cx, Str::UpdateRestartingHint), cx);
            section = section.child(div().text_size(px(SMALL)).text_color(c.ink_3).child(text));
        }
        Some(UpdateStep::Failed { message }) => {
            let label = section.text(say(cx, Str::UpdateRetry), cx);
            let retry = call("about-retry", label, true, true, Call::Install);
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .text_size(px(SMALL))
                    .child(div().text_color(c.err).child(message.clone()))
                    .child(retry),
            );
        }
        Some(UpdateStep::Available) | None => {}
    }
    vec![section]
}
