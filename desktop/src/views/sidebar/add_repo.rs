use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, BoxShadow, ClickEvent, Context, Entity, EventEmitter, FocusHandle, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Point, Render,
    StatefulInteractiveElement, Styled, Window, div, point, px,
};

use api::ids::RepoId;
use api::repos::{FolderList, Locations, PlaceView};
use api::settings::places::{ReachView, SystemView};

use super::calls;
use super::pop::{self, Closed};
use crate::fonts;
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::theme::{Colors, Theme};
use crate::views::field::{self, Field, FieldEvent, Form};
use crate::views::icons::{Lucide, icon};
use crate::views::worktree::{Kind, actions, button, error, hint, labelled, shell, title};

const WIDTH: f32 = 440.0;
const DIRS: f32 = 168.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Folder,
    Clone,
    Create,
}

const TABS: [Tab; 3] = [Tab::Folder, Tab::Clone, Tab::Create];

impl Tab {
    fn name(self, lang: Lang) -> &'static str {
        match self {
            Tab::Folder => Str::RepoTabFolder,
            Tab::Clone => Str::RepoTabClone,
            Tab::Create => Str::RepoTabCreate,
        }
        .text(lang)
    }

    fn submit(self, lang: Lang) -> &'static str {
        match self {
            Tab::Folder => Str::RepoAdd,
            Tab::Clone => Str::RepoClone,
            Tab::Create => Str::RepoCreate,
        }
        .text(lang)
    }

    fn busy(self, lang: Lang) -> &'static str {
        match self {
            Tab::Folder => Str::RepoBusyFolder,
            Tab::Clone => Str::RepoBusyClone,
            Tab::Create => Str::RepoBusyCreate,
        }
        .text(lang)
    }
}

pub struct AddRepo {
    store: Entity<Store>,
    hosts: Vec<String>,
    at: Point<Pixels>,
    focus: FocusHandle,
    tab: Tab,
    dir_field: Entity<Field>,
    url_field: Entity<Field>,
    name_field: Entity<Field>,
    host_field: Entity<Field>,
    dir: String,
    picked: String,
    list: Option<FolderList>,
    failed: String,
    selected: Option<String>,
    open: bool,
    locations: Option<Locations>,
    waiting: Option<String>,
    asking: bool,
    connecting: Option<String>,
    reach: Option<(String, ReachView)>,
    visited: HashSet<String>,
    latest: u64,
    busy: bool,
    error: Option<String>,
}

impl EventEmitter<Closed> for AddRepo {}

fn act(
    cx: &Context<AddRepo>,
    run: fn(&mut AddRepo, &mut Context<AddRepo>),
) -> impl Fn(&mut Window, &mut gpui::App) + 'static {
    let view = cx.entity();
    move |_, app| view.update(app, run)
}

fn child_path(parent: &str, name: &str) -> String {
    let sep = if parent.contains('\\') && !parent.contains('/') {
        '\\'
    } else {
        '/'
    };
    if parent.ends_with(sep) {
        format!("{parent}{name}")
    } else {
        format!("{parent}{sep}{name}")
    }
}

fn repo_name(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let leaf = url.rsplit(['/', ':']).next().unwrap_or_default();
    leaf.strip_suffix(".git").unwrap_or(leaf).to_owned()
}

fn host_key(host: &str) -> String {
    host.to_lowercase()
}

fn ssh_host(path: &str) -> Option<&str> {
    let rest = path.get(..6).filter(|p| p.eq_ignore_ascii_case("ssh://"))?;
    let rest = &path[rest.len()..];
    rest.split('/').next().filter(|h| !h.is_empty())
}

fn unique(hosts: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for host in hosts {
        if !out.iter().any(|h| host_key(h) == host_key(&host)) {
            out.push(host);
        }
    }
    out
}

fn reach_text(host: &str, reach: &ReachView, lang: Lang) -> Option<String> {
    Some(match reach {
        ReachView::Reached { .. } => return None,
        ReachView::UnknownHostKey { folder } => {
            Str::PlaceUnknownHostKey.with(lang, &[("host", host), ("folder", folder)])
        }
        ReachView::AuthFailed { folder } => {
            Str::PlaceAuthFailed.with(lang, &[("host", host), ("folder", folder)])
        }
        ReachView::Unreachable { cause } => {
            Str::PlaceUnreachable.with(lang, &[("host", host), ("cause", cause)])
        }
        ReachView::UnsupportedMac { cpu } => {
            Str::PlaceUnsupportedMac.with(lang, &[("host", host), ("cpu", cpu)])
        }
        ReachView::VersionMismatch => Str::PlaceVersionMismatch.with(lang, &[("host", host)]),
        ReachView::Failed { cause } => {
            Str::PlaceFailed.with(lang, &[("host", host), ("cause", cause)])
        }
    })
}

