import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { CursorView } from './gen/CursorView';
import type { AgentView } from './gen/AgentView';
import type { DocRow } from './gen/DocRow';
import type { AnswerWait } from './gen/AnswerWait';
import type { ChooseOption } from './gen/ChooseOption';
import type { Prompt } from './gen/Prompt';
import type { TypeText } from './gen/TypeText';
import type { CardSize } from './gen/CardSize';
import type { CardView } from './gen/CardView';
import type { CloneRepo } from './gen/CloneRepo';
import type { CommitGraph } from './gen/CommitGraph';
import type { CommitRange } from './gen/CommitRange';
import type { CreateRepo } from './gen/CreateRepo';
import type { EventLog } from './gen/EventLog';
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
import type { Served } from './gen/Served';
import type { Problem } from './gen/Problem';
import type { Provider } from './gen/Provider';
import type { PtyNotice } from './gen/PtyNotice';
import type { PtyRequest } from './gen/PtyRequest';
import type { Relaunched } from './gen/Relaunched';
import type { RenameNode } from './gen/RenameNode';
import type { RenameWorktree } from './gen/RenameWorktree';
import type { RepoId } from './gen/RepoId';
import type { RepoView } from './gen/RepoView';
import type { SettingsView } from './gen/SettingsView';
import type { UpdateView } from './gen/UpdateView';
import type { SettingsPatch } from './settingsPatch';
import type { WaitId } from './gen/WaitId';
import { problemText, statusText } from './problem';
import { QUIET_LIMIT_MS, reconnecting } from './link';

async function request<T>(method: string, path: string, body?: { type: string; data: BodyInit }): Promise<T> {
  const res = await fetch(path, {
    method,
    headers: body === undefined ? undefined : { 'content-type': body.type },
    body: body?.data,
  });
  if (!res.ok) {
    const problem = (await res.json().catch(() => null)) as Problem | null;
    throw new Error(problem ? problemText(problem) : statusText(res.status));
  }
  return res.json();
}

const call = <T>(method: string, path: string, body?: unknown) =>
  request<T>(method, path, body === undefined ? undefined : { type: 'application/json', data: JSON.stringify(body) });

export const PASTE_LIMIT_BYTES = 32 * 1024 * 1024;

export const savePastedFile = (file: File, node: NodeId | null) =>
  request<string>('POST', `/api/pastes/${encodeURIComponent(file.name)}${node === null ? '' : `?node=${node}`}`, {
    type: file.type || 'application/octet-stream',
    data: file,
  });

export const createNode = (req: NewNode) => call<NodeId>('POST', '/api/nodes', req);
export const removeNode = (id: NodeId) => call<null>('DELETE', `/api/nodes/${id}`);
export const adoptNode = (child: NodeId, parent: NodeId) => call<null>('PUT', `/api/nodes/${child}/parent/${parent}`);
export const bookmarkNode = (id: NodeId) => call<null>('PUT', `/api/nodes/${id}/bookmark`);
export const steerQueued = (id: NodeId) => call<null>('POST', `/api/nodes/${id}/steer-queued`);
export const unbookmarkNode = (id: NodeId) => call<null>('DELETE', `/api/nodes/${id}/bookmark`);
export const setCardView = (id: NodeId, view: CardView) => call<null>('PUT', `/api/nodes/${id}/card-view`, view);
export const clearCardView = (id: NodeId) => call<null>('DELETE', `/api/nodes/${id}/card-view`);
export const dismissWait = (wait: WaitId) => call<null>('DELETE', `/api/waits/${wait}`);
export const answerWait = (wait: WaitId, text: string, images: string[]) =>
  call<null>('POST', `/api/waits/${wait}/answer`, { text, images } satisfies AnswerWait);
export const chooseOption = (wait: WaitId, prompt: Prompt, option: number) =>
  call<null>('POST', `/api/waits/${wait}/choose`, { prompt, option } satisfies ChooseOption);
export const typeText = (wait: WaitId, prompt: Prompt, text: string) =>
  call<null>('POST', `/api/waits/${wait}/type`, { prompt, text } satisfies TypeText);
