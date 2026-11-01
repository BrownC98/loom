use std::sync::Arc;
use std::time::Duration;

use api::graph::GraphEvent;
use api::settings::phone::{DeviceView, InviteView, PhoneView, Served, Tailnet};
use chrono::{Local, TimeZone, Timelike, Utc};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Context, Div, ElementId, Entity, FontWeight, Image, ImageFormat,
    InteractiveElement, IntoElement, ParentElement, Rgba, Stateful, StatefulInteractiveElement,
    Styled, StyledText, Subscription, WeakEntity, Window, div, img, px,
};

use crate::link;
use crate::problem::Failure;
use crate::settings::{Ctx, Dialog, Section};
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::views::dock::ui::{SMALL, TEXT, button, colors, open_url, say, say_with};

const QR: f32 = 148.0;
const QR_PAD: f32 = 4.0;
const KEY_W: f32 = 92.0;
const MINUTE: i64 = 60_000;
const AGO: [(i64, Str); 6] = [
    (365 * 24 * 60 * MINUTE, Str::PairingAgoYear),
    (30 * 24 * 60 * MINUTE, Str::PairingAgoMonth),
    (7 * 24 * 60 * MINUTE, Str::PairingAgoWeek),
    (24 * 60 * MINUTE, Str::PairingAgoDay),
    (60 * MINUTE, Str::PairingAgoHour),
    (MINUTE, Str::PairingAgoMinute),
];

#[derive(Default)]
pub struct State {
    model: Option<Entity<Model>>,
}

struct Invite {
    view: InviteView,
    image: Arc<Image>,
}

struct Model {
    store: Entity<Store>,
    dialog: WeakEntity<Dialog>,
    view: Option<PhoneView>,
    invite: Option<Invite>,
    busy: bool,
    error: String,
    consent: String,
    ticket: u64,
    opened: u64,
    _view_changed: Subscription,
}

impl Model {
    fn new(store: Entity<Store>, dialog: WeakEntity<Dialog>, cx: &mut Context<Self>) -> Self {
        let watch = cx.subscribe(&store, |this, _, event: &GraphEvent, cx| {
            if matches!(event, GraphEvent::ViewChanged) {
                this.refresh(cx);
            }
        });
        let mut model = Self {
            store,
            dialog,
            view: None,
            invite: None,
            busy: false,
            error: String::new(),
            consent: String::new(),
            ticket: 0,
            opened: 0,
            _view_changed: watch,
        };
        model.refresh(cx);
        model
    }

    fn touch(&self, cx: &mut Context<Self>) {
        cx.notify();
        let _ = self.dialog.update(cx, |_, cx| cx.notify());
    }

    fn base(&self, cx: &Context<Self>) -> Option<String> {
        self.store.read(cx).base().map(str::to_owned)
    }

    fn apply(&mut self, next: PhoneView, asked: u64) {
        if !next.invite_open && asked == self.opened {
            self.invite = None;
        }
        self.view = Some(next);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(base) = self.base(cx) else {
            return;
        };
        self.ticket += 1;
        let ticket = self.ticket;
        let asked = self.opened;
        cx.spawn(async move |this, cx| {
            let got = cx
                .background_spawn(async move { link::read::<PhoneView>(&base, "/api/phone") })
                .await;
            let _ = this.update(cx, |model, cx| {
                if ticket != model.ticket {
                    return;
                }
                match got {
                    Ok(next) => model.apply(next, asked),
                    Err(error) => model.error = error.text(*cx.global::<Lang>()),
                }
                model.touch(cx);
            });
        })
        .detach();
    }

