use std::f32::consts::FRAC_PI_2;

use super::lanes::{Row, circle_of};

pub const LANE_WIDTH: f32 = 11.0;
pub const ROW_HEIGHT: f32 = 22.0;
pub const RING: f32 = 1.0;
const CIRCLE_RADIUS: f32 = 4.0;
const CURVE_RADIUS: f32 = 5.0;
const ARC_STEPS: usize = 8;

type At = (f32, f32);

pub struct Line {
    pub color: i32,
    pub points: Vec<At>,
}

pub struct Drawing {
    pub lines: Vec<Line>,
    pub lane: usize,
    pub color: i32,
    pub merge: bool,
}

pub fn lane_x(lane: usize) -> f32 {
    LANE_WIDTH * (lane as f32 + 1.0)
}

pub fn dot_radii(merge: bool) -> Vec<f32> {
    if merge {
        vec![CIRCLE_RADIUS + 2.0, CIRCLE_RADIUS - 1.0]
    } else {
        vec![CIRCLE_RADIUS + 1.0]
    }
}

pub fn width(lanes: usize) -> f32 {
    LANE_WIDTH * (lanes as f32 + 1.0)
}

fn arc(from: At, to: At, sweep: bool) -> Vec<At> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let center = if (dx * dy > 0.0) == sweep {
        (from.0, to.1)
    } else {
        (to.0, from.1)
    };
    let start = (from.1 - center.1).atan2(from.0 - center.0);
    let turn = if sweep { FRAC_PI_2 } else { -FRAC_PI_2 };
    let radius = dx.abs();
    (1..=ARC_STEPS)
        .map(|step| {
            let angle = start + turn * step as f32 / ARC_STEPS as f32;
            (
                center.0 + radius * angle.cos(),
                center.1 + radius * angle.sin(),
            )
        })
        .collect()
}

fn joined(parts: Vec<Vec<At>>) -> Vec<At> {
    parts.into_iter().flatten().collect()
}

pub fn drawing(row: &Row) -> Drawing {
    let hash = &row.commit.hash;
    let circle = circle_of(row);
    let half = ROW_HEIGHT / 2.0;
    let mut lines = Vec::new();
    let mut output_index = 0;
    for (i, lane) in row.input.iter().enumerate() {
        if &lane.id == hash {
            if i != circle.lane {
                let start = (lane_x(i), 0.0);
                let end = (LANE_WIDTH * i as f32, LANE_WIDTH);
                lines.push(Line {
                    color: lane.color,
                    points: joined(vec![
                        vec![start],
                        arc(start, end, true),
                        vec![(lane_x(circle.lane), LANE_WIDTH)],
                    ]),
                });
            } else {
                output_index += 1;
            }
            continue;
        }
        if row
            .output
            .get(output_index)
            .is_some_and(|o| o.id == lane.id)
        {
            let (from, to) = (lane_x(i), lane_x(output_index));
            let points = if i == output_index {
                vec![(from, 0.0), (from, ROW_HEIGHT)]
            } else {
                let left = (from, 6.0);
                let turn_in = (from - CURVE_RADIUS, half);
                let turn_out = (to + CURVE_RADIUS, half);
                let down = (to, half + CURVE_RADIUS);
                joined(vec![
                    vec![(from, 0.0), left],
                    arc(left, turn_in, true),
                    vec![turn_out],
                    arc(turn_out, down, false),
                    vec![(to, ROW_HEIGHT)],
                ])
            };
            lines.push(Line {
                color: lane.color,
                points,
            });
            output_index += 1;
        }
    }
    for parent in row.commit.parents.iter().skip(1) {
        let Some(j) = row.output.iter().rposition(|l| &l.id == parent) else {
            continue;
        };
        let color = row.output[j].color;
        let start = (LANE_WIDTH * j as f32, half);
        lines.push(Line {
            color,
            points: joined(vec![vec![start], arc(start, (lane_x(j), ROW_HEIGHT), true)]),
        });
        lines.push(Line {
            color,
            points: vec![start, (lane_x(circle.lane), half)],
        });
    }
    if let Some(lane) = row.input.iter().find(|l| &l.id == hash) {
        lines.push(Line {
            color: lane.color,
            points: vec![(lane_x(circle.lane), 0.0), (lane_x(circle.lane), half)],
        });
    }
    if !row.commit.parents.is_empty() {
        lines.push(Line {
            color: circle.color,
            points: vec![
                (lane_x(circle.lane), half),
                (lane_x(circle.lane), ROW_HEIGHT),
            ],
        });
    }
    Drawing {
        lines,
        lane: circle.lane,
        color: circle.color,
        merge: row.commit.parents.len() > 1,
    }
}
