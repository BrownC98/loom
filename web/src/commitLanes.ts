import type { Commit } from './gen/Commit';

const LANE_COLORS = 5;

export type Lane = { id: string; color: number };
export type Row = { commit: Commit; input: Lane[]; output: Lane[] };
export type LaneState = { open: Lane[]; color: number };

export const FIRST_LANES: LaneState = { open: [], color: -1 };

export function lanes(commits: Commit[], from: LaneState): { rows: Row[]; end: LaneState } {
  let { color, open: prev } = from;
  const rows = commits.map((commit) => {
    const input = prev;
    const output: Lane[] = [];
    let firstParentAdded = false;
    for (const lane of input) {
      if (lane.id === commit.hash) {
        if (!firstParentAdded && commit.parents.length > 0) {
          output.push({ id: commit.parents[0], color: lane.color });
          firstParentAdded = true;
        }
        continue;
      }
      output.push(lane);
    }
    for (let i = firstParentAdded ? 1 : 0; i < commit.parents.length; i++) {
      color = (color + 1) % LANE_COLORS;
      output.push({ id: commit.parents[i], color });
    }
    prev = output;
    return { commit, input, output };
  });
  return { rows, end: { open: prev, color } };
}
