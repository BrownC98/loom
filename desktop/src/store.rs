use api::graph::docs::DocRow;
use api::graph::waits::{AnswerWait, ChooseOption, ClosedPage, Prompt, TypeText};
use api::graph::{GraphEvent, NodeState};
use api::ids::WaitId;
use api::settings::SettingsView;
use gpui::{App, AppContext, Context, EventEmitter, Task, Window, WindowAppearance};
use serde::de::DeserializeOwned;

use crate::link::{self, Graph, LinkState, Loaded, Snapshot};
use crate::{strings, theme};

pub const NODE_ENV: &str = "LOOM_DESKTOP_NODE";

pub struct Store {
    base: Option<String>,
    status: String,
    link: LinkState,
    snapshot: Option<Snapshot>,
    revision: u64,
    settings: SettingsView,
    settings_key: String,
    selected: Option<u32>,
    zoom_percent: u32,
}

impl EventEmitter<GraphEvent> for Store {}

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
            settings,
            settings_key: String::new(),
            selected: None,
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

    #[allow(dead_code)]
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

    pub fn select(&mut self, node: u32, cx: &mut Context<Self>) {
        if self.selected != Some(node) {
            self.selected = Some(node);
            cx.notify();
        }
    }

    #[allow(dead_code)]
    pub fn reload(&self, cx: &mut Context<Self>) {
        let Some(base) = self.base.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let event = match cx.background_spawn(async move { link::load(&base) }).await {
                Ok(loaded) => Graph::Loaded(Box::new(loaded)),
                Err(error) => Graph::Failed(error),
            };
            let _ = this.update(cx, |store, cx| store.apply(event, cx));
        })
        .detach();
    }

    fn apply(&mut self, event: Graph, cx: &mut Context<Self>) {
        match event {
            Graph::Loaded(loaded) => self.load(*loaded, cx),
            Graph::Feed(event) => cx.emit(*event),
            Graph::Link(state) => self.link = state,
            Graph::Failed(error) => {
                self.status = if error.is_empty() {
                    "connection closed".to_owned()
                } else {
                    error
                }
            }
        }
        cx.notify();
    }

    fn load(&mut self, loaded: Loaded, cx: &mut Context<Self>) {
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

const CALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Copy)]
enum Verb {
    Get,
    Post,
    Delete,
}

fn call<T: DeserializeOwned>(
    base: &str,
    verb: Verb,
    path: &str,
    body: Option<String>,
) -> Result<T, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(CALL_LIMIT))
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("{base}{path}");
    let response = match (verb, body) {
        (Verb::Get, _) => agent.get(&url).call(),
        (Verb::Delete, _) => agent.delete(&url).call(),
        (Verb::Post, Some(body)) => agent
            .post(&url)
            .header("content-type", "application/json")
            .send(body),
        (Verb::Post, None) => agent.post(&url).send_empty(),
    }
    .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response
        .into_body()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    if !status.is_success() {
        let code = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("code")?.as_str().map(str::to_owned))
            .unwrap_or_default();
        return Err(format!("HTTP {} {code}", status.as_u16()));
    }
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

impl Store {
    fn request<T: DeserializeOwned + Send + 'static>(
        &self,
        verb: Verb,
        path: String,
        body: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<T, String>> {
        let Some(base) = self.base.clone() else {
            return Task::ready(Err(self.status.clone()));
        };
        cx.background_spawn(async move { call(&base, verb, &path, body) })
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
        self.request(
            Verb::Post,
            format!("/api/waits/{}/answer", wait.0),
            serde_json::to_string(&body).ok(),
            cx,
        )
    }

    pub fn choose_option(
        &self,
        wait: WaitId,
        prompt: Prompt,
        option: usize,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let body = ChooseOption { prompt, option };
        self.request(
            Verb::Post,
            format!("/api/waits/{}/choose", wait.0),
            serde_json::to_string(&body).ok(),
            cx,
        )
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
        self.request(
            Verb::Post,
            format!("/api/waits/{}/type", wait.0),
            serde_json::to_string(&body).ok(),
            cx,
        )
    }

    pub fn dismiss_wait(&self, wait: WaitId, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        self.request(Verb::Delete, format!("/api/waits/{}", wait.0), None, cx)
    }

    pub fn closed_waits(
        &self,
        before: Option<u32>,
        cx: &mut Context<Self>,
    ) -> Task<Result<ClosedPage, String>> {
        let path = match before {
            Some(seq) => format!("/api/waits/closed?before={seq}"),
            None => "/api/waits/closed".to_owned(),
        };
        self.request(Verb::Get, path, None, cx)
    }

    pub fn list_docs(&self, cx: &mut Context<Self>) -> Task<Result<Vec<DocRow>, String>> {
        self.request(Verb::Get, "/api/docs".to_owned(), None, cx)
    }

    pub fn doc_url(&self, id: &str, cx: &mut Context<Self>) -> Task<Result<String, String>> {
        let Some(base) = self.base.clone() else {
            return Task::ready(Err(self.status.clone()));
        };
        let path = format!("/api/docs/{id}/pass");
        cx.background_spawn(async move {
            let found: String = call(&base, Verb::Post, &path, None)?;
            Ok(if found.starts_with('/') {
                format!("{base}{found}")
            } else {
                found
            })
        })
    }
}
