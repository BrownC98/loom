import { deepStrictEqual } from 'node:assert/strict';
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { hopParts, pickHue, siblingLanes } from '../src/hopLanes.ts';
import { arrangeNested, settle } from '../src/layout.ts';
import { attachCards, bookmarked, chosenRepo, groupParents, groupsOf, inGroup, placeName, splitChips } from '../src/repos.ts';

const dir = fileURLToPath(new URL('../../fixtures/layout/', import.meta.url));
const write = process.argv.includes('--write');
const point = (p) => ({ x: p.x, y: p.y });

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
  settle: (i) => Object.fromEntries([...settle(i.items, (key) => i.placed[key], i.gap)].map(([key, p]) => [key, point(p)])),
};

const place = (v) => ({ repo: v.repo, worktree: v.worktree });
const viewsOf = (list) => list.map((v) => ({ id: v.id, parent: v.parent, place: place(v), bookmark: v.bookmark }));
const repoOf = (r) => ({ id: r.id, name: r.name, hasCommits: r.hasCommits, worktrees: r.worktrees });
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
      groups,
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
Object.assign(run, hopAndRepoRuns);

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
