use api::settings::SettingsView;
use gpui::{
    App, AppContext, BoxShadow, Context, Entity, EventEmitter, FocusHandle, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render,
    ScrollHandle, StatefulInteractiveElement, Styled, Subscription, Window, div, point, px,
    transparent_black,
};
use serde_json::Value;

use super::nav::{self, Tab};
use super::patch::{self, Writer};
use super::tabs;
use super::widgets::confirm;
use super::{Ctx, Section, search};
use crate::link;
use crate::problem::Failure;
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::theme::Colors;
use crate::views::dock::ui::{SMALL, button, colors, say, say_with};
use crate::views::field::{self, Field, FieldEvent};

const WIDTH: f32 = 880.0;
const HEIGHT: f32 = 600.0;
const MARGIN: f32 = 32.0;
const NAV_W: f32 = 180.0;
const SHADOW_Y: f32 = 18.0;
const SHADOW_BLUR: f32 = 48.0;
const TRAP_LIMIT: usize = 256;

pub enum DialogEvent {
    Close,
}

#[derive(Clone, Copy)]
pub(super) enum Ask {
    ResetAll,
    ResetTab(Tab),
    Stop,
}

pub struct Dialog {
    pub(super) store: Entity<Store>,
    pub(super) focus: FocusHandle,
    pub(super) restore: Option<FocusHandle>,
    pub(super) search: Entity<Field>,
    pub(super) tab: Tab,
    pub(super) ask: Option<Ask>,
    view: SettingsView,
    defaults: Option<SettingsView>,
    inflight: usize,
    error: Option<String>,
    states: tabs::State,
    scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for Dialog {}

impl Dialog {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        debug_assert_eq!(
            nav::OWNERS.len(),
            serde_json::to_value(SettingsView::default())
                .ok()
                .and_then(|v| v.as_object().map(|o| o.len()))
                .unwrap_or(nav::OWNERS.len())
        );
        let search = field::new(Str::SettingsSearch, cx);
        let focus = cx.focus_handle();
        let subs = vec![
            cx.observe(&store, |this, store, cx| {
                if this.inflight == 0 {
                    this.view = store.read(cx).settings().clone();
                }
                cx.notify();
            }),
            cx.subscribe_in(
                &search,
                window,
                |this, _, event: &FieldEvent, _, cx| match event {
                    FieldEvent::Changed => {
                        this.scroll.set_offset(point(px(0.0), px(0.0)));
                        cx.notify();
                    }
                    FieldEvent::Escape => this.escape_search(cx),
                    _ => {}
                },
            ),
            cx.on_focus_out(&focus, window, |this, _, window, cx| {
                this.keep_focus(window, cx)
            }),
            cx.on_focus_lost(window, |this, window, cx| this.keep_focus(window, cx)),
        ];
        let view = store.read(cx).settings().clone();
        let restore = window.focused(cx);
        let base = store.read(cx).base().map(str::to_owned);
        let dialog = Self {
            store,
            focus,
            restore,
            search,
            tab: Tab::General,
            ask: None,
            view,
            defaults: None,
            inflight: 0,
            error: None,
            states: tabs::State::default(),
            scroll: ScrollHandle::new(),
            _subs: subs,
        };
        if let Some(base) = base {
            cx.spawn(async move |this, cx| {
                let defaults = cx
                    .background_spawn(async move {
                        link::read::<SettingsView>(&base, "/api/settings/default")
                    })
                    .await;
                let _ = this.update(cx, |dialog, cx| {
                    dialog.defaults = defaults.ok();
                    cx.notify();
                });
            })
            .detach();
        }
        let first = dialog.search.read(cx).handle();
        window.focus(&first, cx);
        dialog
    }

    pub fn restore(&self) -> Option<&FocusHandle> {
        self.restore.as_ref()
    }

