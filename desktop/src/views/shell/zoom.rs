use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, Entity, FocusHandle, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window, div, px, relative,
};

use crate::fonts;
use crate::store::Store;
use crate::strings::{Lang, Str};
use crate::theme::Theme;
use crate::views::field::{self, Field, FieldEvent};
use crate::views::icons::Lucide;
use crate::views::rail;
use crate::views::tooltip::tip;

const BUTTON: f32 = 26.0;
const WIDTH: f32 = 40.0;
const FONT: f32 = 11.0;
const PAD_X: f32 = 6.0;
const PAD_Y: f32 = 2.0;
const MARGIN: f32 = 15.0;

pub struct Zoom {
    store: Entity<Store>,
    field: Entity<Field>,
    editing: bool,
    before: Option<FocusHandle>,
}

impl Zoom {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = field::new(Str::GraphZoomEdit, cx);
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(
            &field,
            window,
            |this, _, event: &FieldEvent, window, cx| match event {
                FieldEvent::Enter => this.apply(window, cx),
                FieldEvent::Escape => this.stop(true, window, cx),
                _ => {}
            },
        )
        .detach();
        let handle = field.read(cx).handle();
        cx.on_blur(&handle, window, |this, window, cx| {
            this.stop(false, window, cx);
        })
        .detach();
        Self {
            store,
            field,
            editing: false,
            before: None,
        }
    }

    fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let percent = self.store.read(cx).zoom_percent();
        self.before = window.focused(cx);
        self.field
            .update(cx, |field, cx| field.set_text(&percent.to_string(), cx));
        self.editing = true;
        let handle = self.field.read(cx).handle();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wanted = self.field.read(cx).text().trim().parse::<f32>().ok();
        if let Some(percent) = wanted.filter(|p| p.is_finite() && *p > 0.0) {
            self.store
                .update(cx, |store, cx| store.zoom_to(percent, cx));
        }
        self.stop(true, window, cx);
    }

    fn stop(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editing {
            return;
        }
        self.editing = false;
        let before = self.before.take();
        if let Some(before) = before.filter(|_| restore) {
            window.focus(&before, cx);
        }
        cx.notify();
    }

    fn fold(&mut self, cx: &mut Context<Self>) {
        let folded = self.store.read(cx).settings().minimap_folded;
        let patch = serde_json::json!({ "minimapFolded": !folded });
        self.store
            .update(cx, |store, cx| store.save_settings(patch, cx));
    }
}

impl Render for Zoom {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let (colors, lang) = (theme.colors, *cx.global::<Lang>());
        let store = self.store.read(cx);
        let folded = store.settings().minimap_folded;
        let percent = store.zoom_percent();
        let zoomed = store.zoomed();
        let (glyph, label) = if folded {
            (Lucide::MapPlus, Str::GraphMinimapUnfold)
        } else {
            (Lucide::MapMinus, Str::GraphMinimapFold)
        };
        let toggle = rail::button(
            "zoom-fold",
            glyph,
            label.text(lang).to_owned(),
            BUTTON,
            &colors,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.fold(cx);
                cx.stop_propagation();
            }),
        );
        let level = if self.editing {
            div()
                .id("zoom-edit")
                .w(px(WIDTH))
                .px(px(PAD_X))
                .py(px(PAD_Y))
                .rounded(px(4.0))
                .border_1()
                .border_color(colors.line_strong)
                .bg(colors.panel)
                .text_color(colors.ink)
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.stop(false, window, cx);
                }))
                .child(self.field.clone())
        } else {
            div()
                .id("zoom-level")
                .px(px(PAD_X))
                .py(px(PAD_Y))
                .text_color(colors.ink_2)
                .cursor_text()
                .tooltip(tip(Str::GraphZoomEdit.text(lang).to_owned()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, e: &MouseDownEvent, window, cx| {
                        if e.click_count >= 2 {
                            this.edit(window, cx);
                        }
                        cx.stop_propagation();
                    }),
                )
                .child(format!("{percent}%"))
        };
        div()
            .absolute()
            .right(px(MARGIN))
            .bottom(px(MARGIN))
            .flex()
            .items_center()
            .text_size(px(FONT))
            .line_height(relative(1.4))
            .font_family(fonts::TERMINAL_FAMILY)
            .when(zoomed, |row| row.hidden())
            .child(toggle)
            .child(level)
    }
}
