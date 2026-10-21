// 저장소의 워크트리 묶음. 트리와 그래프가 같은 목록을 그린다. 근거: docs/design.md 「저장소와 워크트리」.
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoId } from './gen/RepoId';
import type { RepoView } from './gen/RepoView';

/** 원래 체크아웃은 첫 커밋 전이면 `unborn`이다. 트리·그래프는 `linked`가 아니면 `worktree.<kind>` 꼬리표를 단다. */
export type GroupKind = 'original' | 'unborn' | 'linked';

export type Group = {
  /** 워크트리 절대 경로. 노드의 `place.worktree`와 같은 글자다. */
  path: string;
  /** 브랜치. 분리된 HEAD나 git에 없는 워크트리면 폴더 이름이다. */
  label: string;
  kind: GroupKind;
  /** 지울 때 함께 지울 수 있는 브랜치. */
  branch: string | null;
};

export function folderName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** git이 주는 워크트리 순서(첫째가 원래 체크아웃) 뒤에, git에 없지만 노드가 남은 워크트리를 붙인다. */
export function groupsOf(repo: RepoView, views: NodeView[]): Group[] {
  const groups: Group[] = repo.worktrees.map((w) => ({ path: w.path, label: w.branch ?? folderName(w.path), kind: !w.original ? 'linked' : repo.hasCommits ? 'original' : 'unborn', branch: w.branch }));
  for (const v of views) {
    if (v.place.repo === repo.id && !groups.some((g) => g.path === v.place.worktree)) {
      groups.push({ path: v.place.worktree, label: folderName(v.place.worktree), kind: 'linked', branch: null });
    }
  }
  return groups;
}

/** 그 묶음의 노드. 부모가 다른 묶음에 있는 노드는 이 묶음의 뿌리처럼 보인다. */
export function inGroup(views: NodeView[], group: Group): NodeView[] {
  return views.filter((v) => v.place.worktree === group.path);
}

/**
 * 묶음 나무의 부모 묶음 번호(`groups` 안). 그 묶음의 노드가 부모를 다른 묶음에 둔 간선에서 첫 번째(묶음 순서) 부모 묶음이고, 없으면 null이다.
 * 그래프 자동 정렬과 사이드바 트리가 이 나무 하나를 같이 쓴다. 순환이 없다.
 */
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

/**
 * 묶음마다 부모 묶음(`parents`) 안의 부모 카드. 이 묶음 노드 가운데 부모가 부모 묶음에 있는 첫 노드(`views` 순서)의 부모다. 뿌리 묶음은 null이다.
 * 그래프 자동 배치가 자식 묶음을 이 카드가 든 묶음 상자 안, 이 카드 아래 가운데에 둔다(목업 docs/mockups/arrange-family.html v4).
 */
export function attachCards(groups: readonly Group[], views: readonly NodeView[], parents: readonly (number | null)[]): (NodeId | null)[] {
  const byId = new Map(views.map((v) => [v.id, v]));
  return groups.map((group, i) => {
    const parent = parents[i];
    if (parent === null) return null;
    const child = views.find((v) => v.place.worktree === group.path && v.parent !== null && byId.get(v.parent)?.place.worktree === groups[parent].path);
    return child?.parent ?? null;
  });
}

/**
 * 순환(A↔B)이면 고리에서 뒤 묶음을 뿌리로 둔다: 부모 줄을 거슬러 올라가다 이미 본 묶음을 만나면 그 고리의 가장 뒤 묶음의 부모를 끊는다.
 */
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

/** 그래프가 그리는 저장소. 설정(`SettingsView.repo`)에 고른 저장소가 목록에 없으면 첫 저장소다. */
export function chosenRepo(repos: RepoView[], saved: RepoId | null): RepoView | null {
  return repos.find((r) => r.id === saved) ?? repos[0] ?? null;
}