fn system_text(system: &SystemView, lang: Lang) -> String {
    match system {
        SystemView::Linux => Str::PlaceLinux.text(lang).to_owned(),
        SystemView::Mac => Str::PlaceMac.text(lang).to_owned(),
        SystemView::Windows { distro } => Str::PlaceWindowsWsl.with(lang, &[("distro", distro)]),
    }
}

fn run_line(place: &PlaceView, lang: Lang) -> String {
    match place {
        PlaceView::Pc => Str::PlaceRun.with(lang, &[("place", Str::PlacePc.text(lang))]),
        PlaceView::Wsl { distro } => Str::PlaceRun.with(
            lang,
            &[(
                "place",
                &Str::PlaceWslDistro.with(lang, &[("distro", distro)]),
            )],
        ),
        PlaceView::Ssh { host } => Str::PlaceRunSsh.with(lang, &[("host", host)]),
    }
}

impl AddRepo {
    pub fn new(
        store: Entity<Store>,
        hosts: Vec<String>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let dir_field = field::form("", false, false, Form::Mono, cx);
        let url_field = field::form(
            "https://github.com/team/app.git",
            false,
            false,
            Form::Mono,
            cx,
        );
        let name_field = field::form("my-app", false, false, Form::Plain, cx);
        let host_field = field::form("", false, false, Form::Mono, cx);
        cx.subscribe(
            &dir_field,
            |this, field, event: &FieldEvent, cx| match event {
                FieldEvent::Changed => {
                    let text = field.read(cx).text().to_owned();
                    if text != this.dir {
                        this.open = true;
                        this.go(text, None, cx);
                    }
                }
                FieldEvent::Enter => this.submit(cx),
                FieldEvent::Escape => this.dismiss(cx),
                FieldEvent::Step(_) => {}
            },
        )
        .detach();
        for field in [&url_field, &name_field] {
            cx.subscribe(field, |this, _, event: &FieldEvent, cx| match event {
                FieldEvent::Changed => {
                    this.error = None;
                    cx.notify();
                }
                FieldEvent::Enter => this.submit(cx),
                FieldEvent::Escape => this.dismiss(cx),
                FieldEvent::Step(_) => {}
            })
            .detach();
        }
        cx.subscribe(
            &host_field,
            |this, field, event: &FieldEvent, cx| match event {
                FieldEvent::Enter => {
                    let host = field.read(cx).text().to_owned();
                    this.connect(host, cx);
                }
                FieldEvent::Escape => this.dismiss(cx),
                _ => cx.notify(),
            },
        )
        .detach();
        let handle = dir_field.read(cx).handle();
        window.focus(&handle, cx);
        let mut this = Self {
            store,
            hosts,
            at,
            focus: cx.focus_handle(),
            tab: Tab::Folder,
            dir_field,
            url_field,
            name_field,
            host_field,
            dir: String::new(),
            picked: String::new(),
            list: None,
            failed: String::new(),
            selected: None,
            open: true,
            locations: None,
            waiting: None,
            asking: false,
            connecting: None,
            reach: None,
            visited: HashSet::new(),
            latest: 0,
            busy: false,
            error: None,
        };
        this.load(None, cx);
        let store = this.store.clone();
        calls::run(&store, cx, calls::locations, |this, done, cx| {
            this.locations = done.ok();
            cx.notify();
        });
        this
    }

    fn connect_typed(&mut self, cx: &mut Context<Self>) {
        let host = self.host_field.read(cx).text().to_owned();
        self.connect(host, cx);
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let store = self.store.clone();
        for host in self.visited.drain() {
            calls::run(
                &store,
                cx,
                move |url| calls::release_ssh(url, &host),
                |_, _, _| {},
            );
        }
        cx.emit(Closed);
    }

