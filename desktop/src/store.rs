use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use api::canvas::{CardSize, Placed};
use api::graph::docs::DocRow;
use api::graph::events::EventLog;
use api::graph::waits::{AnswerWait, ChooseOption, ClosedPage, Prompt, TypeText};
use api::graph::{GraphEvent, NewNode, NodeState};
use api::ids::{NodeId, WaitId};
use api::profile::AgentView;
use api::settings::{ArrangeDirection, SettingsView};
use gpui::{
    App, AppContext, Context, EventEmitter, Image, ImageFormat, Task, Window, WindowAppearance,
};

use crate::canvas::drag::Arrange;
use crate::link::{self, Graph, LinkState, Loaded, Snapshot};
use crate::problem::Failure;
use crate::views::dock::answers::Slot;
use crate::views::notice::{self, Notice};
use crate::{strings, theme};

pub const NODE_ENV: &str = "LOOM_DESKTOP_NODE";

pub struct Store {
    base: Option<String>,
    status: String,
    link: LinkState,
    snapshot: Option<Snapshot>,
    revision: u64,
    loaded: u64,
    settings: SettingsView,
    settings_key: String,
    selected: Option<u32>,
    wanted: Option<u32>,
    zoom_percent: u32,
    notice: Option<(u64, Notice)>,
    notices: u64,
    found: Option<HashSet<String>>,
    picked: Option<HashSet<String>>,
    drafts: HashMap<(u32, Slot), String>,
}

pub struct ZoomTo(pub f32);

pub struct DraftChanged(pub u32, pub Slot);

impl EventEmitter<GraphEvent> for Store {}
impl EventEmitter<Arrange> for Store {}
impl EventEmitter<ZoomTo> for Store {}
impl EventEmitter<DraftChanged> for Store {}

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
        let (base, status) = match started {
            Ok(base) => {
                let (graph, inbox) = async_channel::unbounded();
                let status = link::watch(&base, graph)
                    .err()
                    .unwrap_or_else(|| base.clone());
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
                (Some(base), status)
            }
            Err(error) => (None, error),
        };
        Self {
            base,
            status,
            link: LinkState::Reconnecting,
            snapshot: None,
            revision: 0,
            loaded: 0,
            settings,
            settings_key: String::new(),
            selected: None,
            wanted: None,
            zoom_percent: 100,
            notice: None,
            notices: 0,
            found: None,
            picked: None,
            drafts: HashMap::new(),
        }
    }

    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn link(&self) -> LinkState {
        self.link
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

    pub fn keep_drafts(&mut self, live: &HashSet<u32>) {
        self.drafts.retain(|(wait, _), _| live.contains(wait));
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

    pub fn set_found(&mut self, keys: Option<HashSet<String>>, cx: &mut Context<Self>) {
        if self.found != keys {
            self.found = keys;
            cx.notify();
        }
    }

    pub fn set_picked(&mut self, keys: Option<HashSet<String>>, cx: &mut Context<Self>) {
        if self.picked != keys {
            self.picked = keys;
            cx.notify();
        }
    }

    pub fn zoom_to(&mut self, percent: f32, cx: &mut Context<Self>) {
        cx.emit(ZoomTo(percent / 100.0));
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref().map(|(_, notice)| notice)
    }

    pub fn tell(&mut self, text: String, cx: &mut Context<Self>) {
        self.notices += 1;
        let id = self.notices;
        self.notice = Some((id, Notice { text }));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(notice::shown_for()).await;
            let _ = this.update(cx, |store, cx| {
                if store.notice.as_ref().is_some_and(|(shown, _)| *shown == id) {
                    store.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
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
                    store.tell(text, cx)
                }
            });
        })
        .detach();
    }

    pub fn select_when_loaded(&mut self, node: u32, cx: &mut Context<Self>) {
        self.wanted = Some(node);
        self.reload(cx);
    }

    pub fn select(&mut self, node: u32, cx: &mut Context<Self>) {
        if self.selected != Some(node) {
            self.selected = Some(node);
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
            Graph::Link(state) => self.link = state,
            Graph::Failed { order, error } => {
                if !self.newer(order) {
                    return;
                }
                let text = error.text(*cx.global::<strings::Lang>());
                self.status = if text.is_empty() {
                    "connection closed".to_owned()
                } else {
                    text
                }
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
        if self.selected.is_none_or(|id| !present(id)) {
            let wanted = std::env::var(NODE_ENV)
                .ok()
                .and_then(|id| id.parse().ok())
                .filter(|&id| present(id));
            let running = snapshot
                .nodes
                .iter()
                .find(|n| n.state == NodeState::Running)
                .map(|n| n.id.0);
            self.selected = wanted
                .or(running)
                .or(snapshot.nodes.first().map(|n| n.id.0));
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
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let body = AnswerWait {
            text,
            images: Vec::new(),
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
    ) -> Task<Result<(), link::Rejected>> {
        let Some(base) = self.base.clone() else {
            return Task::ready(Err(link::Rejected::Link(self.status.clone())));
        };
        cx.background_spawn(async move { link::adopt(&base, child, parent) })
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
            let log = link::read(base, "/api/events/log")?;
            let agents = link::read(base, "/api/agents").unwrap_or_default();
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