    fn act<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(&str) -> Result<T, Failure> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(base) = self.base(cx) else {
            return;
        };
        self.busy = true;
        self.error.clear();
        self.touch(cx);
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { work(&base) }).await;
            let _ = this.update(cx, |model, cx| {
                model.busy = false;
                match result {
                    Ok(value) => done(model, value, cx),
                    Err(error) => model.error = error.text(*cx.global::<Lang>()),
                }
                model.touch(cx);
            });
        })
        .detach();
    }

    fn open_invite(&mut self, cx: &mut Context<Self>) {
        self.act(
            |base| {
                link::post_within::<InviteView>(base, "/api/phone/invite", None, link::SLOW_LIMIT)
            },
            |model, view, cx| {
                model.opened += 1;
                let image = Image::from_bytes(ImageFormat::Svg, view.qr_svg.clone().into_bytes());
                model.invite = Some(Invite {
                    view,
                    image: Arc::new(image),
                });
                model.tick(cx);
            },
            cx,
        );
    }

    fn tick(&self, cx: &mut Context<Self>) {
        let generation = self.opened;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let go = this.update(cx, |model, cx| {
                    let until = match &model.invite {
                        Some(invite) if model.opened == generation => invite.view.until,
                        _ => return false,
                    };
                    if now_ms() >= until {
                        model.invite = None;
                        model.refresh(cx);
                    }
                    model.touch(cx);
                    true
                });
                if !matches!(go, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn close_invite(&mut self, cx: &mut Context<Self>) {
        self.invite = None;
        self.touch(cx);
    }

    fn allow_firewall(&mut self, cx: &mut Context<Self>) {
        self.act(
            |base| link::post_within::<()>(base, "/api/phone/firewall", None, link::SLOW_LIMIT),
            |_, (), _| {},
            cx,
        );
    }

    fn turn_on_serve(&mut self, cx: &mut Context<Self>) {
        self.act(
            |base| link::post_within::<Served>(base, "/api/phone/serve", None, link::SLOW_LIMIT),
            |model, served, cx| {
                model.consent = match served {
                    Served::NeedsConsent { url } => url,
                    Served::On => String::new(),
                };
                model.refresh(cx);
            },
            cx,
        );
    }

    fn recheck(&mut self, cx: &mut Context<Self>) {
        let asked = self.opened;
        self.act(
            |base| link::read::<PhoneView>(base, "/api/phone"),
            move |model, next, _| model.apply(next, asked),
            cx,
        );
    }

    fn remove_device(&mut self, id: i64, cx: &mut Context<Self>) {
        self.act(
            move |base| link::delete(base, &format!("/api/phone/devices/{id}")),
            |model, (), cx| model.refresh(cx),
            cx,
        );
    }

    fn open_consent(&mut self, url: String, cx: &mut Context<Self>) {
        if let Err(error) = open_url(&url) {
            self.error = error;
            self.touch(cx);
        }
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn ago(then: i64, now: i64, cx: &App) -> String {
    let elapsed = (now - then).max(0);
    AGO.iter().find(|(size, _)| elapsed >= *size).map_or_else(
        || say(cx, Str::PairingAgoNow).to_owned(),
        |(size, text)| {
            let count = (elapsed + size / 2) / size;
            say_with(cx, *text, &[("n", &count.to_string())])
        },
    )
}

fn day_and_time(millis: i64, cx: &App) -> String {
    let Some(at) = Local.timestamp_millis_opt(millis).single() else {
        return String::new();
    };
    let (month, day, hour, minute) = (at.format("%-m"), at.format("%-d"), at.hour(), at.minute());
    let clock = format!(
        "{}:{minute:02}",
        [12, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11][hour as usize % 12]
    );
    match *cx.global::<Lang>() {
        Lang::Ko => format!(
            "{month}월 {day}일 {} {clock}",
            if hour < 12 { "오전" } else { "오후" }
        ),
        Lang::En => format!(
            "{month}/{day}, {clock} {}",
            if hour < 12 { "AM" } else { "PM" }
        ),
    }
}

fn dot(color: Rgba) -> Div {
    div().flex_none().size(px(7.0)).rounded_full().bg(color)
}

fn mono(text: StyledText) -> Div {
    div()
        .font_family("monospace")
        .text_size(px(SMALL))
        .child(text)
}

fn hint(text: StyledText, cx: &App) -> Div {
    div()
        .text_size(px(SMALL))
        .text_color(colors(cx).ink_3)
        .child(text)
}

fn action(
    id: impl Into<ElementId>,
    label: StyledText,
    busy: bool,
    primary: bool,
    run: impl Fn(&mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let c = colors(cx);
    let base = if primary {
        div()
            .id(id)
            .flex()
            .items_center()
            .min_h(px(26.0))
            .px(px(10.0))
            .py(px(3.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(c.accent)
            .bg(c.accent)
            .text_size(px(SMALL))
            .text_color(c.accent_ink)
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .hover(|style| style.opacity(0.85))
    } else {
        button(id, cx)
    }
    .self_start()
    .child(label);
    if busy {
        base.opacity(0.5).cursor_not_allowed()
    } else {
        base.on_click(move |_, _, cx| run(cx))
    }
}

fn address_row(key: StyledText, light: Rgba, body: Vec<AnyElement>, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .w(px(KEY_W))
                .flex_none()
                .text_size(px(SMALL))
                .text_color(colors(cx).ink_3)
                .child(key),
        )
        .child(dot(light))
        .children(body)
}

pub fn sections(state: &mut State, ctx: &Ctx, _: &mut Window, cx: &mut App) -> Vec<Section> {
    let model = state
        .model
        .get_or_insert_with(|| {
            let (store, dialog) = (ctx.store.clone(), ctx.dialog.clone());
            cx.new(|cx| Model::new(store, dialog, cx))
        })
        .clone();
    let cx: &App = cx;
    let m = model.read(cx);
    let Some(view) = &m.view else {
        if m.error.is_empty() {
            return Vec::new();
        }
        let mut section = Section::new(None, ctx);
        let error = section.text(&m.error, cx);
        return vec![
            section.child(
                div()
                    .text_size(px(SMALL))
                    .text_color(colors(cx).err)
                    .child(error),
            ),
        ];
    };
    vec![
        connect(&model, m, view, ctx, cx),
        devices(&model, m, view, ctx, cx),
        browser(&model, m, view, ctx, cx),
    ]
}

fn connect(model: &Entity<Model>, m: &Model, view: &PhoneView, ctx: &Ctx, cx: &App) -> Section {
    let c = colors(cx);
    let mut s = Section::new(Some(say(cx, Str::PairingConnect)), ctx);
    let door = |ip: &str| {
        view.port
            .map_or_else(|| ip.to_owned(), |port| format!("{ip}:{port}"))
    };
    let reach = |tailnet: bool| -> Vec<String> {
        view.addresses
            .iter()
            .filter(|a| a.tailnet == tailnet)
            .map(|a| door(&a.ip))
            .collect()
    };
    let (lan, remote) = (reach(false), reach(true));

    let lan_key = s.text(say(cx, Str::PairingLan), cx);
    let lan_body = if lan.is_empty() {
        vec![hint_text(&mut s, Str::PairingNoAddress, cx)]
    } else {
        vec![mono(s.text(&lan.join(" · "), cx)).into_any_element()]
    };
    let remote_key = s.text("Tailscale", cx);
    let remote_body = if remote.is_empty() {
        vec![hint_text(&mut s, Str::PairingNoTailnet, cx)]
    } else {
        vec![
            mono(s.text(&remote.join(" · "), cx)).into_any_element(),
            hint_text(&mut s, Str::PairingOutside, cx),
        ]
    };
    let addresses = div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .text_size(px(TEXT))
        .child(address_row(
            lan_key,
            if lan.is_empty() { c.ask } else { c.ok },
            lan_body,
            cx,
        ))
        .child(address_row(
            remote_key,
            if remote.is_empty() { c.ink_3 } else { c.ok },
            remote_body,
            cx,
        ));
    let where_hint = if view.addresses.is_empty() {
        Str::PairingNoAddressHint
    } else {
        Str::PairingWhere
    };
    let where_hint = hint_text(&mut s, where_hint, cx);

    let pairing: Option<AnyElement> = match &m.invite {
        None => {
            let label = s.text(say(cx, Str::PairingConnect), cx);
            let model = model.clone();
            Some(
                action(
                    "phone-connect",
                    label,
                    m.busy,
                    true,
                    move |cx| model.update(cx, |m, cx| m.open_invite(cx)),
                    cx,
                )
                .into_any_element(),
            )
        }
        Some(invite) => {
            let left = ((invite.view.until - now_ms()).max(0) + 999) / 1000;
            let scan = s.text(say(cx, Str::PairingScan), cx);
            let code_hint = hint_text(&mut s, Str::PairingTypeCode, cx);
            let code = s.text(&invite.view.code, cx);
            let remaining = say_with(
                cx,
                Str::PairingLeft,
                &[
                    ("min", &(left / 60).to_string()),
                    ("sec", &(left % 60).to_string()),
                ],
            );
            let remaining = hint(s.text(&remaining, cx), cx);
            let close = s.text(say(cx, Str::CommonClose), cx);
            let model = model.clone();
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(20.0))
                    .p(px(14.0))
                    .border_1()
                    .border_color(c.line)
                    .rounded(px(8.0))
                    .bg(c.surface)
                    .child(
                        div()
                            .flex_none()
                            .size(px(QR))
                            .p(px(QR_PAD))
                            .rounded(px(6.0))
                            .bg(gpui::white())
                            .child(img(invite.image.clone()).size_full()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(TEXT))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(scan),
                            )
                            .child(code_hint)
                            .child(
                                div()
                                    .font_family("monospace")
                                    .text_size(px(22.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(code),
                            )
                            .child(remaining)
                            .child(action(
                                "phone-close",
                                close,
                                false,
                                false,
                                move |cx| model.update(cx, |m, cx| m.close_invite(cx)),
                                cx,
                            )),
                    )
                    .into_any_element(),
            )
        }
    };

    let firewall = view.firewall.then(|| {
        let note = s.text(say(cx, Str::PairingFirewallHint), cx);
        let label = s.text(say(cx, Str::PairingFirewall), cx);
        let model = model.clone();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .child(hint(note, cx))
            .child(action(
                "phone-firewall",
                label,
                m.busy,
                false,
                move |cx| model.update(cx, |m, cx| m.allow_firewall(cx)),
                cx,
            ))
    });
    let error = (!m.error.is_empty()).then(|| {
        let text = s.text(&m.error, cx);
        div().text_size(px(SMALL)).text_color(c.err).child(text)
    });
    let rest: Vec<AnyElement> = pairing
        .into_iter()
        .chain(firewall.map(IntoElement::into_any_element))
        .chain(error.map(IntoElement::into_any_element))
        .collect();
    fill(s.child(addresses).child(where_hint), rest)
}

fn fill(section: Section, rest: Vec<AnyElement>) -> Section {
    rest.into_iter().fold(section, Section::child)
}

fn hint_text(section: &mut Section, text: Str, cx: &App) -> AnyElement {
    hint(section.text(say(cx, text), cx), cx).into_any_element()
}

fn devices(model: &Entity<Model>, m: &Model, view: &PhoneView, ctx: &Ctx, cx: &App) -> Section {
    let c = colors(cx);
    let mut s = Section::new(Some(say(cx, Str::PairingDevices)), ctx);
    if view.devices.is_empty() {
        let empty = hint_text(&mut s, Str::PairingNoDevices, cx);
        return s.child(empty);
    }
    let now = now_ms();
    let last = view.devices.len() - 1;
    let rows: Vec<_> = view
        .devices
        .iter()
        .enumerate()
        .map(|(index, device)| device_row(model, m, device, index == last, now, &mut s, cx))
        .collect();
    s.child(
        div()
            .border_1()
            .border_color(c.line)
            .rounded(px(8.0))
            .overflow_hidden()
            .children(rows),
    )
}

fn device_row(
    model: &Entity<Model>,
    m: &Model,
    device: &DeviceView,
    last: bool,
    now: i64,
    s: &mut Section,
    cx: &App,
) -> Div {
    let c = colors(cx);
    let name = s.text(&device.label, cx);
    let meta = say_with(
        cx,
        Str::PairingDeviceMeta,
        &[
            ("paired", &day_and_time(device.paired_at, cx)),
            ("seen", &ago(device.last_seen, now, cx)),
        ],
    );
    let meta = s.text(&meta, cx);
    let label = s.text(say(cx, Str::PairingDisconnect), cx);
    let (model, id) = (model.clone(), device.id.0);
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .px(px(12.0))
        .py(px(9.0))
        .when(!last, |row| row.border_b_1().border_color(c.line))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(TEXT))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name),
                )
                .child(mono(meta).text_color(c.ink_3)),
        )
        .child(
            action(
                ElementId::NamedInteger("phone-device".into(), id as u64),
                label,
                m.busy,
                false,
                move |cx| model.update(cx, |m, cx| m.remove_device(id, cx)),
                cx,
            )
            .border_color(gpui::transparent_black())
            .bg(c.panel),
        )
}

