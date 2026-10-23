import { AlignLeft, Bell, FileText, GitCommitVertical, PanelRightClose, PanelRightOpen, SquareTerminal, Workflow, type LucideIcon } from 'lucide-react';
import type { KeyboardEvent, ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { shortcut } from './platform';

export type DockTab = 'events' | 'commits' | 'docs' | 'questions' | 'terminal';

const ICON: Record<DockTab, LucideIcon> = { events: AlignLeft, commits: GitCommitVertical, docs: FileText, questions: Bell, terminal: SquareTerminal };
const LABEL: Record<DockTab, 'events.title' | 'app.commitGraph' | 'docs.title' | 'inbox.tab' | 'dock.terminal'> = {
  events: 'events.title',
  commits: 'app.commitGraph',
  docs: 'docs.title',
  questions: 'inbox.tab',
  terminal: 'dock.terminal',
};

function Count(props: { tab: DockTab; waiting: number }) {
  return props.tab === 'questions' && props.waiting > 0 ? <span className="count">{props.waiting}</span> : null;
}

export const DOCK_TOGGLE = 'dock-toggle';
const KEY = shortcut(']');

export function Dock(props: { tabs: readonly DockTab[]; tab: DockTab; waiting: number; onTab: (tab: DockTab) => void; onCollapse: () => void; tools: ReactNode; children: ReactNode }) {
  const { t } = useTranslation();
  const { tabs } = props;
  const onKey = (e: KeyboardEvent) => {
    const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    const next = tabs[(tabs.indexOf(props.tab) + step + tabs.length) % tabs.length];
    props.onTab(next);
    document.getElementById(`dock-tab-${next}`)?.focus();
  };
  return (
    <aside className="dock" id="dock" aria-label={t('dock.label')}>
      <div className="dock-tools">
        <div className="dock-tools-head">
          <h2>{t('dock.view')}</h2>
          <button id={DOCK_TOGGLE} className="btn ghost sm icon" aria-expanded aria-controls="dock" aria-label={t('dock.collapse')} aria-keyshortcuts={KEY.aria} title={`${t('dock.collapse')} (${KEY.label})`} onClick={props.onCollapse}>
            <PanelRightClose />
          </button>
        </div>
        {props.tools}
      </div>
      <div className="tabs" role="tablist" aria-label={t('dock.tabs')} onKeyDown={onKey}>
        {tabs.map((tab) => {
          const Icon = ICON[tab];
          const on = tab === props.tab;
          return (
            <button key={tab} id={`dock-tab-${tab}`} role="tab" aria-selected={on} aria-controls="dock-panel" tabIndex={on ? 0 : -1} title={t(LABEL[tab])} onClick={() => props.onTab(tab)}>
              <Icon />
              <span className="tab-label">{t(LABEL[tab])}</span>
              <Count tab={tab} waiting={props.waiting} />
            </button>
          );
        })}
      </div>
      <div className="dock-panel" id="dock-panel" role="tabpanel" aria-labelledby={`dock-tab-${props.tab}`}>
        {props.children}
      </div>
    </aside>
  );
}

export function DockRail(props: { tabs: readonly DockTab[]; waiting: number; onExpand: (tab: DockTab | null) => void; onArrange: () => void }) {
  const { t } = useTranslation();
  return (
    <aside className="rail dock-rail" id="dock" aria-label={t('dock.label')}>
      <button id={DOCK_TOGGLE} className="btn ghost icon" aria-expanded={false} aria-controls="dock" aria-label={t('dock.expand')} aria-keyshortcuts={KEY.aria} title={`${t('dock.expand')} (${KEY.label})`} onClick={() => props.onExpand(null)}>
        <PanelRightOpen />
      </button>
      <hr />
      <button className="btn ghost icon" aria-label={t('graph.arrange')} title={t('graph.arrange')} onClick={props.onArrange}>
        <Workflow />
      </button>
      <hr />
      {props.tabs.map((tab) => {
        const Icon = ICON[tab];
        const label = tab === 'questions' && props.waiting > 0 ? `${t(LABEL[tab])} ${props.waiting}` : t(LABEL[tab]);
        return (
          <button key={tab} className="btn ghost icon" aria-label={label} title={label} onClick={() => props.onExpand(tab)}>
            <Icon />
            <Count tab={tab} waiting={props.waiting} />
          </button>
        );
      })}
    </aside>
  );
}
