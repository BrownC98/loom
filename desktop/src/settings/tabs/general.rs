use std::collections::BTreeMap;

use api::repos::Locations;
use gpui::{
    App, AppContext, Div, ElementId, Entity, ParentElement, Stateful, Styled, Subscription,
    WeakEntity, Window, div, px,
};

use crate::link;
use crate::problem::Failure;
use crate::settings::widgets::{Text, TextEvent, field_reset, switch};
use crate::settings::{Ctx, Dialog, Section};
use crate::strings::Str;
use crate::views::dock::ui::{GAP, SMALL, colors, say, say_with};

const PC: &str = "";

#[derive(Default)]
struct Places {
    started: bool,
    distros: Vec<String>,
    defaults: BTreeMap<String, String>,
    errors: BTreeMap<String, String>,
}

#[derive(Default)]
pub struct State {
    places: Option<Entity<Places>>,
    fields: BTreeMap<String, Entity<Text>>,
    subs: Vec<Subscription>,
}

fn absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || path.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

fn load(places: Entity<Places>, base: String, dialog: WeakEntity<Dialog>, cx: &mut App) {
    cx.spawn(async move |cx| {
        let found = cx
            .background_spawn(async move {
                let distros = link::read::<Locations>(&base, "/api/locations")?.distros;
                let defaults = distros
                    .iter()
                    .filter_map(|name| {
                        let view = link::distro(&base, name).ok()?;
                        Some((name.clone(), view.workspace))
                    })
                    .collect();
                Ok::<_, Failure>((distros, defaults))
            })
            .await;
        places.update(cx, |places, cx| {
            if let Ok((distros, defaults)) = found {
                places.distros = distros;
                places.defaults = defaults;
            }
            cx.notify();
        });
        let _ = dialog.update(cx, |_, cx| cx.notify());
    })
    .detach();
}

fn field(
    state: &mut State,
    key: &str,
    places: &Entity<Places>,
    ctx: &Ctx,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Text> {
    if let Some(found) = state.fields.get(key) {
        return found.clone();
    }
    let text = cx.new(|cx| Text::new("", false, window, cx).keep_draft());
    let writer = ctx.writer.clone();
    let dialog = ctx.dialog.clone();
    let places = places.clone();
    let owner = key.to_owned();
    state
        .subs
        .push(cx.subscribe(&text, move |_, event: &TextEvent, cx| {
            let TextEvent::Commit(draft) = event;
            let value = draft.trim().to_owned();
            let valid = absolute(&value);
            places.update(cx, |places, _| {
                if valid {
                    places.errors.remove(&owner);
                } else {
                    places.errors.insert(owner.clone(), value.clone());
                }
            });
            if valid {
                let owner = owner.clone();
                writer.edit(cx, move |settings| {
                    if owner == PC {
                        settings.workspace_dir = Some(value);
                    } else {
                        settings.wsl_workspace_dirs.insert(owner, value);
                    }
                });
            }
            let _ = dialog.update(cx, |_, cx| cx.notify());
        }));
    state.fields.insert(key.to_owned(), text.clone());
    text
}

struct Dir<'a> {
    label: String,
    owner: &'a str,
    saved: String,
    note: Option<Str>,
    text: Entity<Text>,
    reset: Option<Stateful<Div>>,
}

fn dir_section(
    dir: Dir,
    errors: &BTreeMap<String, String>,
    ctx: &Ctx,
    window: &Window,
    cx: &mut App,
) -> Section {
    dir.text
        .update(cx, |text, cx| text.show(&dir.saved, window, cx));
    let c = colors(cx);
    let mut section = Section::new(Some(&dir.label), ctx);
    let row = div()
        .flex()
        .items_center()
        .gap(px(GAP))
        .child(div().flex_1().child(dir.text.clone()))
        .children(dir.reset);
    section = section.child(row);
    if let Some(value) = errors.get(dir.owner) {
        let message = say_with(cx, Str::ProblemRepoNotAbsolute, &[("value", value)]);
        section.child(div().text_size(px(SMALL)).text_color(c.err).child(message))
    } else if let Some(note) = dir.note {
        let hint = section.text(say(cx, note), cx);
        section.child(div().text_size(px(SMALL)).text_color(c.ink_3).child(hint))
    } else {
        section
    }
}

pub fn sections(state: &mut State, ctx: &Ctx, window: &mut Window, cx: &mut App) -> Vec<Section> {
    let places = match &state.places {
        Some(places) => places.clone(),
        None => {
            let places = cx.new(|_| Places::default());
            state.places = Some(places.clone());
            places
        }
    };
    if let Some(base) = ctx.store.read(cx).base().map(str::to_owned)
        && !places.read(cx).started
    {
        places.update(cx, |places, _| places.started = true);
        load(places.clone(), base, ctx.dialog.clone(), cx);
    }

    let snapshot = places.read(cx);
    let distros = snapshot.distros.clone();
    let defaults = snapshot.defaults.clone();
    let errors = snapshot.errors.clone();
    let many = !distros.is_empty();

    let pc_label = if many {
        let place = say(cx, Str::PlacePc);
        say_with(cx, Str::WorkspaceFolderAt, &[("place", place)])
    } else {
        say(cx, Str::WorkspaceFolder).to_owned()
    };
    let pc_text = field(state, PC, &places, ctx, window, cx);
    let pc = Dir {
        label: pc_label,
        owner: PC,
        saved: ctx.view.workspace_dir.clone().unwrap_or_default(),
        note: (!many).then_some(Str::WorkspaceHint),
        text: pc_text,
        reset: field_reset("reset-workspace-dir", &["workspaceDir"], ctx, cx),
    };
    let mut out = vec![dir_section(pc, &errors, ctx, window, cx)];
    for (index, distro) in distros.iter().enumerate() {
        let place = say_with(cx, Str::PlaceWslDistro, &[("distro", distro)]);
        let label = say_with(cx, Str::WorkspaceFolderAt, &[("place", &place)]);
        let saved = ctx
            .view
            .wsl_workspace_dirs
            .get(distro)
            .or(defaults.get(distro))
            .cloned()
            .unwrap_or_default();
        let text = field(state, distro, &places, ctx, window, cx);
        let note = (index + 1 == distros.len()).then_some(Str::WorkspaceHintPlaces);
        let dir = Dir {
            label,
            owner: distro,
            saved,
            note,
            text,
            reset: field_reset(
                ElementId::NamedInteger("reset-wsl-dir".into(), index as u64),
                &["wslWorkspaceDirs", distro],
                ctx,
                cx,
            ),
        };
        out.push(dir_section(dir, &errors, ctx, window, cx));
    }

    let c = colors(cx);
    let on = ctx.view.os_notify;
    let writer = ctx.writer.clone();
    let mut notify = Section::new(Some(say(cx, Str::OsNotifyHeading)), ctx);
    let label = notify.text(say(cx, Str::OsNotifyLabel), cx);
    let row = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(px(SMALL))
        .text_color(c.ink_2)
        .child(switch(
            "os-notify",
            on,
            move |value, _, cx| writer.edit(cx, |settings| settings.os_notify = value),
            cx,
        ))
        .child(label)
        .children(field_reset("reset-os-notify", &["osNotify"], ctx, cx));
    out.push(notify.child(row));
    out
}
