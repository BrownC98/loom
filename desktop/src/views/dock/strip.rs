use gpui::{Context, IntoElement, ParentElement, StatefulInteractiveElement};

use super::{Dock, Tab, ui};
use crate::strings::Str;
use crate::views::rail::{self, BUTTON};
use crate::views::sidebar::icons::Glyph;

impl Dock {
    pub(super) fn strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = ui::colors(cx);
        let waiting = self.waiting(cx);
        let direction = self.store.read(cx).settings().arrange_direction;
        let store = self.store.clone();
        let open = rail::button(
            "dock-open",
            Glyph::Panel {
                right: true,
                open: true,
            },
            rail::titled(ui::say(cx, Str::DockExpand), "]"),
            BUTTON,
            &c,
        )
        .on_click({
            let store = store.clone();
            move |_, _, cx| store.update(cx, |store, cx| store.set_pane_open("dock", true, cx))
        });
        let arrange = rail::button(
            "dock-arrange",
            Glyph::Arrange,
            ui::say(cx, Str::GraphArrange).to_owned(),
            BUTTON,
            &c,
        )
        .on_click(move |_, _, cx| store.update(cx, |store, cx| store.arrange(direction, cx)));
        let tabs = Tab::ALL.map(|tab| {
            let count = if tab == Tab::Questions { waiting } else { 0 };
            let label = ui::say(cx, tab.label());
            let label = if count > 0 {
                format!("{label} {count}")
            } else {
                label.to_owned()
            };
            rail::button(tab.id(), tab.glyph(), label, BUTTON, &c)
                .children((count > 0).then(|| rail::badge(count, &c)))
                .on_click(cx.listener(move |dock, _, _, cx| {
                    dock.choose_tab(tab, cx);
                    dock.store
                        .update(cx, |store, cx| store.set_pane_open("dock", true, cx));
                }))
        });
        rail::column(&c)
            .child(open)
            .child(rail::divider(&c))
            .child(arrange)
            .child(rail::divider(&c))
            .children(tabs)
    }
}