export const listDocs = () => call<DocRow[]>('GET', '/api/docs');
export const openDoc = (id: string) => call<null>('POST', `/api/docs/${encodeURIComponent(id)}`);
export const docUrl = (id: string) => `/api/docs/${encodeURIComponent(id)}/`;
export const doneImageUrl = (wait: WaitId, index: number) => `/api/waits/${wait}/images/${index}`;
export const renameNode = (id: NodeId, name: string) => call<null>('PUT', `/api/nodes/${id}/name`, { name } satisfies RenameNode);

export async function shutdown(): Promise<void> {
  const res = await fetch('/api/shutdown', { method: 'POST' });
  if (!res.ok) throw new Error(statusText(res.status));
}

export function fileViewUrl(path: string, node: NodeId | null): string {
  const parts = path.replace(/^\//, '').split('/').map(encodeURIComponent);
  return `/api/files/view/${node ?? '-'}/${parts.join('/')}`;
}

export const filePass = (path: string, node: NodeId | null) => call<string>('POST', '/api/files/pass', { path, node } satisfies OpenFile);
export const docPass = (id: string) => call<string>('POST', `/api/docs/${encodeURIComponent(id)}/pass`);
export const openFile = (path: string, node: NodeId | null) => call<null>('POST', '/api/files/open', { path, node } satisfies OpenFile);
export const getAgents = () => call<AgentView[]>('GET', '/api/agents');
export const getSettings = () => call<SettingsView>('GET', '/api/settings');
export const getDefaultSettings = () => call<SettingsView>('GET', '/api/settings/default');
export const putSettings = (patch: SettingsPatch) => call<null>('PUT', '/api/settings', patch);
export const getUpdate = () => call<UpdateView>('GET', '/api/update');
export const checkUpdate = () => call<UpdateView>('POST', '/api/update/check');
export const installUpdate = () => call<UpdateView>('POST', '/api/update/install');
export const getCommits = (repo: RepoId, range: CommitRange) =>
  call<CommitGraph>('GET', `/api/repos/${repo}/commits?skip=${range.skip}&limit=${range.limit}`);
export const getRepos = () => call<RepoView[]>('GET', '/api/repos');
export const getSpots = () => call<Layout>('GET', '/api/spots');
export const putSpots = (placed: Placed[]) => call<null>('PUT', '/api/spots', placed);
export const putArranged = (arranged: Arranged) => call<null>('PUT', '/api/spots/arranged', arranged);
export const clearSpots = (repo: RepoId) => call<null>('DELETE', `/api/repos/${repo}/spots`);
export const getSizes = () => call<CardSize[]>('GET', '/api/sizes');
export const putSizes = (sizes: CardSize[]) => call<null>('PUT', '/api/sizes', sizes);
export const clearSizes = (nodes: NodeId[]) => call<null>('DELETE', '/api/sizes', nodes);
export const addRepo = (req: NewRepo) => call<RepoView>('POST', '/api/repos', req);
export const cloneRepo = (req: CloneRepo) => call<RepoView>('POST', '/api/repos/clone', req);
export const createRepo = (req: CreateRepo) => call<RepoView>('POST', '/api/repos/create', req);
export const removeRepo = (repo: RepoId) => call<null>('DELETE', `/api/repos/${repo}`);
export const getLocations = () => call<Locations>('GET', '/api/locations');
export const getDistro = (name: string) => call<DistroView>('GET', `/api/distros/${encodeURIComponent(name)}`);
export const getFolders = (path?: string) => call<FolderList>('GET', path === undefined ? '/api/folders' : `/api/folders?${new URLSearchParams({ path })}`);
export const createWorktree = (repo: RepoId, req: NewWorktree) => call<RepoView>('POST', `/api/repos/${repo}/worktrees`, req);
const worktreeQuery = (path: string) => `path=${encodeURIComponent(path)}`;
export const renameWorktree = (repo: RepoId, path: string, name: string) => call<RepoView>('PUT', `/api/repos/${repo}/worktrees/name`, { path, name } satisfies RenameWorktree);
export const getWorktreeChanges = (repo: RepoId, path: string) => call<string[]>('GET', `/api/repos/${repo}/worktrees/changes?${worktreeQuery(path)}`);
export const deleteWorktree = (repo: RepoId, path: string, deleteBranch: boolean) =>
  call<RepoView>('DELETE', `/api/repos/${repo}/worktrees?${worktreeQuery(path)}&deleteBranch=${deleteBranch}`);
export const getEventLog = () => call<EventLog>('GET', '/api/events/log');
export const getAccounts = () => call<AccountView[]>('GET', '/api/accounts');
export const getCursorUsage = () => call<CursorView[]>('GET', '/api/usage/cursor');
const at = (distro: string | null): Record<string, string> => (distro === null ? {} : { distro });
const query = (params: Record<string, string>) => (Object.keys(params).length === 0 ? '' : `?${new URLSearchParams(params)}`);
export const getMovableNodes = (provider: Provider, distro: string | null, to: AccountId | null) =>
  call<MovableNode[]>('GET', `/api/accounts/${provider}/nodes${query({ ...at(distro), ...(to === null ? {} : { to }) })}`);
export const moveNodes = (provider: Provider, distro: string | null) => call<MoveResult>('POST', `/api/accounts/${provider}/move${query(at(distro))}`);
export const removeAccount = (provider: Provider, distro: string | null, id: AccountId) => call<null>('DELETE', `/api/accounts/${provider}/${id}${query(at(distro))}`);
export const startLogin = (provider: Provider, distro: string | null, target: LoginTarget) => call<LoginId>('POST', `/api/accounts/${provider}${query(at(distro))}`, target);
export const logoutSystem = (provider: Provider, distro: string | null) => call<null>('POST', `/api/accounts/${provider}/logout${query(at(distro))}`);
export const getPhone = () => call<PhoneView>('GET', '/api/phone');
export const startServe = () => call<Served>('POST', '/api/phone/serve');
export const allowFirewall = () => call<null>('POST', '/api/phone/firewall');
export const createInvite = () => call<InviteView>('POST', '/api/phone/invite');
export const removeDevice = (id: DeviceId) => call<null>('DELETE', `/api/phone/devices/${id}`);
export const getLogin = (id: LoginId) => call<LoginState>('GET', `/api/logins/${id}`);
export const closeLogin = (id: LoginId) => call<null>('DELETE', `/api/logins/${id}`);
export const loginPty = (id: LoginId) => `/api/logins/${id}/pty`;
export const nodePty = (id: NodeId) => `/api/nodes/${id}/pty`;

function socketUrl(path: string): string {
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}${path}`;
}

export function watchGraph(onEvent: (event: GraphEvent) => void, onLink: (up: boolean) => void = () => {}): () => void {
  const link = reconnecting(socketUrl('/api/events'), {
    onOpen: () => onLink(true),
    onMessage: (e) => onEvent(JSON.parse(e.data) as GraphEvent),
    onDrop: () => onLink(false),
    quietLimitMs: QUIET_LIMIT_MS,
  });
  return link.close;
}

export async function newBuildServed(): Promise<boolean> {
  const html = await (await fetch('/', { cache: 'no-store' })).text();
  const entry = (doc: Document) => doc.querySelector('script[type="module"][src]')?.getAttribute('src');
  return entry(new DOMParser().parseFromString(html, 'text/html')) !== entry(document);
}

export interface PtyLink {
  sendInput(data: string | Uint8Array): void;
  sendRequest(request: PtyRequest): void;
  close(): void;
}

export function attachPty(
  path: string,
  handlers: {
    onAttach: () => void;
    onBacklog: (bytes: Uint8Array) => void;
    onOutput: (bytes: Uint8Array) => void;
    onNotice: (notice: PtyNotice) => void;
  },
): PtyLink {
  const encoder = new TextEncoder();
  let backlogNext = false;
  const link = reconnecting(socketUrl(path), {
    onOpen: () => {
      backlogNext = true;
      handlers.onAttach();
    },
    onMessage: (e) => {
      if (typeof e.data === 'string') return handlers.onNotice(JSON.parse(e.data) as PtyNotice);
      const bytes = new Uint8Array(e.data as ArrayBuffer);
      if (backlogNext) {
        backlogNext = false;
        handlers.onBacklog(bytes);
      } else handlers.onOutput(bytes);
    },
  });
  return {
    sendInput: (data) => link.send(typeof data === 'string' ? encoder.encode(data) : new Uint8Array(data)),
    sendRequest: (request) => link.send(JSON.stringify(request)),
    close: link.close,
  };
}
export const relaunchNode = (id: NodeId) => call<Relaunched>('POST', `/api/nodes/${id}/relaunch`);
