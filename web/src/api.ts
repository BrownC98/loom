// 데몬 API. 요청·응답 타입은 Rust가 원천이다(gen/).
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { CommitGraph } from './gen/CommitGraph';
import type { GraphEvent } from './gen/GraphEvent';
import type { Login } from './gen/Login';
import type { LoginId } from './gen/LoginId';
import type { LoginState } from './gen/LoginState';
import type { NewNode } from './gen/NewNode';
import type { NodeId } from './gen/NodeId';
import type { Problem } from './gen/Problem';
import type { Provider } from './gen/Provider';
import type { PtyNotice } from './gen/PtyNotice';
import type { RepoView } from './gen/RepoView';
import type { Resize } from './gen/Resize';
import type { SettingsView } from './gen/SettingsView';
import { problemText } from './problem';

/** 끊긴 WebSocket을 다시 여는 간격. 데몬이 다시 시작하는 동안 기다린다. */
const RECONNECT_MS = 1000;

export async function getRepo(): Promise<RepoView> {
  const res = await fetch('/api/repo');
  return res.json();
}

/** 실패하면 데몬이 보낸 코드(`Problem`)를 현재 언어로 바꾼 문장을 담은 Error를 던진다. */
async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    headers: body === undefined ? undefined : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new Error(problemText((await res.json()) as Problem));
  return res.json();
}

export const createNode = (req: NewNode) => call<NodeId>('POST', '/api/nodes', req);
/** 노드 하나를 지운다. 자식은 부모 없이 남는다(docs/design.md 「노드 제거와 입양」). */
export const removeNode = (id: NodeId) => call<null>('DELETE', `/api/nodes/${id}`);
/** `parent`가 `child`의 새 부모가 된다. 거절하면 데몬의 문장을 담은 Error를 던진다. */
export const adoptNode = (child: NodeId, parent: NodeId) => call<null>('PUT', `/api/nodes/${child}/parent/${parent}`);
/** 부를 때마다 데몬이 PATH를 다시 훑는다. */
export const getAgents = () => call<AgentView[]>('GET', '/api/agents');
export const getSettings = () => call<SettingsView>('GET', '/api/settings');
export const putSettings = (view: SettingsView) => call<null>('PUT', '/api/settings', view);
/** 모든 브랜치의 최근 커밋과 워크트리마다 체크아웃한 커밋. */
export const getCommits = () => call<CommitGraph>('GET', '/api/commits');
/** 계정마다 사용량을 그 자리에서 읽는다. 몇 초 걸린다. */
export const getAccounts = () => call<AccountView[]>('GET', '/api/accounts');
export const removeAccount = (provider: Provider, id: AccountId) => call<null>('DELETE', `/api/accounts/${provider}/${id}`);
export const startLogin = (provider: Provider, req: Login) => call<LoginId>('POST', `/api/accounts/${provider}`, req);
export const getLogin = (id: LoginId) => call<LoginState>('GET', `/api/logins/${id}`);
export const closeLogin = (id: LoginId) => call<null>('DELETE', `/api/logins/${id}`);
export const loginPty = (id: LoginId) => `/api/logins/${id}/pty`;
export const nodePty = (id: NodeId) => `/api/nodes/${id}/pty`;

function socketUrl(path: string): string {
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}${path}`;
}

/** 그래프 이벤트를 받는다. 연결할 때마다 노드 목록 전체를 노드 이벤트로 먼저 받는다. 반환값은 구독 해제 함수다. */
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
