// 커밋 그래프 칸(docs/mockups/graph-panels.html 그림 B). 읽기만 한다.
//
// 차선 계산(`lanes`)과 줄 그리기(`LaneSvg`)는 VS Code에서 옮겼다.
// 출처: microsoft/vscode 25c94ab342a6b167d4b97ade0829955d4f7e094e
//       src/vs/workbench/contrib/scm/browser/scmHistory.ts
//       (toISCMHistoryItemViewModelArray, renderSCMHistoryItemGraph)
// 라이선스: MIT, Copyright (c) Microsoft Corporation.
// 바꾼 점: 부모 없는 커밋에서도 다른 차선을 이어 간다(원본은 그 줄에서 다른 차선을 버린다).
// 옮기지 않은 것: Incoming/Outgoing 줄, 참조 색 지도, 마우스 올림 설명 창.
import { useEffect, useState, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { getCommits } from './api';
import type { Commit } from './gen/Commit';
import type { CommitGraph as Graph } from './gen/CommitGraph';
import type { Worktree } from './gen/Worktree';
import i18next, { ago } from './i18n';

const LANE_WIDTH = 11;
const ROW_HEIGHT = 22;
const CURVE_RADIUS = 5;
const CIRCLE_RADIUS = 4;
/** 차선 색 토큰 수(`--lane-1`~`--lane-5`). VS Code 기본 색 수와 같다. */
const LANE_COLORS = 5;
/** 칸이 열린 동안 커밋을 다시 읽는 간격. 데몬 이벤트는 커밋을 알리지 않는다. 사람이 터미널에서 한 커밋도 보여야 한다. */
const REFRESH_MS = 3000;

type Lane = { id: string; color: number };
type Row = { commit: Commit; input: Lane[]; output: Lane[] };

/** `commits`는 자식이 부모보다 먼저 온다. 줄마다 위에서 들어오는 차선과 아래로 나가는 차선을 정한다. */
function lanes(commits: Commit[]): Row[] {
  let color = -1;
  let prev: Lane[] = [];
  return commits.map((commit) => {
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
}

const x = (lane: number) => LANE_WIDTH * (lane + 1);
const stroke = (color: number) => `var(--lane-${color + 1})`;

/** 커밋 점이 놓이는 차선과 그 색. */
function circleOf({ commit, input, output }: Row): { lane: number; color: number } {
  const inputIndex = input.findIndex((l) => l.id === commit.hash);
  const lane = inputIndex !== -1 ? inputIndex : input.length;
  const color = lane < output.length ? output[lane].color : lane < input.length ? input[lane].color : 0;
  return { lane, color };
}

function laneCount(row: Row): number {
  return Math.max(row.input.length, row.output.length, 1);
}

function LaneSvg({ row, width }: { row: Row; width: number }) {
  const { commit, input, output } = row;
  const circle = circleOf(row);
  const paths: { d: string; color: number }[] = [];
  let outputIndex = 0;
  input.forEach((lane, i) => {
    if (lane.id === commit.hash) {
      // 같은 커밋으로 모이는 다른 차선: 점 쪽으로 꺾는다.
      if (i !== circle.lane) paths.push({ color: lane.color, d: `M ${x(i)} 0 A ${LANE_WIDTH} ${LANE_WIDTH} 0 0 1 ${LANE_WIDTH * i} ${LANE_WIDTH} H ${x(circle.lane)}` });
      else outputIndex++;
      return;
    }
    if (outputIndex < output.length && lane.id === output[outputIndex].id) {
      if (i === outputIndex) paths.push({ color: lane.color, d: `M ${x(i)} 0 V ${ROW_HEIGHT}` });
      else
        paths.push({
          color: lane.color,
          d: `M ${x(i)} 0 V 6 A ${CURVE_RADIUS} ${CURVE_RADIUS} 0 0 1 ${x(i) - CURVE_RADIUS} ${ROW_HEIGHT / 2} H ${x(outputIndex) + CURVE_RADIUS} A ${CURVE_RADIUS} ${CURVE_RADIUS} 0 0 0 ${x(outputIndex)} ${ROW_HEIGHT / 2 + CURVE_RADIUS} V ${ROW_HEIGHT}`,
        });
      outputIndex++;
    }
  });
  // 병합 커밋의 둘째 부모부터: 점에서 그 부모 차선으로 갈라진다.
  for (const parent of commit.parents.slice(1)) {
    const j = output.map((l) => l.id).lastIndexOf(parent);
    if (j === -1) continue;
    paths.push({ color: output[j].color, d: `M ${LANE_WIDTH * j} ${ROW_HEIGHT / 2} A ${LANE_WIDTH} ${LANE_WIDTH} 0 0 1 ${x(j)} ${ROW_HEIGHT} M ${LANE_WIDTH * j} ${ROW_HEIGHT / 2} H ${x(circle.lane)}` });
  }
  if (input.some((l) => l.id === commit.hash)) paths.push({ color: input.find((l) => l.id === commit.hash)!.color, d: `M ${x(circle.lane)} 0 V ${ROW_HEIGHT / 2}` });
  if (commit.parents.length > 0) paths.push({ color: circle.color, d: `M ${x(circle.lane)} ${ROW_HEIGHT / 2} V ${ROW_HEIGHT}` });
  const fill = stroke(circle.color);
  return (
    <svg className="lanes" width={width} height={ROW_HEIGHT} aria-hidden="true">
      {paths.map((p, i) => (
        <path key={i} d={p.d} stroke={stroke(p.color)} />
      ))}
      {commit.parents.length > 1 ? (
        // 병합 커밋: 고리.
        <>
          <circle cx={x(circle.lane)} cy={LANE_WIDTH} r={CIRCLE_RADIUS + 2} fill={fill} />
          <circle cx={x(circle.lane)} cy={LANE_WIDTH} r={CIRCLE_RADIUS - 1} fill={fill} />
        </>
      ) : (
        <circle cx={x(circle.lane)} cy={LANE_WIDTH} r={CIRCLE_RADIUS + 1} fill={fill} />
      )}
    </svg>
  );
}

function Folder() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
    </svg>
  );
}

/** 묶음 머리의 종류 글과 같다. */
function kindLabel(worktree: Worktree): string {
  return i18next.t(worktree.original ? 'worktree.original' : 'worktree.linked');
}

function Refs({ commit, worktrees, color }: { commit: Commit; worktrees: Worktree[]; color: number }) {
  const style = { '--lane': stroke(color) } as CSSProperties;
  const here = worktrees.filter((w) => w.head === commit.hash);
  return (
    <>
      {commit.refs.map((ref) => {
        const worktree = ref.kind === 'branch' ? here.find((w) => w.branch === ref.name) : undefined;
        return (
          <span key={`${ref.kind}:${ref.name}`} className={`ref ${ref.kind}`} style={style} translate="no">
            {worktree && (
              <span title={kindLabel(worktree)}>
                <Folder />
              </span>
            )}
            {ref.name}
          </span>
        );
      })}
      {here
        .filter((w) => w.branch === null)
        .map((w, i) => (
          <span key={`head:${i}`} className="ref branch" style={style} title={kindLabel(w)} translate="no">
            <Folder />
            HEAD
          </span>
        ))}
    </>
  );
}

/** 칸이 열린 동안 붙어 있다. 닫으면 다시 읽기를 멈춘다. */
export function CommitGraph(props: { onClose: () => void }) {
  const { t } = useTranslation();
  const [graph, setGraph] = useState<Graph | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now);

  useEffect(() => {
    let last = '';
    const read = () =>
      getCommits()
        .then((next) => {
          setError(null);
          setNow(Date.now());
          // 바뀔 때만 다시 그린다.
          const text = JSON.stringify(next);
          if (text !== last) {
            last = text;
            setGraph(next);
          }
        })
        .catch((e: Error) => setError(e.message));
    read();
    const timer = window.setInterval(read, REFRESH_MS);
    return () => clearInterval(timer);
  }, []);

  const rows = graph ? lanes(graph.commits) : [];
  const width = LANE_WIDTH * (Math.max(1, ...rows.map(laneCount)) + 1);

  return (
    <aside className="dock" id="dock" aria-label={t('app.commitGraph')}>
      <div className="dock-head">
        <h2>{t('app.commitGraph')}</h2>
        <span className="hint">{t('commits.scope')}</span>
        <span className="spacer" />
        <button className="btn ghost sm" aria-label={t('commits.close')} onClick={props.onClose}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      </div>
      {error !== null ? (
        <p className="dock-note error" role="alert">
          {error}
        </p>
      ) : graph === null ? (
        <p className="dock-note">{t('commits.reading')}</p>
      ) : rows.length === 0 ? (
        <p className="dock-note">{t('commits.empty')}</p>
      ) : (
        <ol className="commits" aria-label={t('commits.list')}>
          {rows.map((row) => (
              <li key={row.commit.hash} className="commit" title={row.commit.hash}>
                <span className="graph">
                  <LaneSvg row={row} width={width} />
                </span>
                <span className="subject">
                  <Refs commit={row.commit} worktrees={graph.worktrees} color={circleOf(row).color} />
                  <span>{row.commit.subject}</span>
                </span>
                <span className="author">{row.commit.author}</span>
                <time dateTime={new Date(row.commit.time * 1000).toISOString()}>{ago(row.commit.time * 1000, now)}</time>
              </li>
          ))}
        </ol>
      )}
    </aside>
  );
}
