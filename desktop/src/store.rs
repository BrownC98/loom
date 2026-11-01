use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use api::canvas::{CardSize, Placed};
use api::graph::docs::DocRow;
use api::graph::events::EventLog;
use api::graph::waits::{AnswerWait, ChooseOption, ClosedPage, Prompt, TypeText};
use api::graph::{GraphEvent, NewNode, NodeView};
use api::ids::{NodeId, RepoId, WaitId};
use api::profile::AgentView;
use api::repos::PlaceView;
use api::settings::places::ReachView;
use api::settings::{ArrangeDirection, GraphScope, SettingsView};
use gpui::{
    App, AppContext, Bounds, Context, EventEmitter, Image, ImageFormat, Pixels, Task, Window,
    WindowAppearance,
};

use crate::canvas::drag::Arrange;
use crate::canvas::repos::{chosen_repo, groups_of};
use crate::link::{self, Graph, Loaded, Snapshot};
use crate::problem::Failure;
use crate::{strings, theme};

const ROOM_GAP: f32 = 14.0;
const REACH_EVERY: Duration = Duration::from_secs(5);
pub const NODE_ENV: &str = "LOOM_DESKTOP_NODE";

pub struct Store {
    base: Option<String>,
    status: String,
    snapshot: Option<Snapshot>,
    revision: u64,
    loaded: u64,
    settings: SettingsView,
    settings_key: String,
    selected: Option<u32>,
    chosen: Option<String>,
    wanted: Option<u32>,
    zoom_percent: u32,
    notice: Option<Notice>,
    found: Option<HashSet<String>>,
    current: Option<String>,
    picked: Option<HashSet<String>>,
    composer: Option<(Bounds<Pixels>, bool)>,
    zoomed: bool,
    drafts: HashMap<(u32, Slot), String>,
    busy: HashSet<u32>,
    unreached: HashSet<String>,
}

#[derive(Clone)]
pub struct Notice {
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Answer,
    Option(usize),
    Field,
}

pub struct ZoomTo(pub f32);

pub struct NodeMenu {
    pub node: u32,
    pub at: gpui::Point<Pixels>,
    pub button: Option<Bounds<Pixels>>,
}

#[derive(Clone)]
pub enum Target {
    Nodes(Vec<u32>),
    Boxes(Vec<String>),
    All,
}

pub struct Show(pub Target);
pub struct DraftChanged(pub u32, pub Slot);

pub struct BusyChanged;

impl EventEmitter<GraphEvent> for Store {}
impl EventEmitter<Arrange> for Store {}
impl EventEmitter<ZoomTo> for Store {}
impl EventEmitter<NodeMenu> for Store {}
impl EventEmitter<Show> for Store {}
impl EventEmitter<DraftChanged> for Store {}
impl EventEmitter<BusyChanged> for Store {}

fn unreached_of(base: &str, hosts: Vec<String>) -> HashSet<String> {
    hosts
        .into_iter()
        .filter(|host| {
            matches!(
                link::read::<Option<ReachView>>(base, &format!("/api/ssh/{}", link::escape(host))),
                Ok(Some(reach)) if !matches!(reach, ReachView::Reached { .. })
            )
        })
        .collect()
}

