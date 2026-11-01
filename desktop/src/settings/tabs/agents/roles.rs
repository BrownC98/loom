use std::rc::Rc;

use api::ids::AgentId;
use api::profile::{AgentView, ModelRule};
use api::settings::roles::{Role, Roles};
use gpui::{
    AnyElement, App, AppContext, Entity, FocusHandle, FontWeight, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};

use super::parts::{Bound, Shared, ask, hint, keep_focus, labelled, row, rows, small};
use crate::fonts;
use crate::settings::widgets::{Select, SelectEvent, field_reset, inert};
use crate::settings::{Ctx, Section};
use crate::strings::Str;
use crate::views::dock::ui::{SMALL, TEXT, colors, say, say_with};
use crate::views::field::{self, Field, FieldEvent};
use crate::views::icons::{Lucide, icon};

const NAME_W: f32 = 140.0;
const MODEL_W: f32 = 170.0;
const EFFORT_W: f32 = 90.0;
const RNAME_W: f32 = 96.0;
const RUNNER_W: f32 = 250.0;
const FADED: f32 = 0.45;
const DIM: f32 = 0.5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Editing {
    Row(usize),
    New,
}

#[derive(Default)]
struct Local {
    editing: Option<Editing>,
    delete: Option<usize>,
}

#[derive(Default)]
struct Pick {
    options: Vec<Option<AgentId>>,
    agent: Option<AgentId>,
}

struct Form {
    target: Editing,
    name: Entity<Field>,
    model: Entity<Field>,
    effort: Entity<Field>,
    duty: Entity<Field>,
    agent: Entity<Select>,
    pick: Shared<Pick>,
    _subs: Vec<Subscription>,
}

#[derive(Default)]
pub struct State {
    local: Shared<Local>,
    form: Option<Form>,
    seen: Option<Roles>,
    focus: Option<FocusHandle>,
}

