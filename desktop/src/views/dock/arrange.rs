use api::settings::ArrangeDirection;
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    deferred, div, px,
};

use super::Dock;
use super::ui::{self, GAP, PAD, SMALL};
use crate::strings::Str;

const DIRECTIONS: [ArrangeDirection; 4] = [
    ArrangeDirection::Down,
    ArrangeDirection::Right,
    ArrangeDirection::Up,
    ArrangeDirection::Left,
];
const MENU_TOP: f32 = 40.0;
const MENU_WIDTH: f32 = 130.0;

fn arrow(direction: ArrangeDirection) -> &'static str {
    match direction {
        ArrangeDirection::Down => "↓",
        ArrangeDirection::Right => "→",
        ArrangeDirection::Up => "↑",
        ArrangeDirection::Left => "←",
    }
}

fn name(direction: ArrangeDirection) -> Str {
    match direction {
        ArrangeDirection::Down => Str::GraphDirectionsDown,
        ArrangeDirection::Right => Str::GraphDirectionsRight,
        ArrangeDirection::Up => Str::GraphDirectionsUp,
        ArrangeDirection::Left => Str::GraphDirectionsLeft,
    }
}

impl Dock {
    pub(super) fn arrange_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = ui::colors(cx);
        let settings = self.store.read(cx).settings();
        let (direction, always) = (settings.arrange_direction, settings.always_arrange);
        let store = self.store.clone();
        let run = ui::button("arrange-run", cx)
            .child(ui::say(cx, Str::GraphArrange))
            .on_click({
                let store = store.clone();
                move |_, _, cx| store.update(cx, |store, cx| store.arrange(direction, cx))
            });
        let mut keep = ui::button("arrange-keep", cx).child(ui::say(cx, Str::GraphKeepArranged));
        if always {
            keep = keep.bg(c.accent_wash).border_color(c.accent);
        }
        let keep = keep.on_click({
            let store = store.clone();
            move |_, _, cx| store.update(cx, |store, cx| store.keep_arranged(!always, cx))
        });
        let caret = ui::button("arrange-direction", cx)
            .child(arrow(direction))
            .child("▾")
            .on_click(cx.listener(|dock, _, _, cx| {
                dock.arrange_menu = !dock.arrange_menu;
                cx.notify();
            }));
        let menu = self.arrange_menu.then(|| {
            let items = DIRECTIONS.map(|option| {
                let chosen = option == direction;
                div()
                    .id(("arrange-option", option as usize))
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .px(px(PAD))
                    .py(px(5.0))
                    .text_size(px(SMALL))
                    .text_color(c.ink)
                    .cursor_pointer()
                    .hover(move |style| style.bg(c.surface_hover))
                    .child(arrow(option))
                    .child(ui::say(cx, name(option)))
                    .child(div().flex_1())
                    .child(if chosen { "✓" } else { "" })
                    .on_click({
                        let store = store.clone();
                        cx.listener(move |dock, _, _, cx| {
                            dock.arrange_menu = false;
                            store.update(cx, |store, cx| store.arrange(option, cx));
                            cx.notify();
                        })
                    })
            });
            deferred(
                div()
                    .absolute()
                    .top(px(MENU_TOP))
                    .right(px(PAD))
                    .w(px(MENU_WIDTH))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(c.line_strong)
                    .bg(c.surface)
                    .shadow_md()
                    .children(items),
            )
            .with_priority(1)
        });
        div()
            .relative()
            .flex()
            .gap(px(GAP))
            .px(px(PAD))
            .py(px(PAD))
            .border_b_1()
            .border_color(c.line)
            .child(run)
            .child(keep)
            .child(caret)
            .children(menu)
    }
}
