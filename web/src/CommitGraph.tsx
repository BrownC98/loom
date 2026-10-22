// 커밋 그래프 칸(docs/mockups/graph-panels.html 그림 B). 읽기만 한다.
//
// 차선 계산(`lanes`)과 줄 그리기(`LaneSvg`)는 VS Code에서 옮겼다.
// 출처: microsoft/vscode 25c94ab342a6b167d4b97ade0829955d4f7e094e
//       src/vs/workbench/contrib/scm/browser/scmHistory.ts
//       (toISCMHistoryItemViewModelArray, renderSCMHistoryItemGraph)
// 라이선스: MIT, Copyright (c) Microsoft Corporation.
// 바꾼 점: 부모 없는 커밋에서도 다른 차선을 이어 간다(원본은 그 줄에서 다른 차선을 버린다).
// 옮기지 않은 것: Incoming/Outgoing 줄, 참조 색 지도, 마우스 올림 설명 창.
import { Folder } from 'lucide-react';
import { memo, useEffect, useRef, useState, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { getCommits } from './api';
import { FIRST_LANES, lanes, type LaneState, type Row } from './commitLanes';
import type { Commit } from './gen/Commit';
import type { CommitGraph as Graph } from './gen/CommitGraph';
import type { RepoId } from './gen/RepoId';
import type { Worktree } from './gen/Worktree';
import { ago } from './i18n';

const LANE_WIDTH = 11;
/** `.commit`의 `height`와 같다. 보이는 줄을 고르는 데도 쓴다. */
const ROW_HEIGHT = 22;
const CURVE_RADIUS = 5;
const CIRCLE_RADIUS = 4;
/** 칸이 열린 동안 참조 지문을 다시 읽는 간격. 데몬 이벤트는 커밋을 알리지 않는다. 사람이 터미널에서 한 커밋도 보여야 한다. */
const REFRESH_MS = 3000;
/** 한 번에 읽는 커밋 수. 아래로 내려 남은 줄이 `LOAD_AHEAD`보다 적으면 다음 구간을 읽는다. */
const PAGE = 500;
const LOAD_AHEAD = 200;
/** 보이는 줄 위아래로 더 그리는 줄. 빨리 굴릴 때 빈 곳이 보이지 않게 한다. */
const OVERSCAN = 10;

/** 읽은 커밋. `version`은 그때의 참조 지문, `done`은 마지막 구간까지 읽었는지다. */
type Loaded = { version: string; worktrees: Worktree[]; rows: Row[]; end: LaneState; lanes: number; done: boolean };

/** `page`는 `limit`개를 달라고 해 받은 구간이다. `before`가 있으면 그 뒤에 잇는다. */
function loaded(page: Graph, limit: number, before?: Loaded): Loaded {
  const next = lanes(page.commits, before?.end ?? FIRST_LANES);
  return {
    version: page.version,
    worktrees: page.worktrees,
    rows: before ? [...before.rows, ...next.rows] : next.rows,
    end: next.end,
    lanes: Math.max(before?.lanes ?? 1, ...next.rows.map(laneCount)),
    done: page.commits.length < limit,
  };
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

function Refs({ commit, worktrees, color }: { commit: Commit; worktrees: Worktree[]; color: number }) {
  const { t } = useTranslation();
  // 묶음 머리의 종류 글과 같다.
  const kindLabel = (worktree: Worktree) => t(worktree.original ? 'worktree.original' : 'worktree.linked');
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

/** 줄은 커밋 목록이나 「몇 분 전」 글이 바뀔 때만 다시 그린다. 칸은 3초마다 다시 읽는다. */
const CommitRow = memo(function CommitRow({ row, width, worktrees, when }: { row: Row; width: number; worktrees: Worktree[]; when: string }) {
  return (
    <li className="commit" title={row.commit.hash}>
      <span className="graph">
        <LaneSvg row={row} width={width} />
      </span>
      <span className="subject">
        <Refs commit={row.commit} worktrees={worktrees} color={circleOf(row).color} />
        <span>{row.commit.subject}</span>
      </span>
      <span className="author">{row.commit.author}</span>
      <time dateTime={new Date(row.commit.time * 1000).toISOString()}>{when}</time>
    </li>
  );
});

/**
 * 칸이 열린 동안 붙어 있다. 닫으면 다시 읽기를 멈춘다. `name`은 칸 머리 앞에 붙이는 저장소 이름이다(목업 repo-scope.html 4번).
 * 앱은 노드 이벤트마다 다시 그린다. 이 칸은 props가 바뀔 때만 다시 그린다.
 * 보이는 줄만 그린다. 줄 수와 관계없이 칸 너비를 끄는 동안 다시 배치할 줄이 몇십 개다.
 */
export const CommitGraph = memo(function CommitGraph(props: { repo: RepoId; name: string }) {
  const { t } = useTranslation();
  const [graph, setGraph] = useState<Loaded | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now);
  const [shown, setShown] = useState({ first: 0, last: 0 });
  const list = useRef<HTMLOListElement>(null);
  const more = useRef(() => {});

  useEffect(() => {
    let current: Loaded | null = null;
    let closed = false;
    // 읽기는 하나씩 한다. 다음 구간은 앞 구간의 차선에서 잇는다.
    let busy: Promise<void> = Promise.resolve();
    // `step`이 `undefined`를 주면 다시 그리지 않는다.
    const queue = (step: () => Promise<Loaded | undefined>) =>
      (busy = busy
        .then(step)
        .then((next) => {
          if (closed) return;
          setError(null);
          setNow(Date.now());
          if (next !== undefined) setGraph((current = next));
        })
        .catch((e: Error) => {
          if (!closed) setError(e.message);
        }));
    // 참조가 바뀌면 이미 읽은 만큼을 한 번에 다시 읽는다. 사람이 보던 자리가 그대로다.
    const reload = async () => {
      const limit = Math.max(PAGE, current?.rows.length ?? 0);
      return loaded(await getCommits(props.repo, { skip: 0, limit }), limit);
    };
    const refresh = async () => {
      if (current === null) return reload();
      const head = await getCommits(props.repo, { skip: 0, limit: 0 });
      if (head.version !== current.version) return reload();
      return JSON.stringify(head.worktrees) === JSON.stringify(current.worktrees) ? undefined : { ...current, worktrees: head.worktrees };
    };
    let pending = false;
    more.current = () => {
      if (pending || current === null || current.done) return;
      pending = true;
      queue(async () => {
        pending = false;
        if (current === null || current.done) return undefined;
        const page = await getCommits(props.repo, { skip: current.rows.length, limit: PAGE });
        // 그 사이 참조가 바뀌었으면 구간이 밀렸다. 이어 붙이지 않고 다시 읽는다.
        if (page.version !== current.version) return reload();
        return loaded(page, PAGE, current);
      });
    };
    queue(refresh);
    const timer = window.setInterval(() => queue(refresh), REFRESH_MS);
    return () => {
      closed = true;
      clearInterval(timer);
    };
  }, [props.repo]);

  // 굴리거나 칸 높이가 바뀔 때 보이는 줄을 다시 고른다. 칸 너비만 바뀌면 고른 줄이 같아 다시 그리지 않는다.
  const hasRows = (graph?.rows.length ?? 0) > 0;
  useEffect(() => {
    const el = list.current;
    if (!el) return;
    const pick = () => {
      const first = Math.max(0, Math.floor(el.scrollTop / ROW_HEIGHT) - OVERSCAN);
      const last = Math.ceil((el.scrollTop + el.clientHeight) / ROW_HEIGHT) + OVERSCAN;
      setShown((s) => (s.first === first && s.last === last ? s : { first, last }));
    };
    pick();
    el.addEventListener('scroll', pick, { passive: true });
    const observer = new ResizeObserver(pick);
    observer.observe(el);
    return () => {
      el.removeEventListener('scroll', pick);
      observer.disconnect();
    };
  }, [hasRows, error === null]);

  const count = graph?.rows.length ?? 0;
  const last = Math.min(count, shown.last);
  useEffect(() => {
    if (count - last < LOAD_AHEAD) more.current();
  }, [count, last]);

  const rows = graph?.rows ?? [];
  const width = LANE_WIDTH * ((graph?.lanes ?? 1) + 1);
  const first = Math.min(shown.first, last);
  // 그리지 않은 줄의 높이는 목록 앞뒤 빈 칸(styles.css `.commits::before`·`::after`)에 둔다. 굴림 막대가 모든 줄의 길이를 보인다.
  const spacers = { '--rows-before': `${first * ROW_HEIGHT}px`, '--rows-after': `${(count - last) * ROW_HEIGHT}px` } as CSSProperties;

  return (
    <>
      <p className="dock-head hint">
        <span>
          <b translate="no">{props.name}</b> · {t('commits.scope')}
        </span>
      </p>
      {error !== null ? (
        <p className="dock-note error" role="alert">
          {error}
        </p>
      ) : graph === null ? (
        <p className="dock-note">{t('commits.reading')}</p>
      ) : rows.length === 0 ? (
        <p className="dock-note">{t('commits.empty')}</p>
      ) : (
        <ol ref={list} className="commits" aria-label={t('commits.list')} style={spacers}>
          {rows.slice(first, last).map((row) => (
            <CommitRow key={row.commit.hash} row={row} width={width} worktrees={graph.worktrees} when={ago(row.commit.time * 1000, now)} />
          ))}
        </ol>
      )}
    </>
  );
});