    fn load(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        self.latest += 1;
        let ask = self.latest;
        if let Some(host) = path.as_deref().and_then(ssh_host) {
            self.visited.insert(host.to_owned());
        }
        let start = path.is_none();
        let store = self.store.clone();
        calls::run(
            &store,
            cx,
            move |url| calls::folders(url, path.as_deref()),
            move |this, done, cx| {
                if ask != this.latest {
                    return;
                }
                match done {
                    Ok(list) => {
                        this.failed.clear();
                        this.waiting = None;
                        if matches!(list.place, PlaceView::Ssh { .. }) {
                            this.asking = false;
                        }
                        if start {
                            let path = list.path.clone();
                            this.set_dir(&path, cx);
                            this.picked = path;
                        }
                        this.list = Some(list);
                    }
                    Err(error) => {
                        this.failed = error;
                        this.waiting = None;
                    }
                }
                cx.notify();
            },
        );
    }

    fn set_dir(&mut self, path: &str, cx: &mut Context<Self>) {
        self.dir = path.to_owned();
        self.dir_field
            .update(cx, |field, cx| field.set_text_at_end(path, cx));
    }

    fn go(&mut self, path: String, distro: Option<String>, cx: &mut Context<Self>) {
        self.connecting = None;
        self.reach = None;
        self.set_dir(&path, cx);
        self.selected = None;
        self.picked = path.clone();
        self.list = None;
        self.failed.clear();
        self.waiting = distro;
        if path.trim().is_empty() {
            self.latest += 1;
        } else {
            self.load(Some(path.trim().to_owned()), cx);
        }
        cx.notify();
    }

