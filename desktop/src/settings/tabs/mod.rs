mod about;
mod accounts;
mod agents;
mod display;
mod general;
mod graph;
mod phone;
mod terminal;

pub use agents::parts::{keep_focus, modal};

use gpui::{App, Window};

use super::nav::Tab;
use super::{Ctx, Section};

#[derive(Default)]
pub struct State {
    general: general::State,
    phone: phone::State,
    agents: agents::State,
    accounts: accounts::State,
    display: display::State,
    graph: graph::State,
    terminal: terminal::State,
    about: about::State,
}

pub fn sections(
    tab: Tab,
    state: &mut State,
    ctx: &Ctx,
    window: &mut Window,
    cx: &mut App,
) -> Vec<Section> {
    match tab {
        Tab::General => general::sections(&mut state.general, ctx, window, cx),
        Tab::Phone => phone::sections(&mut state.phone, ctx, window, cx),
        Tab::Agents => agents::sections(&mut state.agents, ctx, window, cx),
        Tab::Accounts => accounts::sections(&mut state.accounts, ctx, window, cx),
        Tab::Display => display::sections(&mut state.display, ctx, window, cx),
        Tab::Graph => graph::sections(&mut state.graph, ctx, window, cx),
        Tab::Terminal => terminal::sections(&mut state.terminal, ctx, window, cx),
        Tab::About => about::sections(&mut state.about, ctx, window, cx),
    }
}
