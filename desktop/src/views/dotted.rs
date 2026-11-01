use gpui::{
    Bounds, Div, InteractiveElement, ParentElement, Rgba, SharedString, Stateful, Styled, canvas,
    div, fill, point, px, size,
};

pub const NAME_LIFT: f32 = 2.0;

pub fn underline(
    group: impl Into<SharedString>,
    color: Rgba,
    zoom: f32,
    lift: f32,
) -> Stateful<Div> {
    let dot = px(zoom.max(1.0).round());
    div()
        .id("name-dots")
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(lift * zoom))
        .h(dot)
        .invisible()
        .group_hover(group, |style| style.visible())
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let mut x = bounds.left();
                    while x < bounds.right() {
                        let width = dot.min(bounds.right() - x);
                        window.paint_quad(fill(
                            Bounds::new(point(x, bounds.top()), size(width, dot)),
                            color,
                        ));
                        x += dot * 2.0;
                    }
                },
            )
            .size_full(),
        )
}
