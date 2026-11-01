mod pane;
mod status;
mod title;
mod zoom;

use api::settings::PaneWidths;
use gpui::{
    AnyView, App, AppContext, Context, Entity, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Render, Styled, Subscription, Window, div, px,
};

use crate::fonts;
use crate::settings::{Dialog, DialogEvent};
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::theme::Theme;

use pane::{Pane, Widths};

gpui::actions!(loom, [OpenSettings]);

const GRIP: f32 = 7.0;
const GRIP_BEFORE: f32 = 3.0;
const GRIP_ALPHA: f32 = 0.55;

struct Drag {
    pane: Pane,
    from: Pixels,
    start: f32,
    now: f32,
}

pub fn zoom(store: Entity<Store>, window: &mut Window, cx: &mut App) -> Entity<zoom::Zoom> {
    cx.new(|cx| zoom::Zoom::new(store, window, cx))
}

pub struct Shell {
    store: Entity<Store>,
    zoom: Entity<zoom::Zoom>,
    main: AnyView,
    left: Option<AnyView>,
    right: Option<AnyView>,
    seen: PaneWidths,
    kept: PaneWidths,
    drag: Option<Drag>,
    focus: FocusHandle,
    settings: Option<(Entity<Dialog>, Subscription)>,
}

impl Shell {
    pub fn new(
        store: Entity<Store>,
        zoom: Entity<zoom::Zoom>,
        main: AnyView,
        left: Option<AnyView>,
        right: Option<AnyView>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&store, |this, store, cx| {
            let saved = store.read(cx).settings().pane_widths.clone();
            if saved != this.seen {
                this.seen = saved;
                this.kept = PaneWidths::default();
            }
            cx.notify();
        })
        .detach();
        let seen = store.read(cx).settings().pane_widths.clone();
        Self {
            store,
            zoom,
            main,
            left,
            right,
            seen,
            kept: PaneWidths::default(),
            drag: None,
            focus: cx.focus_handle(),
            settings: None,
        }
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let store = self.store.clone();
        let dialog = cx.new(|cx| Dialog::new(store, window, cx));
        let closed = cx.subscribe_in(
            &dialog,
            window,
            |this, dialog, _: &DialogEvent, window, cx| {
                let back = dialog.read(cx).restore().cloned();
                window.focus(back.as_ref().unwrap_or(&this.focus), cx);
                this.settings = None;
                cx.notify();
            },
        );
        self.settings = Some((dialog, closed));
        cx.notify();
    }

    fn widths(&self, window: &Window, cx: &App) -> Widths {
        let saved = &self.store.read(cx).settings().pane_widths;
        Widths {
            saved: PaneWidths {
                side: self.kept.side.or(saved.side),
                dock: self.kept.dock.or(saved.dock),
            },
            closed: self.store.read(cx).settings().closed_panes.clone(),
            window: f32::from(window.viewport_size().width),
            drag: self.drag.as_ref().map(|d| (d.pane, d.now)),
        }
    }

    fn grab(&mut self, pane: Pane, event: &MouseDownEvent, width: f32, cx: &mut Context<Self>) {
        if event.click_count >= 2 {
            self.commit(pane, pane.initial(), cx);
            return;
        }
        self.drag = Some(Drag {
            pane,
            from: event.position.x,
            start: width,
            now: width,
        });
        cx.notify();
    }

    fn slide(&mut self, event: &MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
        let Some(drag) = &self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            cx.notify();
            return;
        }
        let moved = f32::from(event.position.x - drag.from);
        let wanted = drag.start + drag.pane.grow() * moved;
        let pane = drag.pane;
        let clamped = self.widths(window, cx).clamp(pane, wanted);
        if let Some(drag) = &mut self.drag {
            drag.now = clamped;
        }
        cx.notify();
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.take() {
            if drag.now != drag.start {
                self.commit(drag.pane, drag.now, cx);
            } else {
                cx.notify();
            }
        }
    }

    fn commit(&mut self, pane: Pane, width: f32, cx: &mut Context<Self>) {
        let width = width.round() as u16;
        match pane {
            Pane::Side => self.kept.side = Some(width),
            Pane::Dock => self.kept.dock = Some(width),
        }
        cx.notify();
        let patch = serde_json::json!({ "paneWidths": { pane.key(): width } });
        self.store
            .update(cx, |store, cx| store.save_settings(patch, cx));
    }
}

impl Shell {
    fn grip(&self, pane: Pane, widths: &Widths, cx: &Context<Self>) -> impl IntoElement {
        let width = widths.width(pane);
        let accent = Hsla::from(cx.global::<Theme>().colors.accent).opacity(GRIP_ALPHA);
        let active = self.drag.as_ref().is_some_and(|d| d.pane == pane);
        let edge = div()
            .id(pane.key())
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(GRIP))
            .cursor_col_resize()
            .hover(move |style| style.bg(accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    this.grab(pane, e, width, cx);
                    cx.stop_propagation();
                }),
            );
        let edge = if active { edge.bg(accent) } else { edge };
        match pane {
            Pane::Side => edge.left(px(widths.shown(pane) - GRIP_BEFORE)),
            Pane::Dock => edge.right(px(widths.shown(pane) - (GRIP - GRIP_BEFORE))),
        }
    }

    fn slot(widths: &Widths, pane: Pane, theme: &Theme, view: Option<AnyView>) -> gpui::Div {
        let slot = div()
            .w(px(widths.shown(pane)))
            .h_full()
            .flex_none()
            .overflow_hidden()
            .bg(theme.colors.panel)
            .border_color(theme.colors.line);
        let slot = match pane {
            Pane::Side => slot.border_r_1(),
            Pane::Dock => slot.border_l_1(),
        };
        slot.children(view)
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        let widths = self.widths(window, cx);
        let theme = cx.global::<Theme>();
        let lang = *cx.global::<Lang>();
        let shell = cx.entity();
        let title = title::bar(
            window,
            &theme.colors,
            Str::SettingsTitle.text(lang),
            move |window, cx| {
                shell.update(cx, |this, cx| this.open_settings(window, cx));
            },
        );
        let status = status::bar(self.store.read(cx), &theme.colors, lang, &self.zoom);
        let left = Self::slot(&widths, Pane::Side, theme, self.left.clone());
        let right = Self::slot(&widths, Pane::Dock, theme, self.right.clone());
        let side_grip = if widths.open(Pane::Side) {
            Some(self.grip(Pane::Side, &widths, cx))
        } else {
            None
        };
        let dock_grip = if widths.open(Pane::Dock) {
            Some(self.grip(Pane::Dock, &widths, cx))
        } else {
            None
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .track_focus(&self.focus)
            .bg(theme.colors.bg)
            .text_color(theme.colors.ink)
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                let stroke = &e.keystroke;
                if stroke.key == "," && stroke.modifiers.secondary(cx) {
                    this.open_settings(window, cx);
                    cx.stop_propagation();
                }
            }))
            .font_family(fonts::UI_FAMILY)
            .on_action(
                cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                this.slide(e, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.release(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.release(cx)),
            )
            .child(title)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(left)
                    .child(div().flex_1().min_w_0().h_full().child(self.main.clone()))
                    .child(right)
                    .children(side_grip)
                    .children(dock_grip),
            )
            .child(status)
            .children(self.settings.as_ref().map(|(dialog, _)| dialog.clone()))
    }
}
