// 데몬 API. 요청·응답 타입은 Rust가 원천이다(gen/).
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { AnswerWait } from './gen/AnswerWait';
import type { CardSize } from './gen/CardSize';
import type { CloneRepo } from './gen/CloneRepo';
import type { CommitGraph } from './gen/CommitGraph';
import type { CreateRepo } from './gen/CreateRepo';
import type { EventRecord } from './gen/EventRecord';
import type { FolderList } from './gen/FolderList';
import type { Locations } from './gen/Locations';
import type { DistroView } from './gen/DistroView';
import type { GraphEvent } from './gen/GraphEvent';
import type { DeviceId } from './gen/DeviceId';
import type { InviteView } from './gen/InviteView';
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
import type { PhoneView } from './gen/PhoneView';
import type { Problem } from './gen/Problem';
import type { Provider } from './gen/Provider';
import type { PtyNotice } from './gen/PtyNotice';
import type { Relaunched } from './gen/Relaunched';
import type { RenameNode } from './gen/RenameNode';
import type { RenameWorktree } from './gen/RenameWorktree';
import type { RepoId } from './gen/RepoId';
import type { RepoView } from './gen/RepoView';
import type { Resize } from './gen/Resize';
import type { SettingsView } from './gen/SettingsView';
import type { UpdateView } from './gen/UpdateView';
import type { SettingsPatch } from './settingsPatch';
import type { WaitId } from './gen/WaitId';
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

/** 한 파일의 상한. 데몬 `src/paste.rs`의 `LIMIT_BYTES`와 같다. 큰 파일은 오래 올리지 않게 올리기 전에 거른다. */
export const PASTE_LIMIT_BYTES = 32 * 1024 * 1024;

/** 붙여넣거나 끌어다 놓은 파일을 데몬이 같은 이름으로 남기고 그 절대 경로를 돌려준다. 터미널과 질문 답의 그림이 쓴다. */
export const savePastedFile = (file: File, node: NodeId | null) =>
  request<string>('POST', `/api/pastes/${encodeURIComponent(file.name)}${node === null ? '' : `?node=${node}`}`, {
    type: file.type || 'application/octet-stream',
    data: file,
  });

export const createNode = (req: NewNode) => call<NodeId>('POST', '/api/nodes', req);
/** 노드 하나를 지운다. 자식은 부모 없이 남는다(docs/design.md 「노드 제거와 입양」). */
export const removeNode = (id: NodeId) => call<null>('DELETE', `/api/nodes/${id}`);
/** `parent`가 `child`의 새 부모가 된다. 거절하면 데몬의 문장을 담은 Error를 던진다. */
export const adoptNode = (child: NodeId, parent: NodeId) => call<null>('PUT', `/api/nodes/${child}/parent/${parent}`);
/** 이름을 바꾸고 부모·자식에게 알린다. 거절하면 데몬의 문장을 담은 Error를 던진다. */
/** 노드를 사이드바 북마크 영역에 더하거나 뺀다(docs/mockups/bookmarks.html). */
export const bookmarkNode = (id: NodeId) => call<null>('PUT', `/api/nodes/${id}/bookmark`);
/** 노드 카드 「안 읽음」: 기다리는 큐 메시지를 지금 넘긴다(조정으로 바꾼다). */
export const steerQueued = (id: NodeId) => call<null>('POST', `/api/nodes/${id}/steer-queued`);
export const unbookmarkNode = (id: NodeId) => call<null>('DELETE', `/api/nodes/${id}/bookmark`);
/** 사람 응답 대기를 지운다. 터미널에 키를 누른 것과 같다(docs/design.md 「사람 응답 대기」). */
export const dismissWait = (wait: WaitId) => call<null>('DELETE', `/api/waits/${wait}`);
/** 화면에서 `loom ask` 질문에 답한다. `images`는 `savePastedFile`이 준 그림 경로다. 답은 사람이 보낸 메시지로 그 노드에 가고 질문은 사라진다. */
export const answerWait = (wait: WaitId, text: string, images: string[]) =>
  call<null>('POST', `/api/waits/${wait}/answer`, { text, images } satisfies AnswerWait);
/** 질문에 붙은 문서를 데몬이 그 기계의 기본 브라우저로 연다. 데스크톱 앱에서도 앱 창이 아니라 브라우저로 연다. */
export const openWaitDoc = (wait: WaitId) => call<null>('POST', `/api/waits/${wait}/doc`);
/** 질문 문서의 주소. 끝의 `/` 때문에 문서 속 상대 주소(옆 그림)가 같은 폴더를 가리킨다. */
export const waitDocUrl = (wait: WaitId) => `/api/waits/${wait}/doc/`;
/** 완료 줄의 `index`번째 그림(`loom done --image`). 확인하지 않은 완료의 그림만 나온다. */
export const doneImageUrl = (wait: WaitId, index: number) => `/api/waits/${wait}/images/${index}`;
export const renameNode = (id: NodeId, name: string) => call<null>('PUT', `/api/nodes/${id}/name`, { name } satisfies RenameNode);

/** 데몬만 끝낸다. 노드는 산다(docs/design.md 「배포」). 응답에 본문이 없다. */
export async function shutdown(): Promise<void> {
  const res = await fetch('/api/shutdown', { method: 'POST' });
  if (!res.ok) throw new Error(statusText(res.status));
}

