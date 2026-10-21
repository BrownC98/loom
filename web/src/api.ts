// 데몬 API. 요청·응답 타입은 Rust가 원천이다(gen/).
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { CardSize } from './gen/CardSize';
import type { CloneRepo } from './gen/CloneRepo';
import type { CommitGraph } from './gen/CommitGraph';
import type { CreateRepo } from './gen/CreateRepo';
import type { EventRecord } from './gen/EventRecord';
import type { FolderList } from './gen/FolderList';
import type { GraphEvent } from './gen/GraphEvent';
import type { LoginId } from './gen/LoginId';
import type { LoginState } from './gen/LoginState';
import type { LoginTarget } from './gen/LoginTarget';
import type { MovableNode } from './gen/MovableNode';
import type { MoveResult } from './gen/MoveResult';
import type { NewNode } from './gen/NewNode';
import type { NewRepo } from './gen/NewRepo';
import type { NewWorktree } from './gen/NewWorktree';
import type { NodeId } from './gen/NodeId';
import type { OpenFile } from './gen/OpenFile';
import type { Arranged } from './gen/Arranged';
import type { Layout } from './gen/Layout';
import type { Placed } from './gen/Placed';
import type { Problem } from './gen/Problem';
import type { Provider } from './gen/Provider';
import type { PtyNotice } from './gen/PtyNotice';
import type { RenameNode } from './gen/RenameNode';
import type { RenameWorktree } from './gen/RenameWorktree';
import type { RepoId } from './gen/RepoId';
import type { RepoView } from './gen/RepoView';
import type { Resize } from './gen/Resize';
import type { SettingsView } from './gen/SettingsView';
import { problemText, statusText } from './problem';

/** 끊긴 WebSocket을 다시 여는 간격. 데몬이 다시 시작하는 동안 기다린다. */
const RECONNECT_MS = 1000;

/** 실패하면 데몬이 보낸 코드(`Problem`)를 현재 언어로 바꾼 문장을 담은 Error를 던진다. */
async function request<T>(method: string, path: string, body?: { type: string; data: BodyInit }): Promise<T> {
  const res = await fetch(path, {
    method,
    headers: body === undefined ? undefined : { 'content-type': body.type },
    body: body?.data,
  });
  // 데몬이 아는 거절은 Problem JSON으로 온다. 본문이 JSON이 아니면 화면이 데몬보다 오래된 판이라 없는 경로를 불렀을 가능성이 크다.
  if (!res.ok) {
    const problem = (await res.json().catch(() => null)) as Problem | null;
    throw new Error(problem ? problemText(problem) : statusText(res.status));
  }
  return res.json();
}

const call = <T>(method: string, path: string, body?: unknown) =>
  request<T>(method, path, body === undefined ? undefined : { type: 'application/json', data: JSON.stringify(body) });

/** 붙여넣거나 끌어다 놓은 파일을 데몬이 같은 이름으로 남기고 그 절대 경로를 돌려준다. */
export const savePastedFile = (file: File) =>
  request<string>('POST', `/api/pastes/${encodeURIComponent(file.name)}`, { type: file.type || 'application/octet-stream', data: file });

export const createNode = (req: NewNode) => call<NodeId>('POST', '/api/nodes', req);
/** 노드 하나를 지운다. 자식은 부모 없이 남는다(docs/design.md 「노드 제거와 입양」). */
export const removeNode = (id: NodeId) => call<null>('DELETE', `/api/nodes/${id}`);
/** `parent`가 `child`의 새 부모가 된다. 거절하면 데몬의 문장을 담은 Error를 던진다. */
export const adoptNode = (child: NodeId, parent: NodeId) => call<null>('PUT', `/api/nodes/${child}/parent/${parent}`);
/** 이름을 바꾸고 부모·자식에게 알린다. 거절하면 데몬의 문장을 담은 Error를 던진다. */
/** 노드를 사이드바 북마크 영역에 더하거나 뺀다(docs/mockups/bookmarks.html). */
export const bookmarkNode = (id: NodeId) => call<null>('PUT', `/api/nodes/${id}/bookmark`);
export const unbookmarkNode = (id: NodeId) => call<null>('DELETE', `/api/nodes/${id}/bookmark`);
/** 사람 응답 대기를 지운다. 터미널에 키를 누른 것과 같다(docs/design.md 「사람 응답 대기」). */
export const dismissWait = (id: NodeId) => call<null>('DELETE', `/api/nodes/${id}/wait`);
export const renameNode = (id: NodeId, name: string) => call<null>('PUT', `/api/nodes/${id}/name`, { name } satisfies RenameNode);