fn browser(model: &Entity<Model>, m: &Model, view: &PhoneView, ctx: &Ctx, cx: &App) -> Section {
    let c = colors(cx);
    let mut s = Section::new(Some(say(cx, Str::PairingBrowser)), ctx);
    let (light, status): (Rgba, AnyElement) = match &view.tailnet {
        Tailnet::Ready { host, serving } => {
            let state = say(
                cx,
                if *serving {
                    Str::PairingServeOn
                } else {
                    Str::PairingServeOff
                },
            );
            let host = mono(s.text(host, cx));
            let state = s.text(state, cx);
            (
                if *serving { c.ok } else { c.ink_3 },
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(host)
                    .child("·")
                    .child(state)
                    .into_any_element(),
            )
        }
        Tailnet::Missing => (
            c.ink_3,
            s.text(say(cx, Str::PairingMissing), cx).into_any_element(),
        ),
        Tailnet::Stopped => (
            c.ink_3,
            s.text(say(cx, Str::PairingStopped), cx).into_any_element(),
        ),
    };
    let ready = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(px(TEXT))
        .child(dot(light))
        .child(status);
    let browser_hint = hint_text(&mut s, Str::PairingBrowserHint, cx);

    let controls: Vec<AnyElement> = match &view.tailnet {
        Tailnet::Ready { serving: false, .. } => {
            let mut parts = Vec::new();
            if !m.consent.is_empty() {
                let note = s.text(say(cx, Str::PairingConsent), cx);
                let link_label = s.text(say(cx, Str::PairingConsentLink), cx);
                let (model, url) = (model.clone(), m.consent.clone());
                parts.push(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.0))
                        .child(hint(note, cx))
                        .child(
                            div()
                                .id("phone-consent")
                                .text_size(px(SMALL))
                                .text_color(c.accent)
                                .underline()
                                .cursor_pointer()
                                .child(link_label)
                                .on_click(move |_, _, cx| {
                                    model.update(cx, |m, cx| m.open_consent(url.clone(), cx))
                                }),
                        )
                        .into_any_element(),
                );
            }
            let label = s.text(say(cx, Str::PairingServe), cx);
            let model = model.clone();
            parts.push(
                action(
                    "phone-serve",
                    label,
                    m.busy,
                    false,
                    move |cx| model.update(cx, |m, cx| m.turn_on_serve(cx)),
                    cx,
                )
                .into_any_element(),
            );
            parts
        }
        Tailnet::Ready { .. } => Vec::new(),
        _ => {
            let label = s.text(say(cx, Str::PairingRecheck), cx);
            let model = model.clone();
            vec![
                action(
                    "phone-recheck",
                    label,
                    m.busy,
                    false,
                    move |cx| model.update(cx, |m, cx| m.recheck(cx)),
                    cx,
                )
                .into_any_element(),
            ]
        }
    };
    fill(s.child(ready).child(browser_hint), controls)
}
