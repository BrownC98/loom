use api::settings::DefaultCardView;
use gpui::{
    Context, Div, FontWeight, ParentElement, SharedString, StatefulInteractiveElement, Styled, div,
    px,
};

use super::ui;
use super::{Dock, ui::SMALL};
use crate::strings::Str;
use crate::views::icons::Lucide;
use crate::views::rail;
use crate::views::widgets::{Label, segmented_fit};

const VIEWS: [(DefaultCardView, Str); 3] = [
    (DefaultCardView::Simple, Str::CardViewSimple),
    (DefaultCardView::RootTerminal, Str::CardViewRootTerminal),
    (DefaultCardView::Standard, Str::CardViewStandard),
];

impl Dock {
    pub(super) fn tools(&self, repo: bool, cx: &mut Context<Self>) -> Div {
        let c = ui::colors(cx);
        let store = self.store.clone();
        let current = store.read(cx).settings().card_view;
        let collapse = rail::button(
            "dock-collapse",
            Lucide::PanelRightClose,
            rail::titled(ui::say(cx, Str::DockCollapse), "]"),
            rail::SMALL_BUTTON,
            &c,
        )
        .on_click({
            let store = store.clone();
            move |_, _, cx| store.update(cx, |store, cx| store.set_pane_open("dock", false, cx))
        });
        let items = VIEWS
            .iter()
            .map(|&(value, label)| (value, Label::from(SharedString::from(ui::say(cx, label)))))
            .collect();
        let switch = segmented_fit(
            "card-view",
            items,
            current,
            move |value, _, cx| {
                store.update(cx, |store, cx| {
                    store.save_settings(serde_json::json!({ "cardView": value }), cx)
                })
            },
            cx,
        );
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .pt(px(8.0))
            .px(px(12.0))
            .pb(px(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c.ink_3)
                            .child(ui::say(cx, Str::DockView)),
                    )
                    .child(collapse),
            )
            .child(switch)
            .children(repo.then(|| self.arrange_bar(cx)))
    }
}
