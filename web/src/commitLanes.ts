// 커밋 그래프의 차선 계산. VS Code에서 옮겼다(출처·라이선스는 CommitGraph.tsx 머리).
import type { Commit } from './gen/Commit';

/** 차선 색 토큰 수(`--lane-1`~`--lane-5`). VS Code 기본 색 수와 같다. */
const LANE_COLORS = 5;

export type Lane = { id: string; color: number };
export type Row = { commit: Commit; input: Lane[]; output: Lane[] };
/** 마지막 줄 아래로 이어지는 차선과 마지막에 쓴 색. 다음 구간의 차선 계산이 여기서 잇는다. */
export type LaneState = { open: Lane[]; color: number };

export const FIRST_LANES: LaneState = { open: [], color: -1 };

/** `commits`는 자식이 부모보다 먼저 온다. 줄마다 위에서 들어오는 차선과 아래로 나가는 차선을 정한다. */
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
