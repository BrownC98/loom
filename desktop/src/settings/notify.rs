use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{App, Entity, SharedString, SystemNotification, Window};

use crate::store::Store;
use crate::views::dock::questions::kind_label;
use crate::views::dock::ui::say;

const IDENTITY: &str = "loom.desktop";
const NAME: &str = "loom";

pub const SUPPORTED: bool = !cfg!(target_os = "macos");

pub fn identify(cx: &App) {
    cx.set_app_identity(IDENTITY, NAME);
}

pub fn watch(store: &Entity<Store>, window: &mut Window, cx: &mut App) {
    if !SUPPORTED {
        return;
    }
    let handle = window.window_handle();
    let tags: Rc<RefCell<HashMap<SharedString, u32>>> = Rc::default();
    let target = store.downgrade();
    let known = tags.clone();
    cx.on_system_notification_response(move |response, cx| {
        let Some(node) = known.borrow().get(&response.tag).copied() else {
            return;
        };
        let _ = handle.update(cx, |_, window, _| window.activate_window());
        let _ = target.update(cx, |store, cx| store.select(node, cx));
    });
    let mut seen: Option<HashSet<u32>> = None;
    window
        .observe(store, cx, move |store, window, cx| {
            let store = store.read(cx);
            let Some(snapshot) = store.snapshot() else {
                return;
            };
            let waits: Vec<_> = snapshot
                .nodes
                .iter()
                .flat_map(|node| node.waits.iter().map(move |wait| (node, wait)))
                .collect();
            let now: HashSet<u32> = waits.iter().map(|(_, wait)| wait.wait.id.0).collect();
            let before = seen.replace(now);
            let Some(before) = before else { return };
            if !store.settings().os_notify || window.is_window_active() {
                return;
            }
            let fresh: Vec<(u32, SystemNotification)> = waits
                .into_iter()
                .filter(|(_, wait)| !before.contains(&wait.wait.id.0))
                .map(|(node, wait)| {
                    let title = format!(
                        "{} \u{00B7} {}",
                        node.name,
                        say(cx, kind_label(wait.wait.kind))
                    );
                    let notification = SystemNotification {
                        tag: SharedString::from(format!("loom-wait-{}", wait.wait.id.0)),
                        title: SharedString::from(title),
                        body: SharedString::from(wait.summary.clone()),
                        actions: Vec::new(),
                    };
                    (node.id.0, notification)
                })
                .collect();
            for (node, notification) in fresh {
                tags.borrow_mut().insert(notification.tag.clone(), node);
                cx.show_system_notification(notification);
            }
        })
        .detach();
}