fn blank(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn picks_model(agent: Option<&AgentView>) -> bool {
    agent.is_none_or(|agent| agent.model_rule != ModelRule::Fixed)
}

fn effort_needs_model(agent: Option<&AgentView>, model: &str, effort: &str) -> bool {
    agent.is_some_and(|agent| agent.model_rule == ModelRule::EffortNeedsModel)
        && blank(effort).is_some()
        && blank(model).is_none()
}

fn is_installed(agent: &AgentView) -> bool {
    agent.places.iter().any(|at| at.path.is_some())
}

fn runner_text(role: &Role, agents: &[AgentView], cx: &App) -> String {
    let Some(id) = &role.agent else {
        return say(cx, Str::SettingsRolesNone).to_owned();
    };
    let label = agents
        .iter()
        .find(|agent| &agent.id == id)
        .map_or(id.0.as_str(), |agent| agent.label.as_str());
    let default = say(cx, Str::SettingsRolesCliDefault);
    [
        label,
        role.model.as_deref().unwrap_or(default),
        role.effort.as_deref().unwrap_or(default),
    ]
    .join(" \u{00B7} ")
}

impl Form {
    fn new(target: Editing, initial: &Role, ctx: &Ctx, local: &Bound<Local>, cx: &mut App) -> Self {
        let hint = say(cx, Str::SettingsRolesCliDefault);
        let plain = field::Form::Plain;
        let name = field::form("", false, false, plain, cx);
        let model = field::form(hint, false, false, field::Form::Mono, cx);
        let effort = field::form(hint, false, false, field::Form::Mono, cx);
        let duty = field::form(
            say(cx, Str::SettingsRolesDutyPlaceholder),
            false,
            true,
            plain,
            cx,
        );
        for (entity, text) in [
            (&name, initial.name.as_str()),
            (&model, initial.model.as_deref().unwrap_or_default()),
            (&effort, initial.effort.as_deref().unwrap_or_default()),
            (&duty, initial.duty.as_str()),
        ] {
            entity.update(cx, |field, cx| field.set_text(text, cx));
        }
        let agent = cx.new(|cx| Select::new("role-agent", cx));
        let pick = Shared::<Pick>::default();
        pick.write(|pick| {
            pick.agent = initial.agent.clone();
            pick.options = vec![None];
        });
        let dialog = ctx.dialog.clone();
        let mut subs = Vec::new();
        for entity in [&name, &model, &effort, &duty] {
            let (dialog, local) = (dialog.clone(), local.clone());
            subs.push(
                cx.subscribe(entity, move |_, event: &FieldEvent, cx| match event {
                    FieldEvent::Changed => {
                        let _ = dialog.update(cx, |_, cx| cx.notify());
                    }
                    FieldEvent::Escape => local.change(cx, |local| local.editing = None),
                    _ => {}
                }),
            );
        }
        {
            let (pick, dialog) = (pick.clone(), dialog.clone());
            subs.push(cx.subscribe(&agent, move |_, event: &SelectEvent, cx| {
                let SelectEvent::Pick(index) = *event;
                pick.write(|pick| {
                    if let Some(chosen) = pick.options.get(index) {
                        pick.agent = chosen.clone();
                    }
                });
                let _ = dialog.update(cx, |_, cx| cx.notify());
            }));
        }
        Self {
            target,
            name,
            model,
            effort,
            duty,
            agent,
            pick,
            _subs: subs,
        }
    }
}

pub fn section(
    state: &mut State,
    agents: &[AgentView],
    ctx: &Ctx,
    window: &mut Window,
    cx: &mut App,
) -> Section {
    let c = colors(cx);
    let roles = &ctx.view.roles.0;
    if state
        .seen
        .as_ref()
        .is_some_and(|seen| seen != &ctx.view.roles)
    {
        state.local.write(|local| local.editing = None);
    }
    state.seen = Some(ctx.view.roles.clone());
    let local = state.local.bind(ctx);
    let editing = state.local.read(|local| local.editing);
    if state.form.as_ref().map(|form| form.target) != editing {
        state.form = editing.map(|target| {
            let initial = match target {
                Editing::Row(index) => roles.get(index).cloned(),
                Editing::New => None,
            }
            .unwrap_or(Role {
                name: String::new(),
                duty: String::new(),
                agent: None,
                model: None,
                effort: None,
            });
            Form::new(target, &initial, ctx, &local, cx)
        });
    }
    let focus = state.focus.get_or_insert_with(|| cx.focus_handle()).clone();

    let mut section = Section::new(Some(say(cx, Str::SettingsRolesTitle)), ctx);
    let busy = editing.is_some();
    let mut items: Vec<AnyElement> = Vec::new();
    for (index, role) in roles.iter().enumerate() {
        if editing == Some(Editing::Row(index)) {
            if let Some(form) = &state.form {
                items.push(form_element(
                    &mut section,
                    form,
                    agents,
                    roles,
                    ctx,
                    &local,
                    cx,
                ));
            }
            continue;
        }
        let name = section.text(&role.name, cx);
        let duty = section.text(&role.duty, cx);
        let runner = section.text(&runner_text(role, agents, cx), cx);
        let edit_label = section.text(say(cx, Str::SettingsRolesEdit), cx);
        let delete_label = section.text(say(cx, Str::SettingsRolesDelete), cx);
        let mut edit = small(("role-edit", index), cx).child(edit_label);
        let mut delete = small(("role-delete", index), cx).child(delete_label);
        if busy {
            edit = inert(edit).opacity(DIM);
            delete = inert(delete).opacity(DIM);
        } else {
            let (to_edit, to_delete) = (local.clone(), local.clone());
            edit = edit.on_click(move |_, _, cx| {
                to_edit.change(cx, |local| local.editing = Some(Editing::Row(index)))
            });
            delete = delete
                .on_click(move |_, _, cx| to_delete.change(cx, |local| local.delete = Some(index)));
        }
        let mut line = row()
            .items_start()
            .gap(px(14.0))
            .child(
                div()
                    .w(px(RNAME_W))
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name),
            )
            .child(div().flex_1().min_w_0().text_color(c.ink_2).child(duty))
            .child(
                div()
                    .w(px(RUNNER_W))
                    .flex_none()
                    .text_size(px(SMALL))
                    .text_color(if role.agent.is_none() {
                        c.ink_3
                    } else {
                        c.ink_2
                    })
                    .font_family(if role.agent.is_none() {
                        crate::fonts::UI_FAMILY
                    } else {
                        fonts::TERMINAL_FAMILY
                    })
                    .child(runner),
            )
            .child(div().flex().gap(px(4.0)).child(edit).child(delete));
        if busy {
            line = line.opacity(FADED);
        }
        items.push(line.into_any_element());
    }
    if let Some(form) = state.form.as_ref().filter(|f| f.target == Editing::New) {
        items.push(form_element(
            &mut section,
            form,
            agents,
            roles,
            ctx,
            &local,
            cx,
        ));
    }
    let add_label = section.text(say(cx, Str::SettingsRolesAdd), cx);
    let mut add = small("role-add", cx)
        .child(icon(Lucide::Plus, 14.0, colors(cx).ink))
        .child(add_label);
    if busy {
        add = inert(add).opacity(DIM);
    } else {
        let to_add = local.clone();
        add = add.on_click(move |_, _, cx| {
            to_add.change(cx, |local| local.editing = Some(Editing::New))
        });
    }
    let note = section.text(say(cx, Str::SettingsRolesHint), cx);

    let pending = state.local.read(|local| local.delete);
    let confirm = pending.and_then(|index| roles.get(index)).map(|role| {
        keep_focus(&focus, window, cx);
        let message = say_with(cx, Str::SettingsRolesConfirmDelete, &[("name", &role.name)]);
        let (accept, cancel) = (local.clone(), local.clone());
        let writer = ctx.writer.clone();
        let index = pending.unwrap_or_default();
        ask(
            &focus,
            message,
            say(cx, Str::SettingsRolesDelete),
            Rc::new(move |_, cx| {
                accept.change(cx, |local| local.delete = None);
                writer.edit(cx, move |settings| {
                    if index < settings.roles.0.len() {
                        settings.roles.0.remove(index);
                    }
                });
            }),
            Rc::new(move |_, cx| cancel.change(cx, |local| local.delete = None)),
            window,
            cx,
        )
    });
    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(rows(items, cx))
        .child(div().flex().child(add))
        .child(hint(note, cx));
    if !ctx.view.always_on.o_mode {
        body = body.opacity(FADED);
    }
    let section = section
        .beside(field_reset("agents-roles-reset", &["roles"], ctx, cx))
        .child(body);
    match confirm {
        Some(confirm) => section.child(confirm),
        None => section,
    }
}

