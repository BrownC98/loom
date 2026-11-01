use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{Bounds, Div, ParentElement, Pixels, Styled, canvas, div};

#[derive(Clone, Default)]
pub struct Anchors(Rc<RefCell<HashMap<String, Bounds<Pixels>>>>);

impl Anchors {
    pub fn mark(&self, key: String) -> Div {
        let slots = self.0.clone();
        div().absolute().size_full().child(
            canvas(
                move |bounds, _, _| {
                    slots.borrow_mut().insert(key.clone(), bounds);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
    }

    pub fn get(&self, key: &str) -> Option<Bounds<Pixels>> {
        self.0.borrow().get(key).copied()
    }
}
