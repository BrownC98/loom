use gpui::{App, Window};

use crate::settings::{Ctx, Section};

#[derive(Default)]
pub struct State;

pub fn sections(_: &mut State, _: &Ctx, _: &mut Window, _: &mut App) -> Vec<Section> {
    Vec::new()
}
