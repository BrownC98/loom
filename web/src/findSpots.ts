import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoId } from './gen/RepoId';
import type { RepoView } from './gen/RepoView';
import type { Spot } from './gen/Spot';
import { groupParents, groupsOf, inGroup } from './repos';

export type Found = Exclude<Spot, { kind: 'repo' | 'node' }> | { kind: 'node'; repo: RepoId; node: NodeId };

export function findSpots(repos: readonly RepoView[], views: NodeView[], query: string): Found[] {
  if (query.trim() === '') return [];
  return spotsWhere(repos, views, nameHit(query));
}

export function nameHit(query: string): (name: string) => boolean {
  const needle = query.trim().toLocaleLowerCase();
  return (name) => name.toLocaleLowerCase().includes(needle);
}

export function spotsWhere(repos: readonly RepoView[], views: NodeView[], hit: (name: string) => boolean): Found[] {
  return repos.flatMap((repo) => {
    const groups = groupsOf(repo, views);
    const parents = groupParents(groups, views);
    const nodes = (members: NodeView[], parent: number | null): Found[] =>
      members
        .filter((v) => (parent === null ? !members.some((p) => p.id === v.parent) : v.parent === parent))
        .sort((a, b) => a.id - b.id)
        .flatMap((v): Found[] => [...(hit(v.name) ? [{ kind: 'node', repo: repo.id, node: v.id } as const] : []), ...nodes(members, v.id)]);
    const tree = (parent: number | null): Found[] =>
      groups.flatMap((g, i): Found[] =>
        parents[i] !== parent ? [] : [...(hit(g.label) ? [{ kind: 'worktree', repo: repo.id, path: g.path } as const] : []), ...nodes(inGroup(views, g), null), ...tree(i)],
      );
    return tree(null);
  });
}