fn form_element(
    section: &mut Section,
    form: &Form,
    agents: &[AgentView],
    roles: &[Role],
    ctx: &Ctx,
    local: &Bound<Local>,
    cx: &mut App,
) -> AnyElement {
    let c = colors(cx);
    let installed: Vec<&AgentView> = agents.iter().filter(|agent| is_installed(agent)).collect();
    let chosen = form.pick.read(|pick| pick.agent.clone());
    let mut options: Vec<Option<AgentId>> = installed.iter().map(|a| Some(a.id.clone())).collect();
    let mut labels: Vec<SharedString> = installed
        .iter()
        .map(|a| SharedString::from(a.label.clone()))
        .collect();
    if let Some(id) = &chosen
        && !options.contains(&Some(id.clone()))
    {
        options.push(Some(id.clone()));
        labels.push(SharedString::from(id.0.clone()));
    }
    options.push(None);
    labels.push(SharedString::from(say(cx, Str::SettingsRolesNone)));
    let current = options.iter().position(|o| *o == chosen).unwrap_or(0);
    form.pick.write(|pick| pick.options = options);
    form.agent
        .update(cx, |select, cx| select.show(labels, current, cx));

    let selected = agents.iter().find(|a| Some(&a.id) == chosen.as_ref());
    let (name, model, effort, duty) = (
        form.name.read(cx).text().to_owned(),
        form.model.read(cx).text().to_owned(),
        form.effort.read(cx).text().to_owned(),
        form.duty.read(cx).text().to_owned(),
    );
    let others = roles
        .iter()
        .enumerate()
        .filter(|(index, _)| form.target != Editing::Row(*index));
    let trimmed = name.trim().to_owned();
    let taken = trimmed.is_empty() || others.into_iter().any(|(_, r)| r.name == trimmed);
    let duplicate = !trimmed.is_empty() && taken;
    let picks = picks_model(selected);
    let needs_model = effort_needs_model(selected, &model, &effort);

    let boxed = |width: f32, entity: &Entity<Field>, enabled: bool| {
        let mut slot = div().w(px(width));
        if enabled {
            slot = slot.child(entity.clone());
        } else {
            slot = slot.opacity(DIM).child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(c.line_strong)
                    .bg(c.surface)
                    .text_size(px(13.0))
                    .text_color(c.ink_3)
                    .child(say(cx, Str::SettingsRolesCliDefault)),
            );
        }
        slot
    };
    let l_name = section.text(say(cx, Str::SettingsRolesName), cx);
    let l_agent = section.text(say(cx, Str::SettingsRolesAgent), cx);
    let l_model = section.text(say(cx, Str::SettingsRolesModel), cx);
    let l_effort = section.text(say(cx, Str::SettingsRolesEffort), cx);
    let l_duty = section.text(say(cx, Str::SettingsRolesDuty), cx);
    let line = div()
        .flex()
        .flex_wrap()
        .items_end()
        .gap(px(12.0))
        .child(labelled(l_name, boxed(NAME_W, &form.name, true), cx))
        .child(labelled(l_agent, form.agent.clone(), cx))
        .child(labelled(l_model, boxed(MODEL_W, &form.model, picks), cx))
        .child(labelled(l_effort, boxed(EFFORT_W, &form.effort, picks), cx));
    let mut errors = div().flex().flex_col().gap(px(2.0));
    if duplicate {
        let text = section.text(say(cx, Str::SettingsRolesDuplicate), cx);
        errors = errors.child(div().text_size(px(SMALL)).text_color(c.err).child(text));
    }
    if needs_model {
        let text = section.text(say(cx, Str::SettingsRolesCursorNeedsModel), cx);
        errors = errors.child(div().text_size(px(SMALL)).text_color(c.err).child(text));
    }
    let note = section.text(say(cx, Str::SettingsRolesFormHint), cx);
    let cancel_label = section.text(say(cx, Str::SettingsRolesCancel), cx);
    let save_key = if form.target == Editing::New {
        Str::SettingsRolesAddSave
    } else {
        Str::SettingsRolesSave
    };
    let save_label = section.text(say(cx, save_key), cx);

    let to_cancel = local.clone();
    let cancel = small("role-cancel", cx)
        .border_color(gpui::transparent_black())
        .child(cancel_label)
        .on_click(move |_, _, cx| to_cancel.change(cx, |local| local.editing = None));
    let mut save = small("role-save", cx)
        .bg(c.accent)
        .text_color(c.accent_ink)
        .child(save_label);
    if taken || needs_model {
        save = inert(save).opacity(DIM);
    } else {
        let (to_save, writer, target) = (local.clone(), ctx.writer.clone(), form.target);
        let role = Role {
            name: trimmed,
            duty,
            agent: chosen,
            model: if picks { blank(&model) } else { None },
            effort: if picks { blank(&effort) } else { None },
        };
        save = save.on_click(move |_, _, cx| {
            to_save.change(cx, |local| local.editing = None);
            let role = role.clone();
            writer.edit(cx, move |settings| match target {
                Editing::New => settings.roles.0.push(role),
                Editing::Row(index) => {
                    if let Some(slot) = settings.roles.0.get_mut(index) {
                        *slot = role;
                    }
                }
            });
        });
    }
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .p(px(12.0))
        .bg(c.surface)
        .text_size(px(TEXT))
        .child(line)
        .child(errors)
        .child(labelled(l_duty, form.duty.clone(), cx))
        .child(hint(note, cx))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.0))
                .child(cancel)
                .child(save),
        )
        .into_any_element()
}
