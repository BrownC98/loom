use api::canvas::Commit;
use serde::Deserialize;

use crate::lanes::{Lane, LaneState, circle_of, lane_count, lanes};

#[derive(Deserialize, Debug, PartialEq, Clone)]
pub struct LaneIn {
    id: String,
    color: i32,
}

#[derive(Deserialize)]
pub struct CommitIn {
    hash: String,
    parents: Vec<String>,
}

#[derive(Deserialize, Debug, PartialEq, Clone)]
pub struct StateIo {
    open: Vec<LaneIn>,
    color: i32,
}

#[derive(Deserialize)]
pub struct LanesIn {
    commits: Vec<CommitIn>,
    from: StateIo,
}

#[derive(Deserialize, Debug, PartialEq)]
struct CircleOut {
    lane: usize,
    color: i32,
}

#[derive(Deserialize, Debug, PartialEq)]
struct RowOut {
    hash: String,
    input: Vec<LaneIn>,
    output: Vec<LaneIn>,
    circle: CircleOut,
    count: usize,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct LanesOut {
    rows: Vec<RowOut>,
    end: StateIo,
}

fn lane_in(l: &LaneIn) -> Lane {
    Lane {
        id: l.id.clone(),
        color: l.color,
    }
}

fn lane_out(l: &Lane) -> LaneIn {
    LaneIn {
        id: l.id.clone(),
        color: l.color,
    }
}

pub fn commit_lanes(i: &LanesIn) -> LanesOut {
    let commits = i
        .commits
        .iter()
        .map(|c| Commit {
            hash: c.hash.clone(),
            parents: c.parents.clone(),
            subject: String::new(),
            author: String::new(),
            time: 0,
            refs: Vec::new(),
        })
        .collect();
    let from = LaneState {
        open: i.from.open.iter().map(lane_in).collect(),
        color: i.from.color,
    };
    let (rows, end) = lanes(commits, &from);
    LanesOut {
        rows: rows
            .iter()
            .map(|row| {
                let circle = circle_of(row);
                RowOut {
                    hash: row.commit.hash.clone(),
                    input: row.input.iter().map(lane_out).collect(),
                    output: row.output.iter().map(lane_out).collect(),
                    circle: CircleOut {
                        lane: circle.lane,
                        color: circle.color,
                    },
                    count: lane_count(row),
                }
            })
            .collect(),
        end: StateIo {
            open: end.open.iter().map(lane_out).collect(),
            color: end.color,
        },
    }
}
