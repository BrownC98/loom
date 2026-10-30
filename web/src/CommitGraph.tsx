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
const ROW_HEIGHT = 22;
const CURVE_RADIUS = 5;
const CIRCLE_RADIUS = 4;
const REFRESH_MS = 3000;
const PAGE = 100;
const LOAD_AHEAD = 20;
const OVERSCAN = 10;

type Loaded = { version: string; worktrees: Worktree[]; rows: Row[]; end: LaneState; lanes: number; done: boolean };

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
    let busy: Promise<void> = Promise.resolve();
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
