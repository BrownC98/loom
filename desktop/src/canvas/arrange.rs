use gpui::{
    App, Context, Div, InteractiveElement, MouseButton, MouseDownEvent, ParentElement, Styled,
    Window, div, px,
};

use super::view::CanvasView;
use crate::theme::Colors;
use crate::views::icons::{Lucide, icon};

const MIN_ZOOM: f32 = 0.3;
const BUTTON: f32 = 22.0;
const ICON: f32 = 14.0;
const REPO_TOP: f32 = 12.0;
const REPO_SIDE: f32 = 16.0;

pub fn button(
    zoom: f32,
    colors: Colors,
    down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px((BUTTON * zoom).max(1.0)))
        .rounded(px(4.0 * zoom))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, down)
        .child(icon(Lucide::Workflow, (ICON * zoom).max(1.0), colors.ink_2))
}

pub fn repo_heads(canvas: &CanvasView, cx: &Context<CanvasView>) -> Vec<Div> {
    let zoom = canvas.camera.zoom;
    if zoom < MIN_ZOOM {
        return Vec::new();
    }
    let colors = cx.global::<crate::theme::Theme>().colors;
    let width = canvas.area.get().size.width;
    canvas
        .scene
        .repos
        .iter()
        .map(|frame| {
            let key = frame.key.clone();
            let at = canvas.camera.to_screen(gpui::point(
                (frame.rect.at.x + frame.rect.size.width) as f32,
                frame.rect.at.y as f32,
            ));
            div()
                .absolute()
                .right(width - at.x + px(REPO_SIDE * zoom))
                .top(at.y + px(REPO_TOP * zoom))
                .child(button(
                    zoom,
                    colors,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.arrange_repo(&key, cx);
                        cx.stop_propagation();
                    }),
                ))
        })
        .collect()
}
