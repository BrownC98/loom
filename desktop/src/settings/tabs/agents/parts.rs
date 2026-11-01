use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Anchor, AnyElement, App, AppContext, Div, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, Stateful, Styled, StyledText,
    WeakEntity, Window, anchored, deferred, div, point, px,
};
use serde::de::DeserializeOwned;

use crate::link;
use crate::settings::widgets::confirm;
use crate::settings::{Ctx, Dialog};
use crate::strings::{Lang, Str};
use crate::theme::Colors;
use crate::views::dock::ui::{SMALL, TEXT, button, colors, say, say_with};
use api::repos::PlaceView;

const OVERLAY_PRIORITY: usize = 3;

pub type Act = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct Shared<T>(Rc<RefCell<T>>);

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: Default> Default for Shared<T> {
    fn default() -> Self {
        Self(Rc::default())
    }
}

impl<T> Shared<T> {
    pub fn read<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        read(&self.0.borrow())
    }

    pub fn write(&self, write: impl FnOnce(&mut T)) {
        write(&mut self.0.borrow_mut());
    }

    pub fn bind(&self, ctx: &Ctx) -> Bound<T> {
        Bound {
            shared: self.clone(),
            dialog: ctx.dialog.clone(),
        }
    }
}

pub struct Bound<T> {
    shared: Shared<T>,
    dialog: WeakEntity<Dialog>,
}

impl<T> Clone for Bound<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            dialog: self.dialog.clone(),
        }
    }
}

impl<T> Bound<T> {
    pub fn change(&self, cx: &mut App, change: impl FnOnce(&mut T)) {
        change(&mut self.shared.0.borrow_mut());
        let _ = self.dialog.update(cx, |_, cx| cx.notify());
    }

    pub fn read<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        self.shared.read(read)
    }
}

struct Slot<T> {
    value: Option<Rc<T>>,
    error: Option<String>,
    ticket: u64,
    started: bool,
}

pub struct Remote<T>(Rc<RefCell<Slot<T>>>);

impl<T> Clone for Remote<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for Remote<T> {
    fn default() -> Self {
        Self(Rc::new(RefCell::new(Slot {
            value: None,
            error: None,
            ticket: 0,
            started: false,
        })))
    }
}

impl<T: DeserializeOwned + Send + 'static> Remote<T> {
    pub fn value(&self) -> Option<Rc<T>> {
        self.0.borrow().value.clone()
    }

    pub fn error(&self) -> Option<String> {
        self.0.borrow().error.clone()
    }

    pub fn reloader(&self, path: &str, ctx: &Ctx, cx: &App) -> Reload<T> {
        Reload {
            remote: self.clone(),
            path: path.to_owned(),
            base: ctx.store.read(cx).base().map(str::to_owned),
            dialog: ctx.dialog.clone(),
        }
    }

    pub fn ensure(&self, path: &str, ctx: &Ctx, cx: &mut App) -> bool {
        let first = !std::mem::replace(&mut self.0.borrow_mut().started, true);
        if first {
            self.reloader(path, ctx, cx).run(cx);
        }
        first
    }
}

pub struct Reload<T> {
    remote: Remote<T>,
    path: String,
    base: Option<String>,
    dialog: WeakEntity<Dialog>,
}

impl<T> Clone for Reload<T> {
    fn clone(&self) -> Self {
        Self {
            remote: self.remote.clone(),
            path: self.path.clone(),
            base: self.base.clone(),
            dialog: self.dialog.clone(),
        }
    }
}

