import { describe, expect, it } from 'vitest';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import { findSpots } from './findSpots';

const view = (id: number, name: string, parent: number | null, worktree: string, repo = 1): NodeView => ({
  id,
  name,
  agent: null,
  parent,
  state: { kind: 'running' },
  unread: 0,
  model: null,
  effort: null,
  place: { repo, worktree },
  topic: null,
  bookmark: null,
  waits: [],
});

const repo = (id: number, worktrees: [string, string | null][]): RepoView => ({
  id,
  name: `r${id}`,
  path: worktrees[0][0],
  worktrees: worktrees.map(([path, branch], i) => ({ path, head: '', branch, original: i === 0 })),
  branches: [],
  hasCommits: true,
  wsl: null,
});

// 목업 search.html ②의 모양: main에 총괄과 같은 워크트리 자식, 코디마다 자기 워크트리, 빈 워크트리 하나.
const loom = repo(1, [
  ['/loom', 'main'],
  ['/w/find', 'find'],
  ['/w/skill', 'skill'],
  ['/w/win', 'win-find'],
]);
const views = [view(1, 'loom lead', null, '/loom'), view(2, 'find', 1, '/w/find'), view(3, 'win-find', 1, '/loom'), view(4, 'skill', 1, '/w/skill')];

describe('findSpots', () => {
  it('finds nothing for an empty or blank query', () => {
    expect(findSpots([loom], views, '')).toEqual([]);
    expect(findSpots([loom], views, '  ')).toEqual([]);
  });

  it('matches node and worktree names in sidebar tree order', () => {
    expect(findSpots([loom], views, 'find')).toEqual([
      { kind: 'node', repo: 1, node: 3 },
      { kind: 'worktree', repo: 1, path: '/w/find' },
      { kind: 'node', repo: 1, node: 2 },
      { kind: 'worktree', repo: 1, path: '/w/win' },
    ]);
  });

  it('ignores case and trims the query', () => {
    const r = repo(1, [['/r', 'Feature-X']]);
    expect(findSpots([r], [view(1, 'Lead', null, '/r')], ' feature ')).toEqual([{ kind: 'worktree', repo: 1, path: '/r' }]);
    expect(findSpots([r], [view(1, 'Lead', null, '/r')], 'LEAD')).toEqual([{ kind: 'node', repo: 1, node: 1 }]);
  });

  it('uses the folder name for a detached worktree, as the group head shows it', () => {
    const r = repo(1, [['/r', 'main'], ['/w/detached-one', null]]);
    expect(findSpots([r], [], 'detached')).toEqual([{ kind: 'worktree', repo: 1, path: '/w/detached-one' }]);
  });

  it('does not match a path, agent or state', () => {
    expect(findSpots([loom], views, '/w/')).toEqual([]);
    expect(findSpots([loom], views, 'running')).toEqual([]);
  });

  it('walks nodes depth first inside a group, then child groups, then the next repository', () => {
    const other = repo(2, [['/k', 'main']]);
    const all = [view(10, 'a-root', null, '/k', 2), view(5, 'a-child', 1, '/loom'), view(6, 'a-grand', 5, '/loom'), ...views];
    expect(findSpots([loom, other], all, 'a-')).toEqual([
      { kind: 'node', repo: 1, node: 5 },
      { kind: 'node', repo: 1, node: 6 },
      { kind: 'node', repo: 2, node: 10 },
    ]);
  });
});
