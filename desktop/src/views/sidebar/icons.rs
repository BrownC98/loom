use gpui::{IntoElement, PathBuilder, Rgba, Styled, canvas, point, px};

const GRID: f32 = 24.0;
const STROKE: f32 = 2.0;
const ARC_STEPS: usize = 20;

type Poly = Vec<(f32, f32)>;

pub enum Glyph {
    ChevronOpen,
    ChevronClosed,
    Search,
    Clear,
    Branch,
    Bookmark { filled: bool },
    Mail,
    Reply,
    ExpandAll,
    Folder,
    Map { folded: bool },
    Gear,
    Arrange,
    Monitor,
    Sun,
    Moon,
    Image,
    Paperclip,
    ArrowUp,
    SquareTerminal,
}

fn circle(x: f32, y: f32, r: f32) -> Poly {
    (0..=ARC_STEPS)
        .map(|i| {
            let angle = i as f32 / ARC_STEPS as f32 * std::f32::consts::TAU;
            (x + r * angle.cos(), y + r * angle.sin())
        })
        .collect()
}

fn arc(x: f32, y: f32, r: f32, from: f32, to: f32) -> Poly {
    (0..=ARC_STEPS)
        .map(|i| {
            let angle = (from + (to - from) * i as f32 / ARC_STEPS as f32).to_radians();
            (x + r * angle.cos(), y + r * angle.sin())
        })
        .collect()
}

fn moon() -> Poly {
    let mut outline = arc(12.0, 12.0, 9.0, 3.1, 266.9);
    outline.extend(arc(17.005, 6.994, 6.0, 212.0, 58.0));
    outline.push(outline[0]);
    outline
}

fn paperclip() -> Poly {
    let mut line = vec![(21.44, 11.05), (12.25, 20.24)];
    line.extend(arc(8.0, 16.0, 6.0, 45.0, 225.0));
    line.push((12.33, 3.18));
    line.extend(arc(15.17, 6.01, 4.0, 225.0, 405.0));
    line.push((9.41, 17.41));
    line.extend(arc(8.0, 16.0, 2.0, 45.0, 225.0));
    line.push((15.07, 6.10));
    line
}

fn gear() -> Poly {
    let tooth = |k: usize| {
        let base = k as f32 * std::f32::consts::TAU / 8.0;
        [(-10.0, 7.0), (-6.0, 10.0), (6.0, 10.0), (10.0, 7.0)].map(move |(degrees, radius)| {
            let angle = base + f32::to_radians(degrees);
            (12.0 + radius * angle.cos(), 12.0 + radius * angle.sin())
        })
    };
    let mut ring: Poly = (0..8).flat_map(tooth).collect();
    ring.push(ring[0]);
    ring
}

