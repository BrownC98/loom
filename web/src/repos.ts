// 저장소의 워크트리 묶음. 트리와 그래프가 같은 목록을 그린다. 근거: docs/design.md 「저장소와 워크트리」.
import { useState } from 'react';
import type { NodeView } from './gen/NodeView';
import type { RepoId } from './gen/RepoId';
import type { RepoView } from './gen/RepoView';

/** 그래프를 그리는 저장소를 이 브라우저에 남기는 키. */
const REPO_KEY = 'loom.repo';

export type Group = {
  /** 워크트리 절대 경로. 노드의 `place.worktree`와 같은 글자다. */
  path: string;
  /** 브랜치. 분리된 HEAD나 git에 없는 워크트리면 폴더 이름이다. */
  label: string;
  original: boolean;
  /** 지울 때 함께 지울 수 있는 브랜치. */
  branch: string | null;
};

export function folderName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** git이 주는 워크트리 순서(첫째가 원래 체크아웃) 뒤에, git에 없지만 노드가 남은 워크트리를 붙인다. */
export function groupsOf(repo: RepoView, views: NodeView[]): Group[] {
  const groups: Group[] = repo.worktrees.map((w) => ({ path: w.path, label: w.branch ?? folderName(w.path), original: w.original, branch: w.branch }));
  for (const v of views) {
    if (v.place.repo === repo.id && !groups.some((g) => g.path === v.place.worktree)) {
      groups.push({ path: v.place.worktree, label: folderName(v.place.worktree), original: false, branch: null });
    }
  }
  return groups;
}

/** 그 묶음의 노드. 부모가 다른 묶음에 있는 노드는 이 묶음의 뿌리처럼 보인다. */
export function inGroup(views: NodeView[], group: Group): NodeView[] {
  return views.filter((v) => v.place.worktree === group.path);
}

/** 고른 저장소. 저장한 저장소가 목록에 없으면 첫 저장소다. 저장은 이 브라우저에만 한다. */
export function useSelectedRepo(repos: RepoView[]): [RepoView | null, (id: RepoId) => void] {
  const [saved, setSaved] = useState<RepoId | null>(() => {
    try {
      const value = Number(localStorage.getItem(REPO_KEY));
      return value > 0 ? value : null;
    } catch {
      return null;
    }
  });
  const select = (id: RepoId) => {
    setSaved(id);
    try {
      localStorage.setItem(REPO_KEY, String(id));
    } catch {
      // 저장하지 못해도 이 페이지에서는 고른 저장소를 그린다.
    }
  };
  return [repos.find((r) => r.id === saved) ?? repos[0] ?? null, select];
}
