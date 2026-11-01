use api::profile::Provider;
use gpui::{IntoElement, Rgba, Styled, canvas, point, px, rgb};

use crate::theme::Colors;

const MARK: f32 = 14.0;
const CLAUDE_ORANGE: u32 = 0xd97757;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Brand {
    Claude,
    Codex,
    Cursor,
}

impl Brand {
    pub(super) fn of(provider: Provider) -> Brand {
        match provider {
            Provider::Claude => Brand::Claude,
            Provider::Codex => Brand::Codex,
        }
    }
}

type Poly = Vec<(f32, f32)>;

fn ring(x: f32, y: f32, r: f32, steps: usize) -> Poly {
    (0..=steps)
        .map(|i| {
            let angle =
                i as f32 / steps as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            (x + r * angle.cos(), y + r * angle.sin())
        })
        .collect()
}

fn shapes(brand: Brand) -> Vec<Poly> {
    match brand {
        Brand::Claude => (0..8)
            .map(|k| {
                let angle = k as f32 * std::f32::consts::TAU / 8.0;
                vec![
                    (12.0 + 3.5 * angle.cos(), 12.0 + 3.5 * angle.sin()),
                    (12.0 + 10.0 * angle.cos(), 12.0 + 10.0 * angle.sin()),
                ]
            })
            .collect(),
        Brand::Codex => {
            let outer = ring(12.0, 12.0, 9.0, 6);
            let inner = ring(12.0, 12.0, 4.5, 6);
            vec![outer, inner]
        }
        Brand::Cursor => {
            let hex = ring(12.0, 12.0, 10.0, 6);
            vec![
                hex.clone(),
                vec![hex[0], (12.0, 12.0), hex[2]],
                vec![(12.0, 12.0), hex[4]],
            ]
        }
    }
}

fn draw(size: f32, width: f32, color: Rgba, lines: Vec<Poly>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let scale = size / 24.0;
            for poly in &lines {
                let mut line = gpui::PathBuilder::stroke(px(width * scale));
                for (i, (x, y)) in poly.iter().enumerate() {
                    let at = point(
                        bounds.origin.x + px(x * scale),
                        bounds.origin.y + px(y * scale),
                    );
                    if i == 0 {
                        line.move_to(at);
                    } else {
                        line.line_to(at);
                    }
                }
                if let Ok(path) = line.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .size(px(size))
    .flex_none()
}

pub(super) fn mark(brand: Brand, c: &Colors) -> impl IntoElement {
    let color = match brand {
        Brand::Claude => rgb(CLAUDE_ORANGE),
        Brand::Codex | Brand::Cursor => c.ink,
    };
    draw(MARK, 2.4, color, shapes(brand))
}

pub(super) fn chevron(up: bool, c: &Colors) -> impl IntoElement {
    let (near, far) = if up { (15.0, 9.0) } else { (9.0, 15.0) };
    draw(
        14.0,
        2.0,
        c.ink_3,
        vec![vec![(6.0, near), (12.0, far), (18.0, near)]],
    )
}

pub(super) fn refresh_glyph(c: &Colors) -> impl IntoElement {
    let mut arc = ring(12.0, 12.0, 8.0, 18);
    arc.truncate(15);
    let tip = arc[arc.len() - 1];
    draw(
        14.0,
        2.0,
        c.ink_2,
        vec![
            arc,
            vec![(tip.0 + 4.0, tip.1 - 1.0), tip, (tip.0 + 0.5, tip.1 + 4.5)],
        ],
    )
}