fn strokes(glyph: &Glyph) -> Vec<Poly> {
    match glyph {
        Glyph::ChevronOpen => vec![vec![(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)]],
        Glyph::ChevronClosed => vec![vec![(9.0, 6.0), (15.0, 12.0), (9.0, 18.0)]],
        Glyph::Search => vec![circle(11.0, 11.0, 8.0), vec![(21.0, 21.0), (16.7, 16.7)]],
        Glyph::Clear => vec![
            vec![(18.0, 6.0), (6.0, 18.0)],
            vec![(6.0, 6.0), (18.0, 18.0)],
        ],
        Glyph::Branch => vec![
            vec![(6.0, 3.0), (6.0, 15.0)],
            circle(18.0, 6.0, 3.0),
            circle(6.0, 18.0, 3.0),
            vec![(18.0, 9.0), (17.0, 13.0), (14.0, 16.5), (9.0, 18.0)],
        ],
        Glyph::Bookmark { .. } => vec![vec![
            (5.0, 3.0),
            (19.0, 3.0),
            (19.0, 21.0),
            (12.0, 16.5),
            (5.0, 21.0),
            (5.0, 3.0),
        ]],
        Glyph::Mail => vec![
            vec![
                (3.0, 5.0),
                (21.0, 5.0),
                (21.0, 19.0),
                (3.0, 19.0),
                (3.0, 5.0),
            ],
            vec![(3.0, 7.0), (12.0, 13.0), (21.0, 7.0)],
        ],
        Glyph::Reply => vec![
            vec![(9.0, 14.0), (4.0, 9.0), (9.0, 4.0)],
            vec![(20.0, 20.0), (20.0, 13.0), (17.0, 9.0), (4.0, 9.0)],
        ],
        Glyph::ExpandAll => vec![
            vec![(7.0, 15.0), (12.0, 20.0), (17.0, 15.0)],
            vec![(7.0, 9.0), (12.0, 4.0), (17.0, 9.0)],
        ],
        Glyph::Gear => vec![gear(), circle(12.0, 12.0, 3.0)],
        Glyph::Folder => vec![vec![
            (20.0, 20.0),
            (22.0, 18.0),
            (22.0, 8.0),
            (20.0, 6.0),
            (12.1, 6.0),
            (10.41, 5.1),
            (9.6, 3.9),
            (7.93, 3.0),
            (4.0, 3.0),
            (2.0, 5.0),
            (2.0, 18.0),
            (4.0, 20.0),
            (20.0, 20.0),
        ]],
        Glyph::Arrange => vec![
            vec![
                (3.0, 3.0),
                (11.0, 3.0),
                (11.0, 11.0),
                (3.0, 11.0),
                (3.0, 3.0),
            ],
            vec![(7.0, 11.0), (7.0, 15.0), (9.0, 17.0), (13.0, 17.0)],
            vec![
                (13.0, 13.0),
                (21.0, 13.0),
                (21.0, 21.0),
                (13.0, 21.0),
                (13.0, 13.0),
            ],
        ],
        Glyph::Monitor => vec![
            vec![
                (2.0, 3.0),
                (22.0, 3.0),
                (22.0, 17.0),
                (2.0, 17.0),
                (2.0, 3.0),
            ],
            vec![(8.0, 21.0), (16.0, 21.0)],
            vec![(12.0, 17.0), (12.0, 21.0)],
        ],
        Glyph::Sun => vec![
            circle(12.0, 12.0, 4.0),
            vec![(12.0, 2.0), (12.0, 4.0)],
            vec![(12.0, 20.0), (12.0, 22.0)],
            vec![(4.93, 4.93), (6.34, 6.34)],
            vec![(17.66, 17.66), (19.07, 19.07)],
            vec![(2.0, 12.0), (4.0, 12.0)],
            vec![(20.0, 12.0), (22.0, 12.0)],
            vec![(6.34, 17.66), (4.93, 19.07)],
            vec![(19.07, 4.93), (17.66, 6.34)],
        ],
        Glyph::Moon => vec![moon()],
        Glyph::Image => vec![
            vec![
                (3.0, 3.0),
                (21.0, 3.0),
                (21.0, 21.0),
                (3.0, 21.0),
                (3.0, 3.0),
            ],
            circle(9.0, 9.0, 2.0),
            vec![
                (21.0, 15.0),
                (17.9, 11.9),
                (16.0, 11.2),
                (14.5, 11.5),
                (6.0, 21.0),
            ],
        ],
        Glyph::Paperclip => vec![paperclip()],
        Glyph::ArrowUp => vec![
            vec![(5.0, 12.0), (12.0, 5.0), (19.0, 12.0)],
            vec![(12.0, 19.0), (12.0, 5.0)],
        ],
        Glyph::SquareTerminal => vec![
            vec![
                (3.0, 3.0),
                (21.0, 3.0),
                (21.0, 21.0),
                (3.0, 21.0),
                (3.0, 3.0),
            ],
            vec![(7.0, 11.0), (9.0, 9.0), (7.0, 7.0)],
            vec![(11.0, 13.0), (15.0, 13.0)],
        ],
        Glyph::Map { folded } => {
            let mut lines = vec![
                vec![
                    (3.0, 6.6),
                    (9.0, 3.2),
                    (15.0, 5.8),
                    (21.0, 4.6),
                    (21.0, 17.4),
                    (15.0, 20.8),
                    (9.0, 18.2),
                    (3.0, 19.4),
                    (3.0, 6.6),
                ],
                vec![(15.0, 5.8), (15.0, 20.8)],
                vec![(9.0, 3.2), (9.0, 18.2)],
                vec![(10.5, 12.0), (13.5, 12.0)],
            ];
            if *folded {
                lines.push(vec![(12.0, 10.5), (12.0, 13.5)]);
            }
            lines
        }
    }
}

pub fn icon(glyph: Glyph, size: f32, color: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let scale = size / GRID;
            let at = |(x, y): (f32, f32)| {
                point(
                    bounds.origin.x + px(x * scale),
                    bounds.origin.y + px(y * scale),
                )
            };
            if let Glyph::Bookmark { filled: true } = glyph {
                let mut fill = PathBuilder::fill();
                for (i, p) in strokes(&glyph)[0].iter().enumerate() {
                    if i == 0 {
                        fill.move_to(at(*p));
                    } else {
                        fill.line_to(at(*p));
                    }
                }
                fill.close();
                if let Ok(path) = fill.build() {
                    window.paint_path(path, color);
                }
            }
            for poly in strokes(&glyph) {
                let mut line = PathBuilder::stroke(px(STROKE * scale.max(0.6)));
                for (i, p) in poly.iter().enumerate() {
                    if i == 0 {
                        line.move_to(at(*p));
                    } else {
                        line.line_to(at(*p));
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
