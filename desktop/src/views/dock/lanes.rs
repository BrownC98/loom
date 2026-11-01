use api::canvas::Commit;

pub const LANE_COLORS: i32 = 5;

#[derive(Clone, PartialEq, Debug)]
pub struct Lane {
    pub id: String,
    pub color: i32,
}

pub struct Row {
    pub commit: Commit,
    pub input: Vec<Lane>,
    pub output: Vec<Lane>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct LaneState {
    pub open: Vec<Lane>,
    pub color: i32,
}

impl Default for LaneState {
    fn default() -> LaneState {
        LaneState {
            open: Vec::new(),
            color: -1,
        }
    }
}

pub fn lanes(commits: Vec<Commit>, from: &LaneState) -> (Vec<Row>, LaneState) {
    let mut color = from.color;
    let mut prev = from.open.clone();
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let input = prev;
        let mut output = Vec::new();
        let mut first_parent_added = false;
        for lane in &input {
            if lane.id == commit.hash {
                if !first_parent_added && !commit.parents.is_empty() {
                    output.push(Lane {
                        id: commit.parents[0].clone(),
                        color: lane.color,
                    });
                    first_parent_added = true;
                }
                continue;
            }
            output.push(lane.clone());
        }
        for parent in commit.parents.iter().skip(usize::from(first_parent_added)) {
            color = (color + 1) % LANE_COLORS;
            output.push(Lane {
                id: parent.clone(),
                color,
            });
        }
        prev = output.clone();
        rows.push(Row {
            commit,
            input,
            output,
        });
    }
    (rows, LaneState { open: prev, color })
}

pub struct Circle {
    pub lane: usize,
    pub color: i32,
}

pub fn circle_of(row: &Row) -> Circle {
    let lane = row
        .input
        .iter()
        .position(|l| l.id == row.commit.hash)
        .unwrap_or(row.input.len());
    let color = match (row.output.get(lane), row.input.get(lane)) {
        (Some(out), _) => out.color,
        (None, Some(inp)) => inp.color,
        (None, None) => 0,
    };
    Circle { lane, color }
}

pub fn lane_count(row: &Row) -> usize {
    row.input.len().max(row.output.len()).max(1)
}
