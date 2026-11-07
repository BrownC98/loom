use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::dialog::{Ask, Dialog};
use super::widgets::{ghost, tabbable};
use crate::strings::Str;
use crate::views::dock::ui::{TEXT, colors, say};
use crate::views::icons::{Lucide, icon};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    General,
    Phone,
    Agents,
    Accounts,
    Display,
    Graph,
    Terminal,
    About,
}

impl Tab {
    pub const ALL: [Tab; 8] = [
        Tab::General,
        Tab::Phone,
        Tab::Agents,
        Tab::Accounts,
        Tab::Display,
        Tab::Graph,
        Tab::Terminal,
        Tab::About,
    ];

    pub fn label(self) -> Str {
        match self {
            Tab::General => Str::SettingsTabGeneral,
            Tab::Phone => Str::SettingsTabPhone,
            Tab::Agents => Str::SettingsTabAgents,
            Tab::Accounts => Str::SettingsTabAccounts,
            Tab::Display => Str::SettingsTabDisplay,
            Tab::Graph => Str::SettingsTabGraph,
            Tab::Terminal => Str::SettingsTabTerminal,
            Tab::About => Str::SettingsTabAbout,
        }
    }

    pub fn keys(self) -> Vec<&'static str> {
        OWNERS
            .iter()
            .filter(|(_, owner)| *owner == Some(self))
            .map(|(key, _)| *key)
            .collect()
    }
}

pub fn resettable() -> Vec<&'static str> {
    OWNERS
        .iter()
        .filter(|(_, owner)| owner.is_some())
        .map(|(key, _)| *key)
        .collect()
}

pub const OWNERS: [(&str, Option<Tab>); 32] = [
    ("workspaceDir", Some(Tab::General)),
    ("wslWorkspaceDirs", Some(Tab::General)),
    ("osNotify", Some(Tab::General)),
    ("bypassOff", Some(Tab::Agents)),
    ("alwaysOn", Some(Tab::Agents)),
    ("roles", Some(Tab::Agents)),
    ("treeLimits", Some(Tab::Agents)),
    ("autoSwitch", Some(Tab::Accounts)),
    ("accounts", None),
    ("wslAccounts", None),
    ("theme", Some(Tab::Display)),
    ("colors", Some(Tab::Display)),
    ("language", Some(Tab::Display)),
    ("cardView", Some(Tab::Graph)),
    ("graphScope", Some(Tab::Graph)),
    ("edgeStyle", Some(Tab::Graph)),
    ("questionPill", None),
    ("composerOff", Some(Tab::Graph)),
    ("pillPlace", None),
    ("composerPlace", Some(Tab::Graph)),
    ("pillSize", None),
    ("composerSize", Some(Tab::Graph)),
    ("terminalTheme", Some(Tab::Terminal)),
    ("terminalFont", Some(Tab::Terminal)),
    ("arrangeDirection", None),
    ("alwaysArrange", None),
    ("minimapFolded", None),
    ("repo", None),
    ("paneWidths", None),
    ("closedPanes", None),
    ("closedSwitchNotice", None),
    ("closedPill", None),
];

pub(super) fn column(
    dialog: &Dialog,
    width: f32,
    window: &mut Window,
    cx: &mut Context<Dialog>,
) -> impl IntoElement {
    let c = colors(cx);
    let searching = !dialog.query(cx).is_empty();
    let tabs = Tab::ALL.iter().enumerate().map(|(index, &tab)| {
        let current = !searching && dialog.tab == tab;
        tabbable(div().id(("settings-tab", index)), cx)
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(6.0))
            .text_size(px(14.0))
            .line_height(px(21.0))
            .text_color(if current { c.ink } else { c.ink_2 })
            .font_weight(if current {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .bg(if current { c.accent_wash } else { c.panel })
            .cursor_pointer()
            .hover(move |style| style.bg(c.surface_hover))
            .child(say(cx, tab.label()))
            .on_click(cx.listener(move |this, _, _, cx| this.choose(tab, cx)))
    });
    let clear = (!dialog.query(cx).is_empty()).then(|| {
        tabbable(div().id("settings-search-clear"), cx)
            .flex_none()
            .size(px(20.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .text_color(c.ink_3)
            .cursor_pointer()
            .hover(move |style| style.bg(c.line).text_color(c.ink))
            .child(icon(Lucide::X, 12.0, c.ink_3))
            .on_click(cx.listener(|this, _, _, cx| {
                this.search.update(cx, |field, cx| field.clear(cx));
            }))
    });
    let finder = div()
        .h(px(30.0))
        .mb(px(10.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(if dialog.search.read(cx).focused(window) {
            c.accent
        } else {
            c.line_strong
        })
        .bg(c.surface)
        .text_size(px(TEXT))
        .child(icon(Lucide::Search, 14.0, c.ink_3))
        .child(dialog.search.clone())
        .children(clear);
    div()
        .w(px(width))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .px(px(8.0))
        .py(px(16.0))
        .border_r_1()
        .border_color(c.line)
        .child(
            div()
                .px(px(8.0))
                .pb(px(10.0))
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(say(cx, Str::SettingsTitle)),
        )
        .child(finder)
        .children(tabs)
        .child(div().flex_1())
        .child(
            nav_button("settings-reset-all", Lucide::RotateCcw, cx)
                .child(say(cx, Str::SettingsResetAll))
                .on_click(cx.listener(|this, _, window, cx| this.ask(Ask::ResetAll, window, cx))),
        )
        .child(
            nav_button("settings-stop", Lucide::Power, cx)
                .child(say(cx, Str::SettingsStop))
                .on_click(cx.listener(|this, _, window, cx| this.ask(Ask::Stop, window, cx))),
        )
}

fn nav_button(id: &'static str, glyph: Lucide, cx: &gpui::App) -> gpui::Stateful<gpui::Div> {
    let c = colors(cx);
    ghost(id, cx)
        .min_h(px(30.0))
        .px(px(12.0))
        .text_size(px(TEXT))
        .font_weight(FontWeight::MEDIUM)
        .child(icon(glyph, 14.0, c.ink_2))
}