export const openFile = (path: string, node: NodeId | null) => call<null>('POST', '/api/files/open', { path, node } satisfies OpenFile);
/** 부를 때마다 데몬이 PATH를 다시 훑는다. */
export const getAgents = () => call<AgentView[]>('GET', '/api/agents');
export const getSettings = () => call<SettingsView>('GET', '/api/settings');
/** 모든 칸이 기본값인 설정. 설정 창이 칸마다 [기본값] 단추를 보일지 견준다. */
export const getDefaultSettings = () => call<SettingsView>('GET', '/api/settings/default');
/** 바꾼 칸만 보낸다. 데몬이 지금 설정에 합친다. */
export const putSettings = (patch: SettingsPatch) => call<null>('PUT', '/api/settings', patch);
/** 데몬의 업데이트 상태(docs/mockups/web-update.html). */
export const getUpdate = () => call<UpdateView>('GET', '/api/update');
/** 데몬이 지금 `latest.json`을 본다. */
export const checkUpdate = () => call<UpdateView>('POST', '/api/update/check');
/** 데몬이 새 판을 받아 바꾸고 다시 뜬다. 바로 돌아오고, 진행은 상태로 본다. */
export const installUpdate = () => call<UpdateView>('POST', '/api/update/install');
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
/** [저장소 추가] 위치 칩. 배포판을 켜지 않는다. */
export const getLocations = () => call<Locations>('GET', '/api/locations');
/** 배포판의 홈과 기본 워크스페이스. 꺼진 배포판은 데몬이 켠다(몇 초). */
export const getDistro = (name: string) => call<DistroView>('GET', `/api/distros/${encodeURIComponent(name)}`);
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
/** 계정 라우트의 위치 질의. `distro`가 null이면 이 PC다. */
const at = (distro: string | null): Record<string, string> => (distro === null ? {} : { distro });
const query = (params: Record<string, string>) => (Object.keys(params).length === 0 ? '' : `?${new URLSearchParams(params)}`);
/** 그 위치, 그 공급자에서 `to` 계정이 아닌 계정으로 도는 노드. `to`가 null이면 시스템 기본이 아닌 노드다. */
export const getMovableNodes = (provider: Provider, distro: string | null, to: AccountId | null) =>
  call<MovableNode[]>('GET', `/api/accounts/${provider}/nodes${query({ ...at(distro), ...(to === null ? {} : { to }) })}`);
/** 그 위치의 노드들을 설정에 저장한 그 위치의 계정으로 옮긴다. */
export const moveNodes = (provider: Provider, distro: string | null) => call<MoveResult>('POST', `/api/accounts/${provider}/move${query(at(distro))}`);
export const removeAccount = (provider: Provider, distro: string | null, id: AccountId) => call<null>('DELETE', `/api/accounts/${provider}/${id}${query(at(distro))}`);
export const startLogin = (provider: Provider, distro: string | null, target: LoginTarget) => call<LoginId>('POST', `/api/accounts/${provider}${query(at(distro))}`, target);
export const logoutSystem = (provider: Provider, distro: string | null) => call<null>('POST', `/api/accounts/${provider}/logout${query(at(distro))}`);
/** 설정 「휴대폰」 탭(docs/design.md 「원격」). 이 기계의 화면에서만 부른다. 폰이 부르면 403이다. */
export const getPhone = () => call<PhoneView>('GET', '/api/phone');
/** 데몬이 `tailscale serve --bg <포트>`를 돌린다. 사람이 단추를 누를 때만 부른다. */
export const startServe = () => call<PhoneView>('POST', '/api/phone/serve');
/** 새 짝짓기 코드. 앞 코드는 쓸 수 없게 된다. */
export const createInvite = () => call<InviteView>('POST', '/api/phone/invite');
export const removeDevice = (id: DeviceId) => call<null>('DELETE', `/api/phone/devices/${id}`);
export const getLogin = (id: LoginId) => call<LoginState>('GET', `/api/logins/${id}`);
export const closeLogin = (id: LoginId) => call<null>('DELETE', `/api/logins/${id}`);
export const loginPty = (id: LoginId) => `/api/logins/${id}/pty`;
export const nodePty = (id: NodeId) => `/api/nodes/${id}/pty`;

function socketUrl(path: string): string {
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}${path}`;
}

/**
 * 그래프 이벤트를 받는다. 연결할 때마다 노드 목록 전체(`snapshot`)를 먼저 받는다. 반환값은 구독 해제 함수다.
 * `onLink`는 연결이 열리고 끊길 때 부른다. 폰 화면이 끊김 줄을 보인다(목업 docs/mockups/mobile.html ⑥).
 */
export function watchGraph(onEvent: (event: GraphEvent) => void, onLink: (up: boolean) => void = () => {}): () => void {
  let socket: WebSocket;
  let stopped = false;
  const open = () => {
    socket = new WebSocket(socketUrl('/api/events'));
    socket.onopen = () => onLink(true);
    socket.onmessage = (e) => onEvent(JSON.parse(e.data) as GraphEvent);
    socket.onclose = () => {
      if (stopped) return;
      onLink(false);
      setTimeout(open, RECONNECT_MS);
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
/** 노드를 지금 설정으로 다시 띄우고 같은 대화를 잇는다. 턴 중이면 턴이 끝난 뒤 띄운다(docs/design.md 「다시 띄우기」). */
export const relaunchNode = (id: NodeId) => call<Relaunched>('POST', `/api/nodes/${id}/relaunch`);
/** 그 노드가 턴 끝에 다시 뜰 차례다. */
export const relaunchQueued = (id: NodeId) => call<boolean>('GET', `/api/nodes/${id}/relaunch`);