impl<T: DeserializeOwned + Send + 'static> Reload<T> {
    pub fn repeat(&self, every: Duration, cx: &mut App) {
        let reload = self.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(every).await;
                let alive = cx.update(|cx| {
                    let alive = reload.dialog.upgrade().is_some();
                    if alive {
                        reload.run(cx);
                    }
                    alive
                });
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn run(&self, cx: &mut App) {
        let Some(base) = self.base.clone() else {
            return;
        };
        let ticket = {
            let mut slot = self.remote.0.borrow_mut();
            slot.ticket += 1;
            slot.ticket
        };
        let lang = *cx.global::<Lang>();
        let (remote, path, dialog) = (self.remote.clone(), self.path.clone(), self.dialog.clone());
        cx.spawn(async move |cx| {
            let loaded = cx
                .background_spawn(async move { link::read::<T>(&base, &path) })
                .await;
            {
                let mut slot = remote.0.borrow_mut();
                if slot.ticket != ticket {
                    return;
                }
                match loaded {
                    Ok(value) => {
                        slot.value = Some(Rc::new(value));
                        slot.error = None;
                    }
                    Err(error) => slot.error = Some(error.text(lang)),
                }
            }
            let _ = dialog.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
}

pub fn rows(items: Vec<AnyElement>, cx: &App) -> Div {
    let c = colors(cx);
    let last = items.len().saturating_sub(1);
    div()
        .flex()
        .flex_col()
        .rounded(px(8.0))
        .border_1()
        .border_color(c.line)
        .overflow_hidden()
        .children(items.into_iter().enumerate().map(move |(index, item)| {
            let row = div().flex().flex_col().child(item);
            if index == last {
                row
            } else {
                row.border_b_1().border_color(c.line)
            }
        }))
}

pub fn row() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .px(px(12.0))
        .py(px(9.0))
        .text_size(px(TEXT))
}

pub fn hint(text: StyledText, cx: &App) -> Div {
    div()
        .text_size(px(SMALL))
        .text_color(colors(cx).ink_3)
        .child(text)
}

pub fn badge(text: StyledText, warn: bool, c: &Colors) -> Div {
    let tone = if warn { c.warn } else { c.accent };
    div()
        .px(px(7.0))
        .py(px(1.0))
        .rounded(px(9.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tone)
        .bg(gpui::Hsla::from(tone).opacity(0.14))
        .child(text)
}

pub fn small(id: impl Into<gpui::ElementId>, cx: &App) -> Stateful<Div> {
    button(id, cx).min_h(px(26.0)).px(px(8.0))
}

pub fn labelled(label: StyledText, field: impl IntoElement, cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(
            div()
                .text_size(px(SMALL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors(cx).ink_2)
                .child(label),
        )
        .child(field)
}

fn viewport(window: &Window, content: impl IntoElement) -> AnyElement {
    let view = window.viewport_size();
    deferred(
        anchored()
            .position(point(px(0.0), px(0.0)))
            .anchor(Anchor::TopLeft)
            .child(div().w(view.width).h(view.height).child(content)),
    )
    .with_priority(OVERLAY_PRIORITY)
    .into_any_element()
}

pub fn keyed(
    focus: &FocusHandle,
    on_enter: Option<Act>,
    on_escape: Act,
    content: impl IntoElement,
) -> Div {
    div()
        .size_full()
        .track_focus(focus)
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            match event.keystroke.key.as_str() {
                "escape" => on_escape(window, cx),
                "enter" => match &on_enter {
                    Some(accept) => accept(window, cx),
                    None => return,
                },
                "tab" => {}
                _ => return,
            }
            cx.stop_propagation();
        })
        .child(content)
}

pub fn keep_focus(focus: &FocusHandle, window: &mut Window, cx: &mut App) {
    if !focus.contains_focused(window, cx) {
        window.focus(focus, cx);
    }
}

pub fn ask(
    focus: &FocusHandle,
    message: String,
    accept: &str,
    on_accept: Act,
    on_cancel: Act,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let (accepted, cancelled) = (on_accept.clone(), on_cancel.clone());
    let body = confirm(
        message,
        accept,
        move |_, window, cx| accepted(window, cx),
        move |_, window, cx| cancelled(window, cx),
        cx,
    );
    viewport(window, keyed(focus, Some(on_accept), on_cancel, body))
}

pub fn modal(
    focus: &FocusHandle,
    content: impl IntoElement,
    on_escape: Act,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let c = colors(cx);
    let scrim = div()
        .size_full()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(c.scrim)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(content);
    viewport(window, keyed(focus, None, on_escape, scrim))
}

pub fn place_label(place: &PlaceView, cx: &App) -> String {
    match place {
        PlaceView::Pc => say(cx, Str::PlacePcShort).to_owned(),
        PlaceView::Wsl { distro } => say_with(cx, Str::PlaceWslDistro, &[("distro", distro)]),
        PlaceView::Ssh { host } => say_with(cx, Str::PlaceSshHost, &[("host", host)]),
    }
}
