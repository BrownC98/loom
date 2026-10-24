import i18next from 'i18next';
import type { ReactNode } from 'react';
import { I18nextProvider, initReactI18next } from 'react-i18next';
import en from './locales/en';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import type { SettingsView } from './gen/SettingsView';

const i18n = i18next.createInstance();
void i18n.use(initReactI18next).init({ lng: 'en', resources: { en: { translation: en } } });

export const Providers = (props: { children: ReactNode }) => <I18nextProvider i18n={i18n}>{props.children}</I18nextProvider>;

export type Deferred<T> = { promise: Promise<T>; resolve: (value: T) => void; reject: (error: Error) => void };

export function deferred<T = null>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

export const WORKTREE = '/repo';

export const repo: RepoView = { id: 1, name: 'repo', path: WORKTREE, worktrees: [{ path: WORKTREE, head: 'abc', branch: 'main', original: true }], branches: ['main'], hasCommits: true, wsl: null } as RepoView;

export const nodeView = (id: number, name: string, extra: Partial<NodeView> = {}): NodeView =>
  ({ id, name, agent: null, parent: null, state: { kind: 'running' }, unread: 0, model: null, effort: null, place: { repo: 1, worktree: WORKTREE }, bookmark: null, cardView: null, reply: null, waits: [], ...extra }) as NodeView;

export const settings = { cardView: 'standard', edgeStyle: 'step', minimapFolded: true, alwaysArrange: false, workspaceDir: null } as SettingsView;
