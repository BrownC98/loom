use std::time::Duration;

use gpui::{App, Context, FocusHandle, Global, WeakFocusHandle, Window};

const RETRY: Duration = Duration::from_millis(300);

#[derive(Default)]
struct Held(Option<WeakFocusHandle>);

impl Global for Held {}

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

pub fn busy(window: &Window, cx: &App) -> bool {
    cx.try_global::<Held>()
        .and_then(|held| held.0.as_ref()?.upgrade())
        .is_some_and(|focus| focus.is_focused(window))
}

pub fn refocus(target: &FocusHandle, window: &mut Window, cx: &mut App) -> bool {
    if busy(window, cx) {
        return false;
    }
    window.focus(target, cx);
    true
}

pub fn retry<T: 'static>(window: &mut Window, cx: &mut Context<T>) {
    cx.spawn_in(window, async move |this, cx| {
        cx.background_executor().timer(RETRY).await;
        let _ = this.update(cx, |_, cx| cx.notify());
    })
    .detach();
}