    fn home(&mut self, distro: String, cx: &mut Context<Self>) {
        self.latest += 1;
        let ask = self.latest;
        self.asking = false;
        self.open = true;
        self.list = None;
        self.failed.clear();
        self.waiting = Some(distro.clone());
        let name = distro.clone();
        let store = self.store.clone();
        calls::run(
            &store,
            cx,
            move |url| calls::distro_home(url, &name),
            move |this, done, cx| {
                if ask != this.latest {
                    return;
                }
                match done {
                    Ok(home) => this.go(home, Some(distro), cx),
                    Err(error) => {
                        this.failed = error;
                        this.waiting = None;
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn connect(&mut self, host: String, cx: &mut Context<Self>) {
        let host = host.trim().to_owned();
        if host.is_empty() {
            return;
        }
        self.latest += 1;
        let ask = self.latest;
        self.open = true;
        self.list = None;
        self.failed.clear();
        self.reach = None;
        self.connecting = Some(host.clone());
        self.visited.insert(host.clone());
        let name = host.clone();
        let store = self.store.clone();
        calls::run(
            &store,
            cx,
            move |url| calls::connect_ssh(url, &name),
            move |this, done, cx| {
                if ask != this.latest {
                    return;
                }
                this.connecting = None;
                match done {
                    Ok(view) => {
                        if let ReachView::Reached { home, .. } = &view {
                            let path = format!("ssh://{host}{home}");
                            this.go(path, None, cx);
                        }
                        this.reach = Some((host, view));
                    }
                    Err(error) => this.failed = error,
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn choose(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        self.error = None;
        let target = match tab {
            Tab::Folder => &self.dir_field,
            Tab::Clone => &self.url_field,
            Tab::Create => &self.name_field,
        };
        let handle = target.read(cx).handle();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn target(&self, cx: &gpui::App) -> String {
        let path = self.picked.trim();
        match self.tab {
            Tab::Folder => path.to_owned(),
            Tab::Clone => {
                let name = repo_name(self.url_field.read(cx).text());
                if name.is_empty() {
                    String::new()
                } else {
                    child_path(path, &name)
                }
            }
            Tab::Create => {
                let name = self.name_field.read(cx).text().trim().to_owned();
                if name.is_empty() {
                    String::new()
                } else {
                    child_path(path, &name)
                }
            }
        }
    }

    fn ready(&self, cx: &gpui::App) -> bool {
        !self.picked.trim().is_empty()
            && match self.tab {
                Tab::Folder => true,
                Tab::Clone => !self.url_field.read(cx).text().trim().is_empty(),
                Tab::Create => !self.name_field.read(cx).text().trim().is_empty(),
            }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy || !self.ready(cx) {
            return;
        }
        self.busy = true;
        self.error = None;
        let parent = self.picked.trim().to_owned();
        let url = self.url_field.read(cx).text().to_owned();
        let name = self.name_field.read(cx).text().to_owned();
        let tab = self.tab;
        let store = self.store.clone();
        calls::run(
            &store,
            cx,
            move |base| match tab {
                Tab::Folder => calls::add_repo(base, &parent),
                Tab::Clone => calls::clone_repo(base, &url, &parent),
                Tab::Create => calls::create_repo(base, &parent, &name),
            },
            |this, done, cx| match done {
                Ok(id) => {
                    this.store.update(cx, |store, cx| {
                        store.choose_repo(RepoId(id), cx);
                        store.reload(cx);
                    });
                    this.busy = false;
                    this.dismiss(cx);
                }
                Err(error) => {
                    this.error = Some(error);
                    this.busy = false;
                    cx.notify();
                }
            },
        );
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if key == "escape" {
            self.dismiss(cx);
            cx.stop_propagation();
        }
    }

    fn chip(
        &self,
        id: String,
        glyph: Option<Lucide>,
        text: String,
        mono: bool,
        pressed: bool,
        c: &Colors,
    ) -> gpui::Stateful<gpui::Div> {
        let hover = c.surface_hover;
        let mut chip = div()
            .id(gpui::SharedString::from(id))
            .h(px(26.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(px(13.0))
            .border_1()
            .border_color(if pressed { c.accent } else { c.line_strong })
            .text_size(px(12.0))
            .text_color(if pressed { c.ink } else { c.ink_2 })
            .cursor_pointer();
        chip = if pressed {
            chip.bg(c.accent_wash)
        } else {
            chip.hover(move |s| s.bg(hover))
        };
        let label = div().child(text);
        let label = if mono {
            label.font_family(fonts::TERMINAL_FAMILY)
        } else {
            label
        };
        chip.children(glyph.map(|g| icon(g, 14.0, c.ink_2)))
            .child(label)
    }

    fn picker(&self, lang: Lang, c: &Colors, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = Vec::new();
        let label = if self.tab == Tab::Folder {
            Str::RepoPath
        } else {
            Str::RepoParent
        };
        out.push(
            labelled(
                label.text(lang),
                div()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.open = true;
                            cx.notify();
                        }),
                    )
                    .child(self.dir_field.clone()),
                c,
            )
            .into_any_element(),
        );
        let places = self
            .locations
            .as_ref()
            .is_some_and(|l| !l.distros.is_empty());
        let in_list = self.list.as_ref().map(|l| &l.place);
        let list_host = match in_list {
            Some(PlaceView::Ssh { host }) => Some(host.clone()),
            _ => None,
        };
        let dir_lower = self.dir.to_lowercase();
        let mut chips = div().flex().flex_wrap().gap(px(6.0)).mt(px(-4.0));
        if let (true, Some(locations)) = (places, &self.locations) {
            for (i, drive) in locations.drives.iter().enumerate() {
                let pressed = !self.asking
                    && matches!(in_list, Some(PlaceView::Pc))
                    && self
                        .list
                        .as_ref()
                        .is_some_and(|l| l.path.to_uppercase().starts_with(&drive.to_uppercase()));
                let drive_path = drive.clone();
                chips = chips.child(
                    self.chip(
                        format!("drive-{drive}"),
                        (i == 0).then_some(Lucide::Monitor),
                        drive.clone(),
                        true,
                        pressed,
                        c,
                    )
                    .on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            this.asking = false;
                            this.open = true;
                            this.go(drive_path.clone(), None, cx);
                        },
                    )),
                );
            }
            for distro in &locations.distros {
                let shown = match in_list {
                    Some(PlaceView::Wsl { distro }) => Some(distro.as_str()),
                    _ => None,
                }
                .or(self.waiting.as_deref());
                let pressed = !self.asking && shown == Some(distro.as_str());
                let name = distro.clone();
                chips = chips.child(
                    self.chip(
                        format!("distro-{distro}"),
                        Some(Lucide::Terminal),
                        Str::PlaceWslDistro.with(lang, &[("distro", distro)]),
                        false,
                        pressed,
                        c,
                    )
                    .on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            this.home(name.clone(), cx);
                        },
                    )),
                );
            }
        }
        let known = self.hosts.iter().cloned().chain(list_host.clone());
        for host in unique(known) {
            let pressed =
                !self.asking && dir_lower.starts_with(&format!("ssh://{}/", host.to_lowercase()));
            let name = host.clone();
            chips = chips.child(
                self.chip(
                    format!("host-{host}"),
                    Some(Lucide::Globe),
                    Str::PlaceSshHost.with(lang, &[("host", &host)]),
                    false,
                    pressed,
                    c,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.asking = false;
                    this.connect(name.clone(), cx);
                })),
            );
        }
        chips = chips.child(
            self.chip(
                "host-ask".to_owned(),
                Some(Lucide::Globe),
                Str::PlaceSsh.text(lang).to_owned(),
                false,
                self.asking,
                c,
            )
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.asking = true;
                this.open = true;
                this.go(String::new(), None, cx);
                let handle = this.host_field.read(cx).handle();
                window.focus(&handle, cx);
            })),
        );
        out.push(chips.into_any_element());

        if self.asking {
            let connecting = self.connecting.is_some();
            let host = self.host_field.read(cx).text().to_owned();
            let row = div()
                .flex()
                .gap(px(6.0))
                .child(div().flex_1().min_w_0().child(self.host_field.clone()))
                .child(button(
                    "ssh-connect",
                    if connecting {
                        Str::PlaceConnecting
                    } else {
                        Str::PlaceConnect
                    }
                    .text(lang)
                    .to_owned(),
                    Kind::Primary,
                    host.trim().is_empty() || connecting,
                    c,
                    act(cx, AddRepo::connect_typed),
                ));
            out.push(
                labelled(
                    Str::PlaceHostField.text(lang),
                    div().flex().flex_col().gap(px(5.0)).child(row).child(hint(
                        Str::PlaceSshHint.text(lang).to_owned(),
                        false,
                        c,
                    )),
                    c,
                )
                .into_any_element(),
            );
        }
        if let Some(host) = &self.connecting {
            out.push(
                hint(
                    Str::PlaceConnectingHint.with(lang, &[("host", host)]),
                    false,
                    c,
                )
                .into_any_element(),
            );
        }
        if let Some((host, view)) = &self.reach
            && let Some(text) = reach_text(host, view, lang)
        {
            out.push(error(text, c).into_any_element());
        }
        if self.open {
            out.push(self.dirs(lang, c, cx));
        }
        if let (Some((host, ReachView::Reached { system, .. })), Some(reached)) =
            (&self.reach, &list_host)
            && reached == host
        {
            out.push(
                hint(
                    Str::PlaceSshReached.with(
                        lang,
                        &[("host", host), ("system", &system_text(system, lang))],
                    ),
                    true,
                    c,
                )
                .into_any_element(),
            );
        }
        out
    }

    fn dirs(&self, lang: Lang, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let note = |text: String| {
            div()
                .px(px(8.0))
                .py(px(4.0))
                .text_size(px(12.0))
                .text_color(c.ink_3)
                .child(text)
                .into_any_element()
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        let distro = match self.list.as_ref().map(|l| &l.place) {
            Some(PlaceView::Wsl { distro }) => Some(distro.clone()),
            _ => None,
        };
        if !self.failed.is_empty() {
            rows.push(note(self.failed.clone()));
        } else if let Some(list) = &self.list {
            if let Some(parent) = &list.parent {
                let up = parent.clone();
                let near = distro.clone();
                rows.push(
                    div()
                        .id("dir-up")
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover({
                            let wash = c.accent_wash;
                            move |s| s.bg(wash)
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.go(up.clone(), near.clone(), cx);
                        }))
                        .child(icon(Lucide::ChevronLeft, 14.0, c.ink_3))
                        .child("..")
                        .into_any_element(),
                );
            }
            for (i, folder) in list.folders.iter().enumerate() {
                let path = child_path(&list.path, &folder.name);
                let chosen = self.selected.as_deref() == Some(path.as_str());
                let (pick, open_path, near) = (path.clone(), path, distro.clone());
                let wash = c.accent_wash;
                let row = div()
                    .id(("dir", i))
                    .flex()
                    .items_center()
                    .rounded(px(4.0))
                    .hover(move |s| s.bg(wash));
                let row = if chosen { row.bg(wash) } else { row };
                rows.push(
                    row.child(
                        div()
                            .id(("dir-pick", i))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(8.0))
                            .py(px(4.0))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.selected = Some(pick.clone());
                                this.picked = pick.clone();
                                cx.notify();
                            }))
                            .child(icon(Lucide::Folder, 14.0, c.ink_3))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(folder.name.clone()),
                            )
                            .children(folder.git.then(|| {
                                div()
                                    .ml_auto()
                                    .px(px(5.0))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(c.line_strong)
                                    .text_size(px(11.0))
                                    .text_color(c.ink_3)
                                    .child("git")
                            })),
                    )
                    .child(
                        div()
                            .id(("dir-open", i))
                            .size(px(26.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .hover({
                                let hover = c.surface_hover;
                                move |s| s.bg(hover)
                            })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.go(open_path.clone(), near.clone(), cx);
                            }))
                            .child(icon(Lucide::ChevronRight, 14.0, c.ink_2)),
                    )
                    .into_any_element(),
                );
            }
            if list.folders.is_empty() {
                rows.push(note(Str::RepoNoFolders.text(lang).to_owned()));
            }
        } else {
            let text = if let Some(distro) = &self.waiting {
                Str::PlaceStarting.with(lang, &[("distro", distro)])
            } else if self.dir.trim().is_empty() {
                if self.asking {
                    Str::PlaceSshHome.text(lang).to_owned()
                } else {
                    Str::RepoNoFolders.text(lang).to_owned()
                }
            } else {
                Str::RepoLoading.text(lang).to_owned()
            };
            rows.push(note(text));
        }
        div()
            .id("dirs")
            .mt(px(-6.0))
            .h(px(DIRS))
            .p(px(4.0))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .rounded(px(6.0))
            .border_1()
            .border_color(c.line)
            .bg(c.bg)
            .children(rows)
            .into_any_element()
    }
}

