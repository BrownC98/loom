// 그래프 찾기(목업 docs/mockups/search.html)의 일치 목록. 이름만 본다: 노드 이름과 묶음 머리 글자(`Group.label`).
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import type { Spot } from './gen/Spot';
import { groupParents, groupsOf, inGroup } from './repos';

/** 찾기가 가리키는 상자. 저장소 상자는 찾지 않는다. */
export type Found = Exclude<Spot, { kind: 'repo' }>;

/**
 * `query`가 이름 안에 있는(대소문자 무시) 묶음과 노드. 순서는 사이드바 트리(Tree.tsx)와 같다:
 * 저장소 순서, 그 안의 묶음 나무(`groupParents`), 묶음 줄 다음에 그 묶음의 노드 나무(id 순), 그다음 자식 묶음.
 */
export function findSpots(repos: readonly RepoView[], views: NodeView[], query: string): Found[] {
  const needle = query.trim().toLocaleLowerCase();
  if (needle === '') return [];
  const hit = (name: string) => name.toLocaleLowerCase().includes(needle);
  return repos.flatMap((repo) => {
    const groups = groupsOf(repo, views);
    const parents = groupParents(groups, views);
    /** `parent`가 없으면 묶음의 뿌리: 부모가 없거나 부모가 다른 묶음에 있는 노드다. */
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
