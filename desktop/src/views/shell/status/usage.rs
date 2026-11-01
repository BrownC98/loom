use std::cell::Cell;
use std::rc::Rc;

use api::graph::GraphEvent;
use api::graph::events::{Entry, EventLog, EventRecord, SwitchCause};
use api::ids::AccountId;
use api::profile::Provider;
use api::repos::PlaceView;
use api::settings::accounts::{AccountView, PlaceUsage};
use api::settings::usage::Limits;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Div, Entity, FocusHandle, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Window, canvas, div, px,
};

use crate::fonts;
use crate::link::{self, VERSION};
use crate::settings::{keep_focus, modal, settings_patch};
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::theme::{Colors, Theme};
use crate::views::dock::ui::say;
use crate::views::icons::{Lucide, agent_mark, icon};
use crate::views::tooltip::tip;
use crate::views::usage::{
    self, ACCOUNTS, AccountsChanged, PROVIDERS, REFRESH, Switching, account_name, active_index,
    provider_label,
};

const CURSOR: &str = "/api/usage/cursor";
const EVENTS: &str = "/api/events/log";
const NAME_W: f32 = 220.0;
const NOTICE_W: f32 = 520.0;
const METER_W: f32 = 36.0;
const MARK: f32 = 14.0;

mod popover;

pub struct Usage {
    store: Entity<Store>,
    accounts: Option<Vec<AccountView>>,
    cursor: Option<Vec<PlaceUsage>>,
    loading: bool,
    ticket: u64,
    open: bool,
    expanded: Option<Provider>,
    switching: Option<Switching>,
    latest: Option<EventRecord>,
    focus: FocusHandle,
    dialog_focus: FocusHandle,
    restore: Option<FocusHandle>,
    anchor: Rc<Cell<Bounds<Pixels>>>,
}

