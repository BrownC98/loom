use std::rc::Rc;
use std::time::Duration;

use api::graph::GraphEvent;
use api::graph::events::{Entry, EventLog, EventRecord, SwitchCause};
use api::ids::AccountId;
use api::profile::Provider;
use api::repos::PlaceView;
use api::settings::accounts::{AccountView, PlaceUsage};
use api::settings::usage::Limits;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Anchor, AnyElement, App, AppContext, Context, Div, Entity, FocusHandle, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render, Rgba,
    SharedString, StatefulInteractiveElement, Styled, Window, anchored, canvas, deferred, div,
    point, px, rgb,
};

use crate::fonts;
use crate::link::{self, VERSION};
use crate::settings::{keep_focus, modal, settings_patch, want_accounts};
use crate::store::{Level, Store};
use crate::strings::{Lang, Str};
use crate::theme::{Colors, Theme};
use crate::views::dock::ui::{button, say, say_with};
use crate::views::tooltip::tip;
use crate::views::usage::{self, PROVIDERS, Switching, account_name, active_index, provider_label};

const ACCOUNTS: &str = "/api/accounts";
const CURSOR: &str = "/api/usage/cursor";
const EVENTS: &str = "/api/events/log";
const REFRESH: Duration = Duration::from_secs(5 * 60);
const POPOVER_W: f32 = 360.0;
const POPOVER_GAP: f32 = 6.0;
const POPOVER_MARGIN: f32 = 80.0;
const NAME_W: f32 = 220.0;
const NOTICE_W: f32 = 520.0;
const MARK: f32 = 14.0;
const METER_W: f32 = 36.0;
const LABEL_W: f32 = 92.0;
const PERCENT_W: f32 = 44.0;
const LEFT_W: f32 = 92.0;
const CLAUDE_ORANGE: u32 = 0xd97757;

#[derive(Clone, Copy, PartialEq)]
enum Brand {
    Claude,
    Codex,
    Cursor,
}

impl Brand {
    fn of(provider: Provider) -> Brand {
        match provider {
            Provider::Claude => Brand::Claude,
            Provider::Codex => Brand::Codex,
        }
    }
}

type Poly = Vec<(f32, f32)>;

fn ring(x: f32, y: f32, r: f32, steps: usize) -> Poly {
    (0..=steps)
        .map(|i| {
            let angle =
                i as f32 / steps as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            (x + r * angle.cos(), y + r * angle.sin())
        })
        .collect()
}

fn shapes(brand: Brand) -> Vec<Poly> {
    match brand {
        Brand::Claude => (0..8)
            .map(|k| {
                let angle = k as f32 * std::f32::consts::TAU / 8.0;
                vec![
                    (12.0 + 3.5 * angle.cos(), 12.0 + 3.5 * angle.sin()),
                    (12.0 + 10.0 * angle.cos(), 12.0 + 10.0 * angle.sin()),
                ]
            })
            .collect(),
        Brand::Codex => {
            let outer = ring(12.0, 12.0, 9.0, 6);
            let inner = ring(12.0, 12.0, 4.5, 6);
            vec![outer, inner]
        }
        Brand::Cursor => {
            let hex = ring(12.0, 12.0, 10.0, 6);
            vec![
                hex.clone(),
                vec![hex[0], (12.0, 12.0), hex[2]],
                vec![(12.0, 12.0), hex[4]],
            ]
        }
    }
}

fn draw(size: f32, width: f32, color: Rgba, lines: Vec<Poly>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let scale = size / 24.0;
            for poly in &lines {
                let mut line = gpui::PathBuilder::stroke(px(width * scale));
                for (i, (x, y)) in poly.iter().enumerate() {
                    let at = point(
                        bounds.origin.x + px(x * scale),
                        bounds.origin.y + px(y * scale),
                    );
                    if i == 0 {
                        line.move_to(at);
                    } else {
                        line.line_to(at);
                    }
                }
                if let Ok(path) = line.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .size(px(size))
    .flex_none()
}

fn mark(brand: Brand, c: &Colors) -> impl IntoElement {
    let color = match brand {
        Brand::Claude => rgb(CLAUDE_ORANGE),
        Brand::Codex | Brand::Cursor => c.ink,
    };
    draw(MARK, 2.4, color, shapes(brand))
}

fn chevron(up: bool, c: &Colors) -> impl IntoElement {
    let (near, far) = if up { (15.0, 9.0) } else { (9.0, 15.0) };
    draw(
        14.0,
        2.0,
        c.ink_3,
        vec![vec![(6.0, near), (12.0, far), (18.0, near)]],
    )
}

fn refresh_glyph(c: &Colors) -> impl IntoElement {
    let mut arc = ring(12.0, 12.0, 8.0, 18);
    arc.truncate(15);
    let tip = arc[arc.len() - 1];
    draw(
        14.0,
        2.0,
        c.ink_2,
        vec![
            arc,
            vec![(tip.0 + 4.0, tip.1 - 1.0), tip, (tip.0 + 0.5, tip.1 + 4.5)],
        ],
    )
}

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
}

