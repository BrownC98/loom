//! One rule for IME composition: while the focused input holds marked text,
//! no background update moves focus away or stops painting that input.
//! The platform commits a half-built syllable (loose jamo) when focus moves
//! or when a frame has no input handler for the focused element.

use std::time::Duration;

use gpui::{App, Context, FocusHandle, Global, WeakFocusHandle, Window};

const RETRY: Duration = Duration::from_millis(300);

#[derive(Default)]
struct Held(Option<WeakFocusHandle>);

impl Global for Held {}

/// A field or terminal calls this each time its marked text starts or ends.
pub fn mark(focus: &FocusHandle, open: bool, cx: &mut App) {
    let held = cx.default_global::<Held>();
    let same = held
        .0
        .as_ref()
        .and_then(WeakFocusHandle::upgrade)
        .is_some_and(|held| held == *focus);
    if open {
        held.0 = Some(focus.downgrade());
    } else if same {
        held.0 = None;
    }
}

/// The focused input of this window holds marked text.
pub fn busy(window: &Window, cx: &App) -> bool {
    cx.try_global::<Held>()
        .and_then(|held| held.0.as_ref()?.upgrade())
        .is_some_and(|focus| focus.is_focused(window))
}

/// Background focus move: do nothing while a composition is open.
/// Returns whether focus moved. Moves that the human asks for call `Window::focus`.
pub fn refocus(target: &FocusHandle, window: &mut Window, cx: &mut App) -> bool {
    if busy(window, cx) {
        return false;
    }
    window.focus(target, cx);
    true
}

/// Draw again soon, so a view that held back a change applies it after the composition.
pub fn retry<T: 'static>(window: &mut Window, cx: &mut Context<T>) {
    cx.spawn_in(window, async move |this, cx| {
        cx.background_executor().timer(RETRY).await;
        let _ = this.update(cx, |_, cx| cx.notify());
    })
    .detach();
}