    fn keep_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.contains_focused(window, cx) {
            window.focus(&self.focus, cx);
        }
    }

    fn escape_search(&mut self, cx: &mut Context<Self>) {
        if self.query(cx).is_empty() {
            cx.emit(DialogEvent::Close);
        } else {
            self.search.update(cx, |field, cx| field.clear(cx));
        }
    }

    pub(super) fn query(&self, cx: &App) -> String {
        self.search.read(cx).text().trim().to_owned()
    }

    pub(super) fn choose(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.search.update(cx, |field, cx| field.clear(cx));
        self.tab = tab;
        self.scroll.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    pub(super) fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Close);
    }

    pub(super) fn ask(&mut self, ask: Ask, window: &mut Window, cx: &mut Context<Self>) {
        self.ask = Some(ask);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn run(&mut self, ask: Ask, cx: &mut Context<Self>) {
        match ask {
            Ask::ResetAll => self.send(patch::nulls(&nav::resettable()), cx),
            Ask::ResetTab(tab) => self.send(patch::nulls(&tab.keys()), cx),
            Ask::Stop => self.stop(cx),
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(base) = self.store.read(cx).base().map(str::to_owned) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let stopped = cx
                .background_spawn(async move { link::shutdown(&base) })
                .await;
            match stopped {
                Ok(()) => {
                    cx.update(|cx| cx.quit());
                }
                Err(error) => {
                    let _ = this.update(cx, |dialog, cx| {
                        dialog.error = Some(error.text(*cx.global::<Lang>()));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub fn edit(&mut self, change: impl FnOnce(&mut SettingsView), cx: &mut Context<Self>) {
        self.edit_then(change, |_, _| {}, cx);
    }

    pub fn edit_then(
        &mut self,
        change: impl FnOnce(&mut SettingsView),
        done: impl FnOnce(Result<(), &Failure>, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let mut next = self.view.clone();
        change(&mut next);
        match patch::settings_patch(&self.view, &next) {
            Some(patch) => {
                self.view = next;
                self.send_then(patch, done, cx);
            }
            None => done(Ok(()), cx),
        }
    }

    pub fn send(&mut self, patch: Value, cx: &mut Context<Self>) {
        self.send_then(patch, |_, _| {}, cx);
    }

    pub fn send_then(
        &mut self,
        patch: Value,
        done: impl FnOnce(Result<(), &Failure>, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(base) = self.store.read(cx).base().map(str::to_owned) else {
            done(Err(&Failure::Other(String::new())), cx);
            return;
        };
        self.inflight += 1;
        self.error = None;
        cx.notify();
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_spawn(async move {
                    link::put_settings(&base, &patch)?;
                    link::read::<SettingsView>(&base, "/api/settings")
                })
                .await;
            let _ = this.update(cx, |dialog, cx| {
                dialog.inflight -= 1;
                if let Err(error) = &saved {
                    dialog.error = Some(error.text(*cx.global::<Lang>()));
                }
                match &saved {
                    Ok(fresh) if dialog.inflight == 0 => dialog.view = fresh.clone(),
                    Err(_) if dialog.inflight == 0 => {
                        dialog.view = dialog.store.read(cx).settings().clone();
                    }
                    _ => {}
                }
                cx.notify();
                done(saved.as_ref().map(|_| ()), cx);
            });
            store.update(cx, |store, cx| store.reload(cx));
        })
        .detach();
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let stroke = &event.keystroke;
        match stroke.key.as_str() {
            "escape" => {
                if self.ask.is_some() {
                    self.ask = None;
                    cx.notify();
                } else {
                    self.close(cx);
                }
            }
            "enter" if self.ask.is_some() => self.accept(cx),
            "tab" => self.trap(stroke.modifiers.shift, window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn trap(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.ask.is_some() {
            return;
        }
        for _ in 0..TRAP_LIMIT {
            if back {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            if self.focus.contains_focused(window, cx) {
                break;
            }
        }
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        if let Some(ask) = self.ask.take() {
            self.run(ask, cx);
            cx.notify();
        }
    }

    fn ask_texts(&self, ask: Ask, cx: &App) -> (String, &'static str) {
        match ask {
            Ask::ResetAll => (
                say(cx, Str::SettingsConfirmResetAll).to_owned(),
                say(cx, Str::SettingsResetAll),
            ),
            Ask::ResetTab(tab) => (
                say_with(
                    cx,
                    Str::SettingsConfirmResetTab,
                    &[("tab", say(cx, tab.label()))],
                ),
                say(cx, Str::SettingsResetTab),
            ),
            Ask::Stop => (
                say(cx, Str::SettingsConfirmStop).to_owned(),
                say(cx, Str::SettingsStop),
            ),
        }
    }
}

fn heading(label: String, tag: Option<&'static str>, query: &str, c: &Colors) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(px(SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c.ink_3)
        .children(tag.map(|tag| {
            div()
                .px(px(6.0))
                .py(px(1.0))
                .rounded(px(4.0))
                .bg(c.line)
                .text_color(c.ink_2)
                .text_size(px(11.0))
                .child(tag)
        }))
        .child(search::mark(&label, query, c))
}

pub(super) fn ghost(id: &'static str, cx: &App) -> gpui::Stateful<gpui::Div> {
    let c = colors(cx);
    button(id, cx).border_color(transparent_black()).bg(c.panel)
}

impl Render for Dialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors(cx);
        let query = self.query(cx);
        let searching = !query.is_empty();
        let writer = Writer::new(cx.weak_entity());
        let ctx = Ctx {
            view: &self.view,
            defaults: self.defaults.as_ref(),
            writer,
            query: &query,
            store: &self.store,
            dialog: cx.weak_entity(),
        };
        let shown: Vec<(Tab, Section)> = if searching {
            Tab::ALL
                .iter()
                .flat_map(|&tab| {
                    tabs::sections(tab, &mut self.states, &ctx, window, cx)
                        .into_iter()
                        .map(move |section| (tab, section))
                })
                .filter(|(_, section)| search::found(&section.haystack, &query))
                .collect()
        } else {
            tabs::sections(self.tab, &mut self.states, &ctx, window, cx)
                .into_iter()
                .map(|section| (self.tab, section))
                .collect()
        };
        let empty = searching && shown.is_empty();
        let lang_tag = |tab: Tab| say(cx, tab.label());
        let sections: Vec<_> =
            shown
                .into_iter()
                .map(|(tab, section)| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .children(section.head.map(|head| {
                            heading(head, searching.then(|| lang_tag(tab)), &query, &c)
                        }))
                        .children(section.body)
                })
                .collect();

        let title = if searching {
            say_with(cx, Str::SettingsSearchResults, &[("query", &query)])
        } else {
            say(cx, self.tab.label()).to_owned()
        };
        let can_reset = !searching && !self.tab.keys().is_empty();
        let head = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(22.0))
            .pt(px(18.0))
            .pb(px(8.0))
            .child(
                div()
                    .flex_1()
                    .text_size(px(15.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .children(can_reset.then(|| {
                let tab = self.tab;
                button("settings-reset-tab", cx)
                    .child(say(cx, Str::SettingsResetTab))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.ask(Ask::ResetTab(tab), window, cx)
                    }))
            }))
            .child(
                ghost("settings-close", cx)
                    .child("\u{00D7}")
                    .text_size(px(20.0))
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            );
        let body = div()
            .id("settings-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(22.0))
            .pb(px(18.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .children(
                self.error
                    .clone()
                    .map(|error| div().text_size(px(SMALL)).text_color(c.err).child(error)),
            )
            .children(sections)
            .children(empty.then(|| {
                div()
                    .text_size(px(SMALL))
                    .text_color(c.ink_3)
                    .child(say(cx, Str::SettingsSearchNone))
            }));
        let pane = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(head)
            .child(body);

        let view = window.viewport_size();
        let width = WIDTH.min(f32::from(view.width) - MARGIN);
        let height = HEIGHT.min(f32::from(view.height) - MARGIN);
        let panel = div()
            .w(px(width))
            .h(px(height))
            .flex()
            .overflow_hidden()
            .rounded(px(12.0))
            .bg(c.panel)
            .shadow(vec![BoxShadow {
                color: c.shadow.into(),
                offset: point(px(0.0), px(SHADOW_Y)),
                blur_radius: px(SHADOW_BLUR),
                spread_radius: px(0.0),
                inset: false,
            }])
            .child(nav::column(self, NAV_W, window, cx))
            .child(pane);
        let confirmation = self.ask.map(|ask| {
            let (message, accept) = self.ask_texts(ask, cx);
            confirm(
                message,
                accept,
                cx.listener(|this, _, _, cx| this.accept(cx)),
                cx.listener(|this, _, _, cx| {
                    this.ask = None;
                    cx.notify();
                }),
                cx,
            )
        });
        div()
            .key_context("SettingsDialog")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.keep_focus(window, cx)),
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(c.scrim)
            .text_color(c.ink)
            .child(panel)
            .children(confirmation)
    }
}