/** 데몬만 끝낸다. 노드는 산다(docs/design.md 「배포」). 응답에 본문이 없다. */
export async function shutdown(): Promise<void> {
  const res = await fetch('/api/shutdown', { method: 'POST' });
  if (!res.ok) throw new Error(statusText(res.status));
}

export const openFile = (path: string) => call<null>('POST', '/api/files/open', { path } satisfies OpenFile);
/** 부를 때마다 데몬이 PATH를 다시 훑는다. */
export const getAgents = () => call<AgentView[]>('GET', '/api/agents');
export const getSettings = () => call<SettingsView>('GET', '/api/settings');
export const putSettings = (view: SettingsView) => call<null>('PUT', '/api/settings', view);
/** 모든 브랜치의 최근 커밋과 워크트리마다 체크아웃한 커밋. */
export const getCommits = (repo: RepoId) => call<CommitGraph>('GET', `/api/repos/${repo}/commits`);
/** 더한 순서. 워크트리 첫째가 원래 체크아웃이다. */
export const getRepos = () => call<RepoView[]>('GET', '/api/repos');
/** 사람이 끌어 놓은 자리(docs/mockups/repo-drag.html v4). 한 번 놓기의 자리(놓은 상자와 비킨 상자)는 한 요청으로 쓴다. */
export const getSpots = () => call<Layout>('GET', '/api/spots');
export const putSpots = (placed: Placed[]) => call<null>('PUT', '/api/spots', placed);
export const putArranged = (arranged: Arranged) => call<null>('PUT', '/api/spots/arranged', arranged);
/** [정렬]: 그 저장소의 자리를 지운다. */
export const clearSpots = (repo: RepoId) => call<null>('DELETE', `/api/repos/${repo}/spots`);
/** 사람이 바꾼 표준 카드 크기(docs/mockups/card-resize.html). 저장한 크기가 없는 카드는 기본 크기다. */
export const getSizes = () => call<CardSize[]>('GET', '/api/sizes');
export const putSizes = (sizes: CardSize[]) => call<null>('PUT', '/api/sizes', sizes);
/** 그 카드들을 기본 크기로 되돌린다. */
export const clearSizes = (nodes: NodeId[]) => call<null>('DELETE', '/api/sizes', nodes);
export const addRepo = (req: NewRepo) => call<RepoView>('POST', '/api/repos', req);
export const cloneRepo = (req: CloneRepo) => call<RepoView>('POST', '/api/repos/clone', req);
export const createRepo = (req: CreateRepo) => call<RepoView>('POST', '/api/repos/create', req);
/** 목록에서만 뺀다. 폴더는 그대로다. 그 저장소에 노드가 남았으면 거절한다. */
export const removeRepo = (repo: RepoId) => call<null>('DELETE', `/api/repos/${repo}`);
/** 데몬이 보는 폴더의 자식 폴더. `path`가 없으면 `<홈>/loom`이다. */
export const getFolders = (path?: string) => call<FolderList>('GET', path === undefined ? '/api/folders' : `/api/folders?${new URLSearchParams({ path })}`);
export const createWorktree = (repo: RepoId, req: NewWorktree) => call<RepoView>('POST', `/api/repos/${repo}/worktrees`, req);
const worktreeQuery = (path: string) => `path=${encodeURIComponent(path)}`;
/** 브랜치 이름만 바꾼다(`git branch -m`). 폴더와 노드는 그대로다. */
export const renameWorktree = (repo: RepoId, path: string, name: string) => call<RepoView>('PUT', `/api/repos/${repo}/worktrees/name`, { path, name } satisfies RenameWorktree);
/** 삭제하면 잃는 변경. 줄마다 `git status --porcelain` 한 줄이다. */
export const getWorktreeChanges = (repo: RepoId, path: string) => call<string[]>('GET', `/api/repos/${repo}/worktrees/changes?${worktreeQuery(path)}`);
/** 그 묶음의 노드를 끝내고 폴더를 지운다(`--force`). `deleteBranch`면 브랜치도 지운다. */
export const deleteWorktree = (repo: RepoId, path: string, deleteBranch: boolean) =>
  call<RepoView>('DELETE', `/api/repos/${repo}/worktrees?${worktreeQuery(path)}&deleteBranch=${deleteBranch}`);