fn apply_settings(settings: &SettingsView, cx: &mut App) {
    let dark = matches!(
        cx.window_appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    cx.set_global(theme::Theme::resolve(settings, dark));
    cx.set_global(strings::Lang::resolve(
        settings.language,
        &sys_locale::get_locale().unwrap_or_default(),
    ));
}

impl Store {
    pub fn new(
        started: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = SettingsView::default();
        apply_settings(&settings, cx);
        let (base, status, failure) = match started {
            Ok(base) => {
                let (graph, inbox) = async_channel::unbounded();
                let failure = link::watch(&base, graph).err();
                let status = failure.clone().unwrap_or_else(|| base.clone());
                cx.spawn_in(window, async move |this, cx| {
                    while let Ok(event) = inbox.recv().await {
                        if this
                            .update_in(cx, |store, _, cx| store.apply(event, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
                let reach_base = base.clone();
                cx.spawn_in(window, async move |this, cx| {
                    loop {
                        cx.background_executor().timer(REACH_EVERY).await;
                        let Ok(hosts) = this.read_with(cx, |store, _| store.ssh_hosts()) else {
                            break;
                        };
                        if hosts.is_empty() {
                            continue;
                        }
                        let base = reach_base.clone();
                        let down = cx
                            .background_spawn(async move { unreached_of(&base, hosts) })
                            .await;
                        let kept = this.update(cx, |store, cx| {
                            if store.unreached != down {
                                store.unreached = down;
                                cx.notify();
                            }
                        });
                        if kept.is_err() {
                            break;
                        }
                    }
                })
                .detach();
                (Some(base), status, failure)
            }
            Err(error) => (None, error.clone(), Some(error)),
        };
        let notice = failure.map(|text| Notice { text });
        Self {
            base,
            status,
            snapshot: None,
            revision: 0,
            loaded: 0,
            settings,
            settings_key: String::new(),
            selected: None,
            chosen: None,
            wanted: None,
            zoom_percent: 100,
            notice,
            found: None,
            current: None,
            picked: None,
            composer: None,
            zoomed: false,
            drafts: HashMap::new(),
            busy: HashSet::new(),
            unreached: HashSet::new(),
        }
    }

    fn ssh_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self
            .snapshot
            .iter()
            .flat_map(|snapshot| &snapshot.repos)
            .filter_map(|repo| match &repo.place {
                PlaceView::Ssh { host } => Some(host.to_lowercase()),
                _ => None,
            })
            .collect();
        hosts.sort();
        hosts.dedup();
        hosts
    }

    pub fn is_unreached(&self, node: &NodeView) -> bool {
        self.snapshot
            .iter()
            .flat_map(|snapshot| &snapshot.repos)
            .find(|repo| repo.id == node.place.repo)
            .is_some_and(|repo| match &repo.place {
                PlaceView::Ssh { host } => self.unreached.contains(&host.to_lowercase()),
                _ => false,
            })
    }

    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn loaded(&self) -> u64 {
        self.loaded
    }

    pub fn settings(&self) -> &SettingsView {
        &self.settings
    }

    pub fn selected(&self) -> Option<u32> {
        self.selected
    }

    pub fn draft(&self, wait: u32, slot: Slot) -> &str {
        self.drafts.get(&(wait, slot)).map_or("", String::as_str)
    }

    pub fn set_draft(&mut self, wait: u32, slot: Slot, text: &str, cx: &mut Context<Self>) {
        if self.draft(wait, slot) == text {
            return;
        }
        if text.is_empty() {
            self.drafts.remove(&(wait, slot));
        } else {
            self.drafts.insert((wait, slot), text.to_owned());
        }
        cx.emit(DraftChanged(wait, slot));
    }

    pub fn keep_waits(&mut self, live: &HashSet<u32>, cx: &mut Context<Self>) {
        self.drafts.retain(|(wait, _), _| live.contains(wait));
        let before = self.busy.len();
        self.busy.retain(|wait| live.contains(wait));
        if self.busy.len() != before {
            cx.emit(BusyChanged);
        }
    }

    pub fn is_busy(&self, wait: u32) -> bool {
        self.busy.contains(&wait)
    }

    pub fn set_busy(&mut self, wait: u32, busy: bool, cx: &mut Context<Self>) {
        let changed = if busy {
            self.busy.insert(wait)
        } else {
            self.busy.remove(&wait)
        };
        if changed {
            cx.emit(BusyChanged);
        }
    }

    pub fn zoom_percent(&self) -> u32 {
        self.zoom_percent
    }

    pub fn set_zoom_percent(&mut self, percent: u32, cx: &mut Context<Self>) {
        if self.zoom_percent != percent {
            self.zoom_percent = percent;
            cx.notify();
        }
    }

    pub fn lit(&self) -> Option<&HashSet<String>> {
        self.found.as_ref().or(self.picked.as_ref())
    }

    pub fn current(&self) -> Option<&str> {
        self.found.as_ref().and(self.current.as_deref())
    }

    pub fn set_found(
        &mut self,
        keys: Option<HashSet<String>>,
        current: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.found != keys || self.current != current {
            self.found = keys;
            self.current = current;
            cx.notify();
        }
    }

    pub fn has_picked(&self) -> bool {
        self.picked.is_some()
    }

    pub fn set_picked(&mut self, keys: Option<HashSet<String>>, cx: &mut Context<Self>) {
        if self.picked != keys {
            self.picked = keys;
            cx.notify();
        }
    }

    pub fn show_node(&mut self, node: u32, cx: &mut Context<Self>) {
        self.show_nodes(vec![node], cx);
    }

    pub fn open_node_menu(
        &mut self,
        node: u32,
        at: gpui::Point<Pixels>,
        button: Option<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        cx.emit(NodeMenu { node, at, button });
    }

    pub fn go(&mut self, node: u32, cx: &mut Context<Self>) {
        self.select(node, cx);
        self.show_node(node, cx);
    }

    pub fn show_nodes(&mut self, nodes: Vec<u32>, cx: &mut Context<Self>) {
        let present: Vec<(u32, RepoId)> = nodes
            .into_iter()
            .filter_map(|id| Some((id, self.repo_of(id)?)))
            .collect();
        let shown = self.shown_repos();
        let mut ids: Vec<u32> = present
            .iter()
            .filter(|(_, repo)| shown.contains(repo))
            .map(|(id, _)| *id)
            .collect();
        if ids.is_empty()
            && let Some(&(_, repo)) = present.first()
        {
            self.choose_repo(repo, cx);
            ids = present
                .iter()
                .filter(|(_, r)| *r == repo)
                .map(|(id, _)| *id)
                .collect();
        }
        if !ids.is_empty() {
            cx.emit(Show(Target::Nodes(ids)));
        }
    }

    pub fn show_boxes(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        cx.emit(Show(Target::Boxes(paths)));
    }

    pub fn show_repo(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        self.choose_repo(repo, cx);
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(info) = snapshot.repos.iter().find(|r| r.id == repo) else {
            return;
        };
        let views: Vec<&NodeView> = snapshot
            .nodes
            .iter()
            .filter(|n| n.place.repo == repo)
            .collect();
        let paths = groups_of(info, &views)
            .into_iter()
            .map(|g| g.path)
            .collect();
        cx.emit(Show(Target::Boxes(paths)));
    }

    pub fn show_all(&mut self, cx: &mut Context<Self>) {
        cx.emit(Show(Target::All));
    }

    fn current_repo(&self) -> Option<RepoId> {
        let snapshot = self.snapshot.as_ref()?;
        chosen_repo(&snapshot.repos, self.settings.repo).map(|r| r.id)
    }

    pub fn shown_repos(&self) -> Vec<RepoId> {
        match (self.settings.graph_scope, &self.snapshot) {
            (GraphScope::All, Some(snapshot)) => snapshot.repos.iter().map(|r| r.id).collect(),
            _ => self.current_repo().into_iter().collect(),
        }
    }

    pub fn choose_repo(&mut self, repo: RepoId, cx: &mut Context<Self>) {
        if self.current_repo() == Some(repo) {
            return;
        }
        self.settings.repo = Some(repo);
        self.settings_key.clear();
        self.save_settings(serde_json::json!({ "repo": repo }), cx);
        cx.notify();
    }

    fn repo_of(&self, node: u32) -> Option<RepoId> {
        let snapshot = self.snapshot.as_ref()?;
        snapshot
            .nodes
            .iter()
            .find(|n| n.id.0 == node)
            .map(|n| n.place.repo)
    }

    pub fn zoomed(&self) -> bool {
        self.zoomed
    }

    pub fn set_zoomed(&mut self, zoomed: bool, cx: &mut Context<Self>) {
        if self.zoomed != zoomed {
            self.zoomed = zoomed;
            cx.notify();
        }
    }

    pub fn zoom_to(&mut self, percent: f32, cx: &mut Context<Self>) {
        cx.emit(ZoomTo(percent / 100.0));
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn tell(&mut self, text: String, cx: &mut Context<Self>) {
        self.notice = Some(Notice { text });
        cx.notify();
    }

    pub fn clear_notice(&mut self, cx: &mut Context<Self>) {
        if self.base.is_some() && self.notice.take().is_some() {
            cx.notify();
        }
    }

    pub fn save_settings(&self, patch: serde_json::Value, cx: &mut Context<Self>) {
        let Some(base) = self.base.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_spawn(async move { link::put_settings(&base, &patch) })
                .await;
            let _ = this.update(cx, |store, cx| match saved {
                Ok(()) => store.reload(cx),
                Err(error) => {
                    let text = error.text(*cx.global::<strings::Lang>());
                    store.tell(text, cx);
                    store.reload(cx)
                }
            });
        })
        .detach();
    }

    pub fn set_pane_open(&self, pane: &str, open: bool, cx: &mut Context<Self>) {
        self.save_settings(serde_json::json!({ "closedPanes": { pane: !open } }), cx);
    }

    pub fn select_when_loaded(&mut self, node: u32, cx: &mut Context<Self>) {
        self.wanted = Some(node);
        self.reload(cx);
    }

    pub fn select(&mut self, node: u32, cx: &mut Context<Self>) {
        if let Some(repo) = self.repo_of(node) {
            self.choose_repo(repo, cx);
        }
        if (self.selected != Some(node)) | self.chosen.take().is_some() {
            self.selected = Some(node);
            cx.notify();
        }
    }

    pub fn chosen(&self) -> Option<&str> {
        self.chosen.as_deref()
    }

    pub fn choose_box(&mut self, key: String, cx: &mut Context<Self>) {
        self.selected = None;
        self.chosen = Some(key);
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if self.selected.take().is_some() | self.chosen.take().is_some() {
            cx.notify();
        }
    }

    pub fn reload(&self, cx: &mut Context<Self>) {
        let Some(base) = self.base.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let event = cx.background_spawn(async move { link::load(&base) }).await;
            let _ = this.update(cx, |store, cx| store.apply(event, cx));
        })
        .detach();
    }

    fn apply(&mut self, event: Graph, cx: &mut Context<Self>) {
        match event {
            Graph::Loaded(loaded) => self.load(*loaded, cx),
            Graph::Feed(event) => cx.emit(*event),
            Graph::Link => {}
            Graph::Failed { order, error } => {
                if !self.newer(order) {
                    return;
                }
                let text = error.text(*cx.global::<strings::Lang>());
                let text = if text.is_empty() {
                    "connection closed".to_owned()
                } else {
                    text
                };
                self.notice = Some(Notice { text });
            }
        }
        cx.notify();
    }

    fn newer(&mut self, order: u64) -> bool {
        let newer = order > self.loaded;
        if newer {
            self.loaded = order;
        }
        newer
    }

    fn load(&mut self, loaded: Loaded, cx: &mut Context<Self>) {
        if !self.newer(loaded.order) {
            return;
        }
        self.status = self.base.clone().unwrap_or_default();
        let key = serde_json::to_string(&loaded.settings).unwrap_or_default();
        if key != self.settings_key {
            apply_settings(&loaded.settings, cx);
            if loaded.settings.closed_panes.dock {
                self.picked = None;
            }
            self.settings = loaded.settings;
            self.settings_key = key;
        }
        self.snapshot = Some(loaded.snapshot);
        self.revision += 1;
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let present = |id: u32| snapshot.nodes.iter().any(|n| n.id.0 == id);
        if let Some(id) = self.wanted.filter(|&id| present(id)) {
            self.selected = Some(id);
            self.wanted = None;
        }
        if self.selected.is_some_and(|id| !present(id)) {
            self.selected = None;
        }
        if self.revision == 1
            && self.selected.is_none()
            && let Some(id) = std::env::var(NODE_ENV)
                .ok()
                .and_then(|id| id.parse().ok())
                .filter(|&id| present(id))
        {
            self.selected = Some(id);
        }
    }
}

impl Store {
    pub fn arrange(&mut self, direction: ArrangeDirection, cx: &mut Context<Self>) {
        self.save_settings(serde_json::json!({ "arrangeDirection": direction }), cx);
        cx.emit(Arrange { direction });
    }

    pub fn keep_arranged(&mut self, on: bool, cx: &mut Context<Self>) {
        self.save_settings(serde_json::json!({ "alwaysArrange": on }), cx);
        if on {
            cx.emit(Arrange {
                direction: self.settings.arrange_direction,
            });
        }
    }

    fn call<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        work: impl FnOnce(&str) -> Result<T, Failure> + Send + 'static,
    ) -> Task<Result<T, String>> {
        let Some(base) = self.base.clone() else {
            return Task::ready(Err(self.status.clone()));
        };
        cx.spawn(async move |_, cx| {
            let done = cx.background_spawn(async move { work(&base) }).await;
            done.map_err(|failure| failure.text(cx.update(|app| *app.global::<strings::Lang>())))
        })
    }

    pub fn agents(&self, cx: &mut Context<Self>) -> Task<Result<Vec<AgentView>, String>> {
        self.call(cx, |base| link::read(base, "/api/agents"))
    }

    pub fn create_node(
        &self,
        body: NewNode,
        cx: &mut Context<Self>,
    ) -> Task<Result<NodeId, String>> {
        self.call(cx, move |base| {
            link::post_within(
                base,
                "/api/nodes",
                serde_json::to_string(&body).ok(),
                link::SLOW_LIMIT,
            )
        })
    }

    pub fn answer_wait(
        &self,
        wait: WaitId,
        text: String,
        images: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let body = AnswerWait {
            text,
            images: images.into_iter().map(Into::into).collect(),
        };
        self.call(cx, move |base| {
            let path = format!("/api/waits/{}/answer", wait.0);
            link::post(base, &path, serde_json::to_string(&body).ok())
        })
    }

    pub fn choose_option(
        &self,
        wait: WaitId,
        prompt: Prompt,
        option: usize,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let body = ChooseOption { prompt, option };
        self.call(cx, move |base| {
            let path = format!("/api/waits/{}/choose", wait.0);
            link::post(base, &path, serde_json::to_string(&body).ok())
        })
    }

    pub fn type_text(
        &self,
        wait: WaitId,
        prompt: Prompt,
        text: String,
        option: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let body = TypeText {
            prompt,
            text,
            option,
        };
        self.call(cx, move |base| {
            let path = format!("/api/waits/{}/type", wait.0);
            link::post(base, &path, serde_json::to_string(&body).ok())
        })
    }

    pub fn adopt(
        &self,
        child: u32,
        parent: u32,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        self.call(cx, move |base| link::adopt(base, child, parent))
    }

    pub fn wait_image(
        &self,
        wait: WaitId,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Task<Result<Arc<Image>, String>> {
        self.call(cx, move |base| {
            let (mime, bytes) =
                link::image(base, &format!("/api/waits/{}/images/{index}", wait.0))?;
            let format = ImageFormat::from_mime_type(&mime)
                .ok_or_else(|| Failure::Other(format!("image type {mime}")))?;
            Ok(Arc::new(Image::from_bytes(format, bytes)))
        })
    }

    pub fn dismiss_wait(&self, wait: WaitId, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        self.call(cx, move |base| {
            link::delete(base, &format!("/api/waits/{}", wait.0))
        })
    }

    pub fn closed_waits(
        &self,
        before: Option<u32>,
        cx: &mut Context<Self>,
    ) -> Task<Result<ClosedPage, String>> {
        self.call(cx, move |base| {
            let path = match before {
                Some(seq) => format!("/api/waits/closed?before={seq}"),
                None => "/api/waits/closed".to_owned(),
            };
            link::read(base, &path)
        })
    }

    pub fn event_console(
        &self,
        cx: &mut Context<Self>,
    ) -> Task<Result<(EventLog, Vec<AgentView>), String>> {
        self.call(cx, |base| {
            let agents = link::read(base, "/api/agents").unwrap_or_default();
            let log = link::read(base, "/api/events/log")?;
            Ok((log, agents))
        })
    }

    pub fn list_docs(&self, cx: &mut Context<Self>) -> Task<Result<Vec<DocRow>, String>> {
        self.call(cx, |base| link::read(base, "/api/docs"))
    }

    pub fn doc_url(&self, id: &str, cx: &mut Context<Self>) -> Task<Result<String, String>> {
        let path = format!("/api/docs/{id}/pass");
        self.call(cx, move |base| {
            let found: String = link::post(base, &path, None)?;
            Ok(if found.starts_with('/') {
                format!("{base}{found}")
            } else {
                found
            })
        })
    }
}

impl Store {
    pub fn save_size(
        &self,
        size: CardSize,
        spot: Option<Placed>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        self.call(cx, move |base| {
            link::put_sizes(base, &[size])?;
            match spot {
                Some(spot) => link::put_spots(base, &[spot]),
                None => Ok(()),
            }
        })
    }

    pub fn clear_sizes(
        &self,
        nodes: Vec<NodeId>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        self.call(cx, move |base| link::clear_sizes(base, &nodes))
    }
}

impl Store {
    pub fn composer_box(&self) -> Option<Bounds<Pixels>> {
        self.composer.map(|(bounds, _)| bounds)
    }

    pub fn composer_room(&self) -> f32 {
        self.composer
            .filter(|(_, home)| *home)
            .map_or(0.0, |(bounds, _)| f32::from(bounds.size.height) + ROOM_GAP)
    }

    pub fn set_composer(
        &mut self,
        composer: Option<(Bounds<Pixels>, bool)>,
        cx: &mut Context<Self>,
    ) {
        if self.composer != composer {
            self.composer = composer;
            cx.notify();
        }
    }

    pub fn save_paste(
        &self,
        node: NodeId,
        name: String,
        mime: &'static str,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) -> Task<Result<String, String>> {
        self.call(cx, move |base| {
            link::save_paste(base, node, &name, mime, &bytes)
        })
    }
}
