// 데몬 API. 요청·응답 타입은 Rust가 원천이다(gen/).
import type { NewNode } from './gen/NewNode';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import type { Resize } from './gen/Resize';

/** 끊긴 WebSocket을 다시 여는 간격. 데몬이 다시 시작하는 동안 기다린다. */
const RECONNECT_MS = 1000;

export async function getRepo(): Promise<RepoView> {
  const res = await fetch('/api/repo');
  return res.json();
}

/** 실패하면 데몬이 보낸 문장을 담은 Error를 던진다. */
export async function createNode(req: NewNode): Promise<NodeId> {
  const res = await fetch('/api/nodes', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(req),
  });
  if (!res.ok) throw new Error(await res.text());
  return res.json();
}

function socketUrl(path: string): string {
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}${path}`;
}

/** 노드 상태 이벤트를 받는다. 연결할 때마다 노드 목록 전체를 먼저 받는다. 반환값은 구독 해제 함수다. */
export function watchNodes(onNode: (view: NodeView) => void): () => void {
  let socket: WebSocket;
  let stopped = false;
  const open = () => {
    socket = new WebSocket(socketUrl('/api/events'));
    socket.onmessage = (e) => onNode(JSON.parse(e.data) as NodeView);
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

/** 노드 PTY에 붙는다. 연결할 때마다 `onAttach`를 부른 뒤 backlog부터 출력을 넘긴다. */
export function attachPty(
  id: NodeId,
  handlers: { onAttach: () => void; onOutput: (bytes: Uint8Array) => void },
): PtyLink {
  const encoder = new TextEncoder();
  let socket: WebSocket;
  let stopped = false;
  const open = () => {
    socket = new WebSocket(socketUrl(`/api/nodes/${id}/pty`));
    socket.binaryType = 'arraybuffer';
    socket.onopen = handlers.onAttach;
    socket.onmessage = (e) => handlers.onOutput(new Uint8Array(e.data as ArrayBuffer));
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
