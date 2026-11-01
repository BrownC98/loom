use crate::views::widgets::btn;
mod confirm;
mod list;
mod number;
mod reset;
mod select;
mod switch;
mod text;

pub use crate::views::widgets::{Label, segmented, segmented_on};
pub use btn::{button, ghost, inert, tabbable};
pub use confirm::confirm;
pub use list::{PITCH, list_box_on};
pub use number::{Number, NumberEvent};
pub use reset::field_reset;
pub use select::{Select, SelectEvent};
pub use switch::switch;
pub use text::{Text, TextEvent};