impl Usage {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
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
        if !self.open {
            self.refresh(cx);
            window.focus(&self.focus, cx);
        }
        self.open = !self.open;
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
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
        cx: &mut Context<Self>,
    ) {
        let Some(base) = self.base(cx) else {
            return;
        };
        cx.spawn(async move |this, cx| {
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

    fn move_all(&mut self, cx: &mut Context<Self>) {
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
        let Some(base) = self.base(cx) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_spawn(async move {
                    if let Some(patch) = patch {
                        link::put_settings(&base, &patch)?;
                    }
                    usage::move_nodes(&base, provider, distro.as_deref())
                })
                .await;
            let lang = cx.update(|cx| *cx.global::<Lang>());
            let _ = this.update(cx, |this, cx| {
                this.switching = None;
                this.store.update(cx, |store, cx| {
                    store.reload(cx);
                    if let Err(error) = &done {
                        store.tell(error.text(lang), Level::Error, cx);
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

fn separator(c: &Colors) -> Div {
    div().w(px(1.0)).h(px(14.0)).flex_none().bg(c.line)
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
            summary.push(mark(Brand::of(provider), &c).into_any_element());
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
            summary.push(mark(Brand::Cursor, &c).into_any_element());
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
            .size(px(18.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .when(self.loading, |style| style.opacity(0.5))
            .hover(move |style| style.bg(c.surface_hover))
            .tooltip(tip(say(cx, Str::StatusBarRefresh).to_owned()))
            .on_click(cx.listener(|this, _, _, cx| {
                if !this.loading {
                    this.refresh(cx);
                }
            }))
            .child(refresh_glyph(&c));

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
            .gap(px(2.0))
            .child(div().min_w_0().h_full().overflow_hidden().child(button))
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
                move |_, cx| {
                    let (distro, to) = (distro.clone(), to.clone());
                    keep.update(cx, |this, cx| {
                        this.switching = None;
                        this.save_account(provider, distro.as_deref(), to, cx);
                        cx.notify();
                    });
                },
                move |_, cx| all.update(cx, |this, cx| this.move_all(cx)),
                cx,
            );
            host = host.child(modal(
                &self.dialog_focus,
                content,
                Rc::new(move |_, cx| {
                    cancel.update(cx, |this, cx| {
                        this.switching = None;
                        cx.notify();
                    })
                }),
                window,
                cx,
            ));
        }
        host
    }
}

impl Usage {
    fn backdrop(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let view = window.viewport_size();
        deferred(
            anchored()
                .position(point(px(0.0), px(0.0)))
                .anchor(Anchor::TopLeft)
                .child(div().w(view.width).h(view.height).occlude().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.close(cx)),
                )),
        )
        .into_any_element()
    }

    fn popover(
        &self,
        distro: Option<&str>,
        c: &Colors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let max_h = f32::from(window.viewport_size().height) - POPOVER_MARGIN;
        let mut body = div()
            .id("usage-popover")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close(cx);
                    cx.stop_propagation();
                }
            }))
            .w(px(POPOVER_W))
            .max_h(px(max_h))
            .overflow_y_scroll()
            .occlude()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(14.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(c.line)
            .bg(c.surface)
            .shadow_lg()
            .text_size(px(13.0))
            .text_color(c.ink)
            .child(self.heading(c, cx));
        for provider in PROVIDERS {
            body = body.child(self.section(provider, distro, c, cx));
        }
        let cursor = self
            .cursor
            .iter()
            .flatten()
            .find(|entry| entry.distro.as_deref() == distro);
        if let Some(entry) = cursor {
            body = body.child(self.cursor_section(entry, c, cx));
        }
        body = body.child(self.foot(distro, c, cx));
        deferred(
            anchored()
                .anchor(Anchor::BottomLeft)
                .offset(point(px(0.0), px(-POPOVER_GAP)))
                .snap_to_window()
                .child(body),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn heading(&self, c: &Colors, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(14.0))
            .child(div().flex_1().child(say(cx, Str::StatusBarUsage)))
            .child(
                div()
                    .id("usage-collapse")
                    .size(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover({
                        let hover = c.surface_hover;
                        move |style| style.bg(hover)
                    })
                    .tooltip(tip(say(cx, Str::StatusBarCollapse).to_owned()))
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
                    .child(chevron(false, c)),
            )
    }

    fn windows(&self, limits: &Limits, c: &Colors, cx: &App) -> Div {
        let mut grid = div().flex().flex_col().gap(px(6.0)).text_size(px(12.0));
        for line in usage::lines(limits, cx) {
            grid = grid.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_color(c.ink_2)
                    .child(
                        div()
                            .w(px(LABEL_W))
                            .flex_none()
                            .truncate()
                            .child(line.label),
                    )
                    .child(usage::meter(line.used, c))
                    .child(
                        div()
                            .w(px(PERCENT_W))
                            .flex_none()
                            .flex()
                            .justify_end()
                            .font_family(fonts::TERMINAL_FAMILY)
                            .child(format!("{}%", line.used)),
                    )
                    .child(
                        div()
                            .w(px(LEFT_W))
                            .flex_none()
                            .flex()
                            .justify_end()
                            .font_family(fonts::TERMINAL_FAMILY)
                            .text_color(c.ink_3)
                            .child(line.left),
                    ),
            );
        }
        if let Some(problem) = usage::problem(limits, cx) {
            grid = grid.child(div().text_color(c.err).child(problem));
        }
        let reset = usage::reset_text(limits, cx);
        if !reset.is_empty() {
            grid = grid.child(div().text_color(c.ink_3).child(reset));
        }
        grid
    }

    fn section(
        &self,
        provider: Provider,
        distro: Option<&str>,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> Div {
        let expanded = self.expanded == Some(provider);
        let active = self.active(provider, distro, cx);
        let mut head = div()
            .id(("usage-provider", provider as usize))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(6.0))
            .py(px(4.0))
            .mx(px(-6.0))
            .my(px(-4.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .hover({
                let hover = c.surface_hover;
                move |style| style.bg(hover)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.expanded = if this.expanded == Some(provider) {
                    None
                } else {
                    Some(provider)
                };
                cx.notify();
            }))
            .child(mark(Brand::of(provider), c));
        if let Some(account) = active {
            head = head.child(name_cell(account_name(account, cx)).text_color(c.ink_2));
        }
        head = head.child(div().flex_1()).child(chevron(expanded, c));
        if expanded {
            head = head.bg(c.surface_hover);
        }
        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .pt(px(12.0))
            .border_t_1()
            .border_color(c.line)
            .child(head);
        if let Some(account) = active {
            section = section.child(self.windows(&account.usage.limits, c, cx));
        }
        if expanded {
            let own = self.own(provider, distro);
            let mut list = div().flex().flex_col().gap(px(2.0));
            for (index, account) in own.iter().enumerate() {
                list =
                    list.child(self.account_row(provider, distro, account, active, index, c, cx));
            }
            section = section.child(list);
        }
        section
    }

    #[allow(clippy::too_many_arguments)]
    fn account_row(
        &self,
        provider: Provider,
        distro: Option<&str>,
        account: &AccountView,
        active: Option<&AccountView>,
        index: usize,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = active.is_some_and(|a| std::ptr::eq(a, account));
        let name = account_name(account, cx);
        let sub = if current {
            say(cx, Str::AccountsCurrent).to_owned()
        } else if let Limits::Ready {
            session: Some(session),
            ..
        } = &account.usage.limits
        {
            say_with(cx, Str::UsageUsed, &[("n", &session.used.to_string())])
        } else if matches!(account.usage.limits, Limits::NeedsLogin) {
            say(cx, Str::UsageLoginRequired).to_owned()
        } else {
            String::new()
        };
        let (place, to, picked) = (distro.map(str::to_owned), account.id.clone(), name.clone());
        let hover = c.surface_hover;
        let mut row = div()
            .id(("usage-account", provider as usize * 100 + index))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.choose(provider, place.clone(), to.clone(), picked.clone(), cx)
            }))
            .child(name_cell(name))
            .child(div().flex_1())
            .child(text(sub).text_size(px(12.0)).text_color(c.ink_3));
        if current {
            row = row.bg(c.accent_wash);
        }
        row
    }

    fn cursor_section(&self, entry: &PlaceUsage, c: &Colors, cx: &App) -> Div {
        let name = entry
            .usage
            .email
            .clone()
            .unwrap_or_else(|| usage::unnamed(true, cx));
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .pt(px(12.0))
            .border_t_1()
            .border_color(c.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(mark(Brand::Cursor, c))
                    .child(name_cell(name).text_color(c.ink_2)),
            )
            .child(self.windows(&entry.usage.limits, c, cx))
    }

    fn foot(&self, _: Option<&str>, c: &Colors, cx: &mut Context<Self>) -> Div {
        let read = usage::oldest_read(
            self.accounts.as_deref().unwrap_or_default(),
            self.cursor.as_deref().unwrap_or_default(),
        )
        .map(|at| usage::fetched(at, cx));
        div()
            .flex()
            .items_center()
            .pt(px(8.0))
            .border_t_1()
            .border_color(c.line)
            .text_size(px(12.0))
            .text_color(c.ink_3)
            .child(div().flex_1().children(read))
            .child(
                button("usage-manage", cx)
                    .min_h(px(26.0))
                    .px(px(8.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close(cx);
                        want_accounts(cx);
                        window.dispatch_action(Box::new(crate::views::shell::OpenSettings), cx);
                    }))
                    .child(say(cx, Str::StatusBarManage)),
            )
    }
}

pub fn version() -> Div {
    text(format!("v{VERSION}"))
        .px(px(10.0))
        .font_family(fonts::TERMINAL_FAMILY)
}
