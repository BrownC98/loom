import { deepStrictEqual } from 'node:assert/strict';
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { circleOf, laneCount, lanes } from '../src/commitLanes.ts';
import { hopParts, pickHue, siblingLanes } from '../src/hopLanes.ts';
import {
  arrangeNested,
  bendCenter,
  centerSide,
  crossed,
  edgeSides,
  freeSpot,
  labelHeading,
  push,
  samePoint,
  settle,
  stepAt,
  swap,
  unpile,
} from '../src/layout.ts';
import { attachCards, bookmarked, chosenRepo, groupParents, groupsOf, inGroup, placeName, splitChips } from '../src/repos.ts';

const dir = fileURLToPath(new URL('../../fixtures/layout/', import.meta.url));
const write = process.argv.includes('--write');
const point = (p) => ({ x: p.x, y: p.y });
const box = (b) => ({ at: point(b.at), size: { width: b.size.width, height: b.size.height } });
const points = (m) => Object.fromEntries([...m].map(([key, p]) => [key, point(p)]));
const entries = (o) => new Map(Object.entries(o));

const run = {
  arrangeNested: (i) => {
    const out = arrangeNested(
      i.groups,
      (id) => i.parents[id] ?? null,
      (id) => i.sizes[id],
      i.direction,
      i.inset,
      (kid) => i.rank['card' in kid ? `card:${kid.card}` : `group:${kid.group}`] ?? Infinity,
    );
    return { cards: Object.fromEntries([...out.cards].map(([id, p]) => [id, point(p)])), boxes: out.boxes };
  },
  settle: (i) => points(settle(i.items, (key) => i.placed[key], i.gap)),
  push: (i) => points(push(i.siblings, i.mover, i.targets, i.gap)),
  unpile: (i) => points(unpile(i.siblings, i.gap)),
  swap: (i) => {
    const out = swap({ slot: i.state.slot, sides: entries(i.state.sides), at: entries(i.state.at) }, i.others, i.dragged);
    return { slot: box(out.slot), sides: Object.fromEntries(out.sides), at: points(out.at) };
  },
  centerSide: (cases) => cases.map((c) => centerSide(c.dragged, c.box)),
  crossed: (cases) => cases.map((c) => crossed(c.before, c.dragged, c.box)),
  freeSpot: (cases) => cases.map((c) => point(freeSpot(c.box, c.others, c.gap))),
  samePoint: (cases) => cases.map((c) => samePoint(c.a, c.b)),
  stepAt: (cases) => cases.map((c) => ({ step: stepAt(c.parent, c.child, c.direction, c.bend) })),
  bendCenter: (cases) => cases.map((c) => bendCenter(c.parent, c.bend, c.direction)),
  edgeSides: (cases) => cases.map((c) => edgeSides(c.parent, c.child, c.direction)),
  labelHeading: (cases) => cases.map((c) => ({ heading: labelHeading(c.from, c.to, c.step) })),
};

const place = (v) => ({ repo: v.repo, worktree: v.worktree });
const viewsOf = (list) => list.map((v) => ({ id: v.id, parent: v.parent, place: place(v), bookmark: v.bookmark }));
const repoOf = (r) => ({ id: r.id, name: r.name, hasCommits: r.hasCommits, git: r.git, worktrees: r.worktrees });
const ids = (list) => list.map((v) => v.id);

const hopAndRepoRuns = {
  hopParts: (i) => i.cases.map((c) => hopParts(c.counts, c.source, c.target, c.turned)),
  pickHue: (i) => {
    const randoms = [...i.randoms];
    const real = Math.random;
    Math.random = () => randoms.shift();
    try {
      return pickHue(i.shown);
    } finally {
      Math.random = real;
    }
  },
  siblingLanes: (i) => Object.fromEntries(siblingLanes(i.parent, i.pairs, i.direction)),
  repoGroups: (i) => {
    const repo = repoOf(i.repo);
    const views = viewsOf(i.views);
    const groups = groupsOf(repo, views);
    const parents = groupParents(groups, views);
    const { boxes, chips } = splitChips(groups, views);
    return {
      groups: groups.map((g) => ({ path: g.path, label: g.label, kind: g.kind })),
      boxes: boxes.map((g) => g.path),
      chips: chips.map((g) => g.path),
      parents,
      attach: attachCards(groups, views, parents),
      inGroup: groups.map((g) => ids(inGroup(views, g))),
    };
  },
  repoPlaces: (i) => {
    const repos = i.repos.map(repoOf);
    const views = viewsOf(i.views);
    return {
      names: views.map((v) => placeName(repos, views, v)),
      bookmarked: ids(bookmarked(views)),
      chosen: i.saved.map((saved) => chosenRepo(repos, saved)?.id ?? null),
    };
  },
};
const commitRuns = {
  commitLanes: (i) => {
    const commits = i.commits.map((c) => ({ hash: c.hash, parents: c.parents, subject: '', author: '', time: 0, refs: [] }));
    const out = lanes(commits, i.from);
    return {
      rows: out.rows.map((row) => ({ hash: row.commit.hash, input: row.input, output: row.output, circle: circleOf(row), count: laneCount(row) })),
      end: out.end,
    };
  },
};
Object.assign(run, hopAndRepoRuns, commitRuns);

let failed = false;
for (const name of readdirSync(dir).filter((f) => f.endsWith('.json')).sort()) {
  const file = join(dir, name);
  const fixture = JSON.parse(readFileSync(file, 'utf8'));
  const actual = JSON.parse(JSON.stringify(run[fixture.kind](fixture.input)));
  if (write) {
    writeFileSync(file, `${JSON.stringify({ ...fixture, output: actual }, null, 2)}\n`);
    continue;
  }
  try {
    deepStrictEqual(actual, fixture.output);
  } catch (err) {
    failed = true;
    console.error(`${name}: ${err.message}`);
  }
}
if (failed) process.exit(1);
