import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoId } from './gen/RepoId';
import type { RepoView } from './gen/RepoView';

export type GroupKind = 'original' | 'unborn' | 'linked';

export type Group = {
  path: string;
  label: string;
  kind: GroupKind;
  branch: string | null;
};

export function folderName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

export function groupsOf(repo: RepoView, views: NodeView[]): Group[] {
  const groups: Group[] = repo.worktrees.map((w) => ({ path: w.path, label: w.branch ?? folderName(w.path), kind: !w.original ? 'linked' : repo.hasCommits ? 'original' : 'unborn', branch: w.branch }));
  for (const v of views) {
    if (v.place.repo === repo.id && !groups.some((g) => g.path === v.place.worktree)) {
      groups.push({ path: v.place.worktree, label: folderName(v.place.worktree), kind: 'linked', branch: null });
    }
  }
  return groups;
}

export function splitChips(groups: readonly Group[], views: readonly NodeView[]): { boxes: Group[]; chips: Group[] } {
  const chip = (g: Group) => g.kind === 'linked' && !views.some((v) => v.place.worktree === g.path);
  if (!groups.some((g) => g.kind !== 'linked')) return { boxes: [...groups], chips: [] };
  return { boxes: groups.filter((g) => !chip(g)), chips: groups.filter(chip) };
}

export function inGroup(views: NodeView[], group: Group): NodeView[] {
  return views.filter((v) => v.place.worktree === group.path);
}

export function groupParents(groups: readonly Group[], views: readonly NodeView[]): (number | null)[] {
  const groupOf = new Map(views.map((v) => [v.id, groups.findIndex((g) => g.path === v.place.worktree)]));
  return breakCycles(
    groups.map((group, i) => {
      const parents = views.flatMap((v) => {
        const parent = v.parent === null ? undefined : groupOf.get(v.parent);
        return v.place.worktree === group.path && parent !== undefined && parent >= 0 && parent !== i ? [parent] : [];
      });
      return parents.length > 0 ? Math.min(...parents) : null;
    }),
  );
}

export function attachCards(groups: readonly Group[], views: readonly NodeView[], parents: readonly (number | null)[]): (NodeId | null)[] {
  const byId = new Map(views.map((v) => [v.id, v]));
  return groups.map((group, i) => {
    const parent = parents[i];
    if (parent === null) return null;
    const child = views.find((v) => v.place.worktree === group.path && v.parent !== null && byId.get(v.parent)?.place.worktree === groups[parent].path);
    return child?.parent ?? null;
  });
}

function breakCycles(parents: readonly (number | null)[]): (number | null)[] {
  const fixed = [...parents];
  for (let i = 0; i < fixed.length; i++) {
    const seen = new Set<number>([i]);
    for (let at = fixed[i]; at !== null; at = fixed[at]) {
      if (seen.has(at)) {
        const ring = [at];
        for (let k = fixed[at]!; k !== at; k = fixed[k]!) ring.push(k);
        fixed[Math.max(...ring)] = null;
        break;
      }
      seen.add(at);
    }
  }
  return fixed;
}

export function chosenRepo(repos: RepoView[], saved: RepoId | null): RepoView | null {
  return repos.find((r) => r.id === saved) ?? repos[0] ?? null;
}