/** 최근 이벤트 기록 500개, 최근 것부터. */
export const getEventLog = () => call<EventRecord[]>('GET', '/api/events/log');
/** 계정마다 사용량을 그 자리에서 읽는다. 몇 초 걸린다. */
export const getAccounts = () => call<AccountView[]>('GET', '/api/accounts');
/** 그 공급자에서 `to` 계정이 아닌 계정으로 도는 노드. `to`가 null이면 시스템 기본이 아닌 노드다. */
export const getMovableNodes = (provider: Provider, to: AccountId | null) =>
  call<MovableNode[]>('GET', `/api/accounts/${provider}/nodes${to === null ? '' : `?${new URLSearchParams({ to })}`}`);
/** 그 노드들을 설정에 저장한 계정으로 옮긴다. */
export const moveNodes = (provider: Provider) => call<MoveResult>('POST', `/api/accounts/${provider}/move`);
export const removeAccount = (provider: Provider, id: AccountId) => call<null>('DELETE', `/api/accounts/${provider}/${id}`);
export const startLogin = (provider: Provider, target: LoginTarget) => call<LoginId>('POST', `/api/accounts/${provider}`, target);
export const logoutSystem = (provider: Provider) => call<null>('POST', `/api/accounts/${provider}/logout`);
export const getLogin = (id: LoginId) => call<LoginState>('GET', `/api/logins/${id}`);
export const closeLogin = (id: LoginId) => call<null>('DELETE', `/api/logins/${id}`);
export const loginPty = (id: LoginId) => `/api/logins/${id}/pty`;
export const nodePty = (id: NodeId) => `/api/nodes/${id}/pty`;

function socketUrl(path: string): string {
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}${path}`;
}

/** 그래프 이벤트를 받는다. 연결할 때마다 노드 목록 전체(`snapshot`)를 먼저 받는다. 반환값은 구독 해제 함수다. */
export function watchGraph(onEvent: (event: GraphEvent) => void): () => void {
  let socket: WebSocket;
  let stopped = false;
  const open = () => {
    socket = new WebSocket(socketUrl('/api/events'));
    socket.onmessage = (e) => onEvent(JSON.parse(e.data) as GraphEvent);
    socket.onclose = () => {
      if (!stopped) setTimeout(open, RECONNECT_MS);
    };
  };
  open();
  return () => {
    stopped = true;
    socket.close();
  };
}

/**
 * 데몬이 지금 내는 index.html의 진입 스크립트가 이 화면을 띄운 것과 다른가. `scripts/host.sh`가 데몬을 새 판으로 바꾸면 참이다.
 * 진입 스크립트 이름에는 빌드 해시가 붙는다. 데스크톱 앱도 화면을 자기 데몬에서 받으므로 같은 규칙이다.
 */
export async function newBuildServed(): Promise<boolean> {
  const html = await (await fetch('/', { cache: 'no-store' })).text();
  const entry = (doc: Document) => doc.querySelector('script[type="module"][src]')?.getAttribute('src');
  return entry(new DOMParser().parseFromString(html, 'text/html')) !== entry(document);
}

export interface PtyLink {
  sendInput(data: string | Uint8Array): void;
  sendResize(size: Resize): void;
  close(): void;
}

/** PTY에 붙는다(`nodePty`·`loginPty`). 연결할 때마다 `onAttach`를 부른 뒤 backlog부터 출력을 넘긴다. */
export function attachPty(
  path: string,
  handlers: { onAttach: () => void; onOutput: (bytes: Uint8Array) => void; onNotice: (notice: PtyNotice) => void },
): PtyLink {
  const encoder = new TextEncoder();
  let socket: WebSocket;
  let stopped = false;
  const open = () => {
    socket = new WebSocket(socketUrl(path));
    socket.binaryType = 'arraybuffer';
    socket.onopen = handlers.onAttach;
    // 출력은 바이너리 프레임, 데몬의 알림(`PtyNotice`)은 글 프레임이다.
    socket.onmessage = (e) =>
      typeof e.data === 'string' ? handlers.onNotice(JSON.parse(e.data) as PtyNotice) : handlers.onOutput(new Uint8Array(e.data as ArrayBuffer));
    socket.onclose = () => {
      if (!stopped) setTimeout(open, RECONNECT_MS);
    };
  };
  open();
  const send = (data: string | Uint8Array<ArrayBuffer>) => {
    if (socket.readyState === WebSocket.OPEN) socket.send(data);
  };
  return {
    sendInput: (data) => send(typeof data === 'string' ? encoder.encode(data) : new Uint8Array(data)),
    sendResize: (size) => send(JSON.stringify(size)),
    close: () => {
      stopped = true;
      socket.close();
    },
  };
}
