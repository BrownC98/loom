use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use api::ids::WaitId;
use gpui::{
    AppContext, Context, Div, Entity, Image, InteractiveElement, MouseButton, ObjectFit,
    ParentElement, StatefulInteractiveElement, Styled, StyledImage, div, img, px,
};

use super::ui;
use crate::store::{SentPart, Store};
use crate::strings::Str;
use crate::views::tooltip::tip;
use crate::views::viewer::Viewer;

const SHOT_HEIGHT: f32 = 44.0;
const SHOT_WIDTH: f32 = 64.0;

type Key = (u32, SentPart, usize);

pub(crate) struct Shots {
    store: Entity<Store>,
    images: HashMap<Key, Arc<Image>>,
    loading: HashSet<Key>,
    failed: HashSet<Key>,
    viewer: Entity<Viewer>,
    zoomed: Option<(u32, SentPart)>,
}

impl Shots {
    pub(crate) fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        Self {
            store,
            images: HashMap::new(),
            loading: HashSet::new(),
            failed: HashSet::new(),
            viewer: cx.new(Viewer::new),
            zoomed: None,
        }
    }

    fn load(&mut self, key: Key, cx: &mut Context<Self>) {
        if self.images.contains_key(&key) || self.failed.contains(&key) || !self.loading.insert(key)
        {
            return;
        }
        let (wait, part, index) = key;
        let task = self.store.update(cx, |store, cx| {
            store.wait_image(WaitId(wait), part, index, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |shots, cx| {
                shots.loading.remove(&key);
                match result {
                    Ok(image) => {
                        shots.images.insert(key, image);
                    }
                    Err(_) => {
                        shots.failed.insert(key);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn strip<T: 'static>(
        this: &Entity<Self>,
        wait: WaitId,
        part: SentPart,
        names: &[String],
        cx: &mut Context<T>,
    ) -> Option<Div> {
        if names.is_empty() {
            return None;
        }
        let c = ui::colors(cx);
        let shots: Vec<(usize, Arc<Image>)> = this.update(cx, |shots, cx| {
            (0..names.len())
                .filter_map(|index| {
                    let key = (wait.0, part, index);
                    shots.load(key, cx);
                    Some((index, shots.images.get(&key)?.clone()))
                })
                .collect()
        });
        let viewer = {
            let state = this.read(cx);
            (state.zoomed == Some((wait.0, part)) && state.viewer.read(cx).is_open())
                .then(|| state.viewer.clone())
        };
        let seed = (wait.0 as usize * 2 + part as usize) * 256;
        Some(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.0))
                .pt(px(4.0))
                .children(shots.into_iter().map(|(index, image)| {
                    let owner = this.clone();
                    let opened = image.clone();
                    div()
                        .id(("sent-shot", seed + index % 256))
                        .cursor_pointer()
                        .tooltip(tip(ui::say_with(
                            cx,
                            Str::InboxZoomImage,
                            &[("name", &names[index])],
                        )))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            let opened = opened.clone();
                            owner.update(cx, |shots, cx| {
                                shots.zoomed = Some((wait.0, part));
                                shots
                                    .viewer
                                    .update(cx, |viewer, cx| viewer.open(opened, window, cx));
                            })
                        })
                        .child(
                            img(image)
                                .w(px(SHOT_WIDTH))
                                .h(px(SHOT_HEIGHT))
                                .object_fit(ObjectFit::Cover)
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(c.line_strong)
                                .bg(c.panel),
                        )
                }))
                .children(viewer),
        )
    }
}
