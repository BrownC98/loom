use api::graph::{GraphEvent, NodeState};
use api::settings::SettingsView;
use gpui::{App, AppContext, Context, EventEmitter, Window, WindowAppearance};

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
            loaded: 0,
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
        if loaded.order <= self.loaded {
            return;
        }
        self.loaded = loaded.order;
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