impl Render for AddRepo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.global::<Theme>().colors;
        let lang = *cx.global::<Lang>();
        let tabs = div()
            .flex()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(c.line)
            .bg(c.bg)
            .children(TABS.iter().map(|&tab| {
                let on = tab == self.tab;
                let button = div()
                    .id(("repo-tab", tab as usize))
                    .flex_1()
                    .py(px(4.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .text_size(px(13.0))
                    .child(tab.name(lang));
                if on {
                    button
                        .bg(c.surface)
                        .text_color(c.ink)
                        .font_weight(FontWeight::SEMIBOLD)
                        .shadow(vec![BoxShadow {
                            color: c.shadow.into(),
                            offset: point(px(0.0), px(1.0)),
                            blur_radius: px(2.0),
                            spread_radius: px(0.0),
                            inset: false,
                        }])
                } else {
                    button
                        .text_color(c.ink_2)
                        .cursor_pointer()
                        .hover({
                            let ink = c.ink;
                            move |s| s.text_color(ink)
                        })
                        .when(self.busy, |d| d.opacity(0.5))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            if !this.busy {
                                this.choose(tab, window, cx);
                            }
                        }))
                }
            }));
        let mut panel = div().flex().flex_col().gap(px(12.0));
        match self.tab {
            Tab::Clone => {
                panel = panel.child(labelled(
                    Str::RepoUrl.text(lang),
                    self.url_field.clone(),
                    &c,
                ));
            }
            Tab::Create => {
                panel = panel.child(labelled(
                    Str::RepoName.text(lang),
                    self.name_field.clone(),
                    &c,
                ));
            }
            Tab::Folder => {}
        }
        panel = panel.children(self.picker(lang, &c, cx));
        let target = self.target(cx);
        let footer = if let Some(text) = &self.error {
            Some(error(text.clone(), &c).into_any_element())
        } else if !target.is_empty() {
            let key = if self.tab == Tab::Folder {
                Str::RepoTarget
            } else {
                Str::RepoFolder
            };
            let mut lines = div().flex().flex_col().gap(px(2.0)).child(hint(
                key.with(lang, &[("path", &target)]),
                false,
                &c,
            ));
            if let Some(list) = &self.list
                && (self
                    .locations
                    .as_ref()
                    .is_some_and(|l| !l.distros.is_empty())
                    || !matches!(list.place, PlaceView::Pc))
            {
                lines = lines.child(hint(run_line(&list.place, lang), false, &c));
            }
            Some(lines.into_any_element())
        } else {
            None
        };
        let ready = self.ready(cx);
        let body = shell(&c, WIDTH)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .child(title(Str::TreeAddRepo.text(lang).to_owned(), None, &c))
            .child(tabs)
            .child(panel)
            .children(footer)
            .child(actions(
                button(
                    "repo-cancel",
                    Str::CommonCancel.text(lang).to_owned(),
                    Kind::Ghost,
                    self.busy,
                    &c,
                    act(cx, AddRepo::dismiss),
                ),
                button(
                    "repo-submit",
                    if self.busy {
                        self.tab.busy(lang)
                    } else {
                        self.tab.submit(lang)
                    }
                    .to_owned(),
                    Kind::Primary,
                    self.busy || !ready,
                    &c,
                    act(cx, AddRepo::submit),
                ),
            ));
        pop::overlay(
            self.at,
            body,
            cx.listener(|this, _, _, cx| this.dismiss(cx)),
            window,
        )
    }
}
