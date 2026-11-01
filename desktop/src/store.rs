use api::canvas::{CardSize, Placed};
use api::graph::docs::DocRow;
use api::graph::events::EventLog;
use api::graph::waits::{AnswerWait, ChooseOption, ClosedPage, Prompt, TypeText};
use api::graph::{GraphEvent, NewNode, NodeState};
use api::ids::{NodeId, WaitId};
use api::profile::AgentView;
use api::settings::SettingsView;
use gpui::{App, AppContext, Context, EventEmitter, Task, Window, WindowAppearance};

use crate::link::{self, Graph, LinkState, Loaded, Snapshot};
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
}

pub struct ZoomTo(pub f32);

impl EventEmitter<GraphEvent> for Store {}
impl EventEmitter<ZoomTo> for Store {}

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

    pub fn settings(&self) -> &SettingsView {
        &self.settings
    }

    pub fn selected(&self) -> Option<u32> {
        self.selected
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

    pub fn zoom_to(&mut self, percent: f32, cx: &mut Context<Self>) {
        cx.emit(ZoomTo(percent / 100.0));
    }

    pub fn save_settings(&self, patch: serde_json::Value, cx: &mut Context<Self>) {
        let Some(base) = self.base.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_spawn(async move { link::put_settings(&base, &patch) })
                .await;
            if saved.is_ok() {
                let _ = this.update(cx, |store, cx| store.reload(cx));
            }
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
                self.status = if error.is_empty() {
                    "connection closed".to_owned()
                } else {
                    error
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
    fn call<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        work: impl FnOnce(&str) -> Result<T, String> + Send + 'static,
    ) -> Task<Result<T, String>> {
        let Some(base) = self.base.clone() else {
            return Task::ready(Err(self.status.clone()));
        };
        cx.background_spawn(async move { work(&base) })
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
