import { AlignLeft, GitCommitVertical, PanelRightClose, PanelRightOpen, SquareTerminal, Workflow, type LucideIcon } from 'lucide-react';
import type { KeyboardEvent, ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { shortcut } from './platform';

/** 오른쪽 사이드바 아래 칸. 한 번에 하나만 보인다. 터미널은 심플 보기에서 노드를 골랐을 때만 있다. */
export type DockTab = 'events' | 'commits' | 'terminal';

const ICON: Record<DockTab, LucideIcon> = { events: AlignLeft, commits: GitCommitVertical, terminal: SquareTerminal };
const LABEL: Record<DockTab, 'events.title' | 'app.commitGraph' | 'dock.terminal'> = { events: 'events.title', commits: 'app.commitGraph', terminal: 'dock.terminal' };

/** 접기·펴기 단추의 id. 펼친 사이드바와 접힌 줄에 하나씩만 있어, 누른 뒤 같은 id로 포커스를 옮긴다. */
export const DOCK_TOGGLE = 'dock-toggle';
/** 접기·펴기 키(Terminal.tsx `paneKey`). 툴팁과 `aria-keyshortcuts`에 보인다. */
const KEY = shortcut(']');

/**
 * 오른쪽 사이드바(목업 docs/mockups/toolbar.html E안). 위 「보기」에 그래프 보기 도구(`tools`)를 두고, 아래 탭이 칸(`children`)을 고른다.
 */
export function Dock(props: { tabs: readonly DockTab[]; tab: DockTab; onTab: (tab: DockTab) => void; onCollapse: () => void; tools: ReactNode; children: ReactNode }) {
  const { t } = useTranslation();
  const { tabs } = props;
  // 탭 목록은 ←·→로 옆 탭을 고른다(WAI-ARIA 탭 패턴).
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
            <button key={tab} id={`dock-tab-${tab}`} role="tab" aria-selected={on} aria-controls="dock-panel" tabIndex={on ? 0 : -1} onClick={() => props.onTab(tab)}>
              <Icon />
              {t(LABEL[tab])}
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

/** 접힌 오른쪽 사이드바: 펴기, 지금 방향으로 정렬, 칸 아이콘. 칸 아이콘은 그 탭으로 편다. */
export function DockRail(props: { tabs: readonly DockTab[]; onExpand: (tab: DockTab | null) => void; onArrange: () => void }) {
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
        return (
          <button key={tab} className="btn ghost icon" aria-label={t(LABEL[tab])} title={t(LABEL[tab])} onClick={() => props.onExpand(tab)}>
            <Icon />
          </button>
        );
      })}
    </aside>
  );
}