impl Usage {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        cx.observe_global::<AccountsChanged>(|this, cx| this.refresh(cx))
            .detach();
        cx.subscribe(&store, |this, _, event: &GraphEvent, cx| {
            this.feed(event, cx)
        })
        .detach();
        let usage = Self {
            store,
            accounts: None,
            cursor: None,
            loading: false,
            ticket: 0,
            open: false,
            expanded: None,
            switching: None,
            latest: None,
            focus: cx.focus_handle(),
            dialog_focus: cx.focus_handle(),
            restore: None,
            anchor: Rc::default(),
        };
        cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH).await;
            }
        })
        .detach();
        usage.load_latest(cx);
        usage
    }

    fn base(&self, cx: &App) -> Option<String> {
        self.store.read(cx).base().map(str::to_owned)
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(base) = self.base(cx) else {
            return;
        };
        self.ticket += 1;
        let ticket = self.ticket;
        self.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_spawn(async move {
                    let accounts = link::read::<Vec<AccountView>>(&base, ACCOUNTS)?;
                    let cursor = link::read::<Vec<PlaceUsage>>(&base, CURSOR).unwrap_or_default();
                    Ok::<_, crate::problem::Failure>((accounts, cursor))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.ticket != ticket {
                    return;
                }
                this.loading = false;
                if let Ok((accounts, cursor)) = read {
                    this.accounts = Some(accounts);
                    this.cursor = Some(cursor);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_latest(&self, cx: &mut Context<Self>) {
        let Some(base) = self.base(cx) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let log = cx
                .background_spawn(async move { link::read::<EventLog>(&base, EVENTS) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(log) = log
                    && let Some(record) = log.records.into_iter().find(|r| is_switch(&r.entry))
                {
                    this.latest.get_or_insert(record);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn feed(&mut self, event: &GraphEvent, cx: &mut Context<Self>) {
        let GraphEvent::Recorded { record } = event else {
            return;
        };
        if !is_switch(&record.entry) {
            return;
        }
        self.latest = Some((**record).clone());
        if matches!(&record.entry, Entry::AccountSwitched { why, .. } if *why != SwitchCause::Manual)
        {
            self.refresh(cx);
        }
        cx.notify();
    }

    fn close_notice(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.latest.as_ref().map(|r| r.id) else {
            return;
        };
        let patch = serde_json::json!({ "closedSwitchNotice": id });
        self.store
            .update(cx, |store, cx| store.save_settings(patch, cx));
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(window, cx);
            return;
        }
        self.refresh(cx);
        self.restore = window.focused(cx);
        window.focus(&self.focus, cx);
        self.open = true;
        cx.notify();
    }

    fn give_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.restore.take() {
            window.focus(&handle, cx);
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        if self.switching.is_none() {
            self.give_back(window, cx);
        }
        cx.notify();
    }

    fn end_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.switching = None;
        self.give_back(window, cx);
        cx.notify();
    }

    fn distro(&self, cx: &App) -> Option<String> {
        let store = self.store.read(cx);
        let snapshot = store.snapshot()?;
        let by_node = store
            .selected()
            .and_then(|id| snapshot.nodes.iter().find(|n| n.id.0 == id))
            .map(|node| node.place.repo);
        let saved = store.settings().repo;
        let repo = by_node
            .and_then(|id| snapshot.repos.iter().find(|r| r.id == id))
            .or_else(|| snapshot.repos.iter().find(|r| Some(r.id) == saved))
            .or_else(|| snapshot.repos.first())?;
        let PlaceView::Wsl { distro } = &repo.place else {
            return None;
        };
        let known = self
            .accounts
            .as_ref()
            .is_some_and(|all| all.iter().any(|a| a.distro.as_ref() == Some(distro)));
        known.then(|| distro.clone())
    }

    fn own<'a>(&'a self, provider: Provider, distro: Option<&str>) -> Vec<&'a AccountView> {
        self.accounts
            .iter()
            .flatten()
            .filter(|a| a.provider == provider && a.distro.as_deref() == distro)
            .collect()
    }

    fn active<'a>(
        &'a self,
        provider: Provider,
        distro: Option<&str>,
        cx: &App,
    ) -> Option<&'a AccountView> {
        let own = self.own(provider, distro);
        let chosen = self.store.read(cx).settings().account(provider, distro);
        active_index(&own, chosen).map(|index| own[index])
    }

    fn choose(
        &mut self,
        provider: Provider,
        distro: Option<String>,
        to: Option<AccountId>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(base) = self.base(cx) else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let (found, id) = (distro.clone(), to.clone());
            let nodes = cx
                .background_spawn(async move {
                    usage::movable_nodes(&base, provider, found.as_deref(), id.as_ref())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if nodes.is_empty() {
                    this.save_account(provider, distro.as_deref(), to, cx);
                    return;
                }
                this.open = false;
                this.switching = Some(Switching {
                    provider,
                    distro,
                    to,
                    name,
                    nodes,
                    busy: false,
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn account_patch(
        &self,
        provider: Provider,
        distro: Option<&str>,
        to: Option<AccountId>,
        cx: &App,
    ) -> Option<serde_json::Value> {
        let before = self.store.read(cx).settings();
        let mut after = before.clone();
        after.set_account(provider, distro, to);
        settings_patch(before, &after)
    }

    fn save_account(
        &mut self,
        provider: Provider,
        distro: Option<&str>,
        to: Option<AccountId>,
        cx: &mut Context<Self>,
    ) {
        if let Some(patch) = self.account_patch(provider, distro, to, cx) {
            self.store
                .update(cx, |store, cx| store.save_settings(patch, cx));
        }
    }

    fn move_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(base) = self.base(cx) else {
            return;
        };
        let Some(switching) = &mut self.switching else {
            return;
        };
        switching.busy = true;
        let (provider, distro, to) = (
            switching.provider,
            switching.distro.clone(),
            switching.to.clone(),
        );
        cx.notify();
        let patch = self.account_patch(provider, distro.as_deref(), to, cx);
        cx.spawn_in(window, async move |this, cx| {
            let done = cx
                .background_spawn(async move {
                    if let Some(patch) = patch {
                        link::put_settings(&base, &patch)?;
                    }
                    usage::move_nodes(&base, provider, distro.as_deref())
                })
                .await;
            let lang = cx.update(|_, cx| *cx.global::<Lang>()).unwrap_or(Lang::En);
            let _ = this.update_in(cx, |this, window, cx| {
                this.end_switch(window, cx);
                this.store.update(cx, |store, cx| {
                    store.reload(cx);
                    if let Err(error) = &done {
                        store.tell(error.text(lang), cx);
                    }
                });
                this.refresh(cx);
            });
        })
        .detach();
    }

    fn name_of(
        &self,
        provider: Provider,
        distro: Option<&str>,
        id: Option<&AccountId>,
        cx: &App,
    ) -> String {
        match self
            .own(provider, distro)
            .into_iter()
            .find(|a| a.id.as_ref() == id)
        {
            Some(account) => account_name(account, cx),
            None => usage::unnamed(id.is_none(), cx),
        }
    }

    fn notice_text(&self, record: &EventRecord, cx: &App) -> (String, bool) {
        let lang = *cx.global::<Lang>();
        let say = |text: Str, args: &[(&str, &str)]| text.with(lang, args);
        match &record.entry {
            Entry::NoAccountLeft {
                provider,
                resets_at,
                ..
            } => {
                let head = say(
                    Str::SwitchNoticeNoneLeft,
                    &[("provider", provider_label(*provider, cx))],
                );
                let text = match resets_at {
                    Some(at) => {
                        let time = usage::weekday_time(*at, lang);
                        let tail = say(Str::SwitchNoticeResetsAt, &[("time", &time)]);
                        format!("{head} \u{00B7} {tail}")
                    }
                    None => head,
                };
                (text, true)
            }
            Entry::AccountSwitched {
                provider,
                distro,
                from,
                to,
                why,
                moved,
                later,
                failed,
            } => {
                let place = distro.as_deref();
                let label = provider_label(*provider, cx);
                let to_name = self.name_of(*provider, place, to.as_ref(), cx);
                let head = match why {
                    SwitchCause::Manual => say(
                        Str::SwitchNoticeManual,
                        &[("provider", label), ("to", &to_name)],
                    ),
                    SwitchCause::RateLimit => {
                        let from_name = self.name_of(*provider, place, from.as_ref(), cx);
                        say(
                            Str::SwitchNoticeAuto,
                            &[
                                ("provider", label),
                                ("from", &from_name),
                                ("to", &to_name),
                                ("cause", Str::SwitchNoticeCauseRateLimit.text(lang)),
                            ],
                        )
                    }
                };
                let counts = [
                    (*moved, Str::SwitchNoticeMoved),
                    (*later, Str::SwitchNoticeLater),
                    (*failed, Str::SwitchNoticeFailed),
                ]
                .into_iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, text)| say(text, &[("n", &n.to_string())]));
                let text = std::iter::once(head)
                    .chain(counts)
                    .collect::<Vec<_>>()
                    .join(" \u{00B7} ");
                (text, false)
            }
            _ => (String::new(), false),
        }
    }
}

fn is_switch(entry: &Entry) -> bool {
    matches!(
        entry,
        Entry::AccountSwitched { .. } | Entry::NoAccountLeft { .. }
    )
}

fn text(label: impl Into<SharedString>) -> Div {
    div().flex_none().whitespace_nowrap().child(label.into())
}

fn name_cell(name: String) -> Div {
    div().flex_none().max_w(px(NAME_W)).truncate().child(name)
}

fn inline_marks(limits: &Limits, c: &Colors, cx: &App) -> Vec<Div> {
    usage::inline_marks(limits, cx)
        .into_iter()
        .map(|(label, used)| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(5.0))
                .font_family(fonts::TERMINAL_FAMILY)
                .child(div().w(px(METER_W)).flex().child(usage::meter(used, c)))
                .child(text(format!("{label} {used}%")))
        })
        .collect()
}

fn segment(name: String, limits: &Limits, c: &Colors, cx: &App) -> Vec<Div> {
    if matches!(limits, Limits::NeedsLogin) {
        return vec![text(say(cx, Str::StatusBarLoginToSee))];
    }
    if usage::cli_missing(limits) {
        return vec![text(say(cx, Str::AccountsNotInstalled))];
    }
    let mut parts = vec![name_cell(name)];
    parts.extend(inline_marks(limits, c, cx));
    if matches!(limits, Limits::Failed { .. }) {
        parts.push(text(say(cx, Str::StatusBarUnavailable)).text_color(c.err));
    }
    parts
}

fn provider_mark(provider: Provider, c: &Colors) -> AnyElement {
    let (id, label) = match provider {
        Provider::Claude => ("claude", "Claude"),
        Provider::Codex => ("codex", "Codex"),
    };
    agent_mark(Some(id), label, MARK, c.ink_2)
}

fn cursor_mark(c: &Colors) -> AnyElement {
    agent_mark(Some("cursor"), "Cursor", MARK, c.ink_2)
}

fn chevron(up: bool, c: &Colors) -> gpui::Svg {
    icon(
        if up {
            Lucide::ChevronUp
        } else {
            Lucide::ChevronDown
        },
        MARK,
        c.ink_3,
    )
}

fn separator(c: &Colors) -> Div {
    div()
        .w(px(1.0))
        .h(px(14.0))
        .mx(px(2.0))
        .flex_none()
        .bg(c.line)
}

impl Render for Usage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.global::<Theme>().colors;
        let distro = self.distro(cx);
        let place = distro.as_deref();
        let this = cx.entity();

        let mut summary: Vec<AnyElement> = Vec::new();
        for (index, provider) in PROVIDERS.into_iter().enumerate() {
            if index > 0 {
                summary.push(separator(&c).into_any_element());
            }
            summary.push(provider_mark(provider, &c));
            match self.active(provider, place, cx) {
                None => summary.push(text(say(cx, Str::StatusBarLoading)).into_any_element()),
                Some(account) => summary.extend(
                    segment(account_name(account, cx), &account.usage.limits, &c, cx)
                        .into_iter()
                        .map(IntoElement::into_any_element),
                ),
            }
        }
        let cursor = self
            .cursor
            .iter()
            .flatten()
            .find(|entry| entry.distro.as_deref() == place);
        if let Some(entry) = cursor {
            summary.push(separator(&c).into_any_element());
            summary.push(cursor_mark(&c));
            let name = entry
                .usage
                .email
                .clone()
                .unwrap_or_else(|| usage::unnamed(true, cx));
            summary.extend(
                segment(name, &entry.usage.limits, &c, cx)
                    .into_iter()
                    .map(IntoElement::into_any_element),
            );
        }

        let open = self.open;
        let mut button = div()
            .id("usage-button")
            .flex_none()
            .h_full()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(10.0))
            .overflow_hidden()
            .cursor_pointer()
            .hover(move |style| style.bg(c.surface_hover).text_color(c.ink))
            .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx)))
            .children(summary);
        if open {
            button = button.bg(c.accent_wash).text_color(c.ink);
        }
        if self.loading {
            button = button.opacity(0.7);
        }

        let refresh = div()
            .id("usage-refresh")
            .w(px(26.0))
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.0))
            .when(self.loading, |style| style.opacity(0.5))
            .hover(move |style| style.bg(c.surface_hover))
            .tooltip(tip(say(cx, Str::StatusBarRefresh).to_owned()))
            .on_click(cx.listener(|this, _, _, cx| {
                if !this.loading {
                    this.refresh(cx);
                }
            }))
            .child(icon(Lucide::RotateCw, MARK, c.ink_2));

        let notice = self.latest.as_ref().and_then(|record| {
            let closed = self.store.read(cx).settings().closed_switch_notice;
            if closed == Some(record.id) {
                return None;
            }
            let (message, warn) = self.notice_text(record, cx);
            let tone = if warn { c.warn } else { c.ink };
            let close = div()
                .id("usage-notice-close")
                .px(px(4.0))
                .cursor_pointer()
                .text_color(c.ink_3)
                .tooltip(tip(say(cx, Str::SwitchNoticeClose).to_owned()))
                .on_click(cx.listener(|this, _, _, cx| this.close_notice(cx)))
                .child("\u{00D7}");
            Some(
                div()
                    .min_w_0()
                    .max_w(px(NOTICE_W))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .border_l_1()
                    .border_color(c.line)
                    .text_color(tone)
                    .child(div().min_w_0().truncate().child(message))
                    .child(close),
            )
        });

        let mut host = div()
            .relative()
            .h_full()
            .min_w_0()
            .flex()
            .items_center()
            .child(
                div()
                    .relative()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(button)
                    .child(
                        canvas(
                            {
                                let anchor = self.anchor.clone();
                                move |bounds, _, _| anchor.set(bounds)
                            },
                            |_, _, _, _| (),
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    ),
            )
            .child(refresh)
            .children(notice);
        if open && self.accounts.is_some() {
            host = host.child(self.backdrop(window, cx)).child(self.popover(
                distro.as_deref(),
                &c,
                window,
                cx,
            ));
        }
        if let Some(switching) = self.switching.clone() {
            keep_focus(&self.dialog_focus, window, cx);
            let (keep, all, cancel) = (this.clone(), this.clone(), this);
            let (provider, distro, to) = (
                switching.provider,
                switching.distro.clone(),
                switching.to.clone(),
            );
            let content = usage::switch_content(
                &switching,
                move |window, cx| {
                    let (distro, to) = (distro.clone(), to.clone());
                    keep.update(cx, |this, cx| {
                        this.end_switch(window, cx);
                        this.save_account(provider, distro.as_deref(), to, cx);
                    });
                },
                move |window, cx| all.update(cx, |this, cx| this.move_all(window, cx)),
                cx,
            );
            host = host.child(modal(
                &self.dialog_focus,
                content,
                Rc::new(move |window, cx| {
                    cancel.update(cx, |this, cx| this.end_switch(window, cx))
                }),
                window,
                cx,
            ));
        }
        host
    }
}

pub fn version() -> Div {
    text(format!("v{VERSION}"))
        .px(px(10.0))
        .font_family(fonts::TERMINAL_FAMILY)
}
