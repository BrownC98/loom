import { describe, expect, it } from 'vitest';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import { attachCards, groupParents, groupsOf, inGroup, type Group } from './repos';

const view = (id: number, parent: number | null, worktree: string, repo = 1): NodeView => ({
  id,
  name: `n${id}`,
  agent: null,
  parent,
  state: { kind: 'running' },
  unread: 0,
  held: 0,
  model: null,
  effort: null,
  place: { repo, worktree },
  topic: null,
  wait: null,
});

const group = (path: string): Group => ({ path, label: path, kind: 'linked', branch: null });

const repo = (worktrees: RepoView['worktrees'], hasCommits = true): RepoView => ({ id: 1, name: 'r', path: '/r', worktrees, branches: [], hasCommits });

describe('groupParents', () => {
  const groups = [group('/a'), group('/b'), group('/c')];

  it('is null for groups without a node whose parent lives in another group', () => {
    expect(groupParents(groups, [view(1, null, '/a'), view(2, 1, '/a')])).toEqual([null, null, null]);
  });

  it('the parent group holds the parent node of a node in this group', () => {
    expect(groupParents(groups, [view(1, null, '/a'), view(2, 1, '/b'), view(3, 2, '/c')])).toEqual([null, 0, 1]);
  });

  it('with several candidates the first group by order wins', () => {
    const views = [view(1, null, '/a'), view(2, null, '/b'), view(3, 2, '/c'), view(4, 1, '/c')];
    expect(groupParents(groups, views)).toEqual([null, null, 0]);
  });

  it('A<->B cycle: the later group becomes the root of the ring', () => {
    const views = [view(1, 2, '/a'), view(2, 1, '/b')];
    expect(groupParents(groups, views)).toEqual([1, null, null]);
  });

  it('ignores parents outside every group', () => {
    expect(groupParents(groups, [view(1, 99, '/a')])).toEqual([null, null, null]);
  });
});

describe('attachCards', () => {
  const groups = [group('/a'), group('/b'), group('/c')];

  it('is the parent node inside the parent group', () => {
    // /a: 1 -> 2. /b: 3 (parent 1), /c: 4 (parent 2).
    const views = [view(1, null, '/a'), view(2, 1, '/a'), view(3, 1, '/b'), view(4, 2, '/c')];
    expect(attachCards(groups, views, groupParents(groups, views))).toEqual([null, 1, 2]);
  });

  it('the first child in views order wins', () => {
    const views = [view(1, null, '/a'), view(2, 1, '/a'), view(3, 2, '/b'), view(4, 1, '/b')];
    expect(attachCards(groups, views, groupParents(groups, views))[1]).toBe(2);
  });
});

describe('groupsOf', () => {
  it('git order first; the original checkout is original, or unborn before the first commit', () => {
    const worktrees = [
      { path: '/r', head: '', branch: 'main', original: true },
      { path: '/r/w', head: '', branch: 'feat', original: false },
      { path: '/r/d', head: '', branch: null, original: false },
    ];
    expect(groupsOf(repo(worktrees), []).map((g) => [g.label, g.kind, g.branch])).toEqual([
      ['main', 'original', 'main'],
      ['feat', 'linked', 'feat'],
      ['d', 'linked', null],
    ]);
    expect(groupsOf(repo(worktrees.slice(0, 1), false), [])[0].kind).toBe('unborn');
  });

  it('appends a worktree git does not list when a node of this repo stays there', () => {
    const groups = groupsOf(repo([{ path: '/r', head: '', branch: 'main', original: true }]), [view(1, null, '/gone/x'), view(2, null, '/other', 2), view(3, null, '/r')]);
    expect(groups.map((g) => g.path)).toEqual(['/r', '/gone/x']);
    expect(groups[1]).toEqual({ path: '/gone/x', label: 'x', kind: 'linked', branch: null });
  });
});

describe('inGroup', () => {
  it('keeps the nodes in the worktree, including roots whose parent is in another group', () => {
    const views = [view(1, null, '/a'), view(2, 1, '/b'), view(3, 2, '/b')];
    expect(inGroup(views, group('/b')).map((v) => v.id)).toEqual([2, 3]);
  });
});
