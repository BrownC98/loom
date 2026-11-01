use api::profile::Provider;
use api::settings::accounts::{AccountView, PlaceUsage};
use api::settings::usage::Limits;
use gpui::{
    Anchor, AnyElement, App, Context, Div, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, StatefulInteractiveElement, Styled, Window, anchored,
    deferred, div, point, px,
};

use super::marks::{Brand, chevron, mark};
use super::{Usage, name_cell, text};
use crate::fonts;
use crate::settings::want_accounts;
use crate::strings::Str;
use crate::theme::Colors;
use crate::views::dock::ui::{button, say, say_with};
use crate::views::tooltip::tip;
use crate::views::usage::{self, PROVIDERS, account_name};

const POPOVER_W: f32 = 360.0;
const POPOVER_GAP: f32 = 6.0;
const POPOVER_MARGIN: f32 = 80.0;
const LABEL_W: f32 = 92.0;
const PERCENT_W: f32 = 44.0;
const LEFT_W: f32 = 92.0;

impl Usage {
    pub(super) fn backdrop(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let view = window.viewport_size();
        deferred(
            anchored()
                .position(point(px(0.0), px(0.0)))
                .anchor(Anchor::TopLeft)
                .child(div().w(view.width).h(view.height).occlude().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close(window, cx)),
                )),
        )
        .into_any_element()
    }

    pub(super) fn popover(
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close(window, cx);
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
                .position(self.anchor.get().origin)
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
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
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
            .on_click(cx.listener(move |this, _, window, cx| {
                this.choose(
                    provider,
                    place.clone(),
                    to.clone(),
                    picked.clone(),
                    window,
                    cx,
                )
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
                    .border_color(gpui::transparent_black())
                    .bg(gpui::transparent_black())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close(window, cx);
                        want_accounts(cx);
                        window.dispatch_action(Box::new(crate::views::shell::OpenSettings), cx);
                    }))
                    .child(say(cx, Str::StatusBarManage)),
            )
    }
}
