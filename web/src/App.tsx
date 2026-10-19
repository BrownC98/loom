import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { ReactFlowProvider } from '@xyflow/react';
import { useTranslation } from 'react-i18next';
import { getAccounts, getAgents, getRepo, getSettings, putSettings, watchGraph } from './api';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import { CommitGraph } from './CommitGraph';
import { EventConsole, involved, useEventLog } from './EventConsole';
import type { EventRecord } from './gen/EventRecord';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import type { SettingsView } from './gen/SettingsView';
import type { Theme } from './gen/Theme';
import { Graph } from './Graph';
import { applyLanguage, INITIAL_LANGUAGE } from './i18n';
import { Settings, SETTINGS_TABS, type SettingsTab } from './Settings';
import { StatusBar, type Usage } from './StatusBar';
import { activeTheme, RESET_TERMINAL_THEME, type ColorMode } from './terminalThemes';
import { Tree } from './Tree';
import { XtermTheme } from './Terminal';
import { withAccount } from './usage';

/** 메시지가 오간 간선을 강조하는 시간. 마지막 메시지부터 잰다. */
const PULSE_MS = 3000;
/** 사용량을 다시 읽는 간격. 근거: docs/design.md 「계정과 사용량」. */
const USAGE_REFRESH_MS = 5 * 60 * 1000;
const EMPTY_SETTINGS: SettingsView = {
  defaultAgent: null,
  disabledAgents: [],
  claudeAccount: null,
  codexAccount: null,
  // 데몬이 첫 페이지에 넣은 테마(`Theme`의 serde 이름)다. 설정을 받기 전에도 같은 테마를 그린다.
  theme: (document.documentElement.dataset.theme as Theme | undefined) ?? 'system',
  terminalTheme: RESET_TERMINAL_THEME,
  language: INITIAL_LANGUAGE,
};
const SETTINGS_HASH = '#settings/';

/** 열린 설정 탭. URL 해시(`#settings/agents`)가 원천이다. */
function useSettingsTab(): [SettingsTab | null, (tab: SettingsTab | null) => void] {
  const read = (): SettingsTab | null => {
    const tab = location.hash.startsWith(SETTINGS_HASH) ? location.hash.slice(SETTINGS_HASH.length) : null;
    return SETTINGS_TABS.find((t) => t === tab) ?? null;
  };
  const [tab, setTab] = useState(read);
  useEffect(() => {
    const onHash = () => setTab(read());
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);
  const go = (next: SettingsTab | null) => {
    history.replaceState(null, '', next ? SETTINGS_HASH + next : location.pathname + location.search);
    setTab(next);
  };
  return [tab, go];
}

const DARK_QUERY = '(prefers-color-scheme: dark)';

/** 화면이 그리는 모드. 「시스템」이면 OS 설정이 바뀔 때 바로 따라간다. */
function useColorMode(theme: Theme): ColorMode {
  const systemDark = useSyncExternalStore(
    (onChange) => {
      const query = matchMedia(DARK_QUERY);
      query.addEventListener('change', onChange);
      return () => query.removeEventListener('change', onChange);
    },
    () => matchMedia(DARK_QUERY).matches,
  );
  return theme === 'system' ? (systemDark ? 'dark' : 'light') : theme;
}

export function App() {
  const { t } = useTranslation();
  const [repo, setRepo] = useState<RepoView | null>(null);
  const [nodes, setNodes] = useState<ReadonlyMap<NodeId, NodeView>>(new Map());
  // 자식 id → 강조하는 동안 그 자식의 간선으로 오간 메시지 수.
  const [pulses, setPulses] = useState<ReadonlyMap<NodeId, number>>(new Map());
  const pulseTimers = useRef(new Map<NodeId, number>());
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [settings, setSettings] = useState<SettingsView>(EMPTY_SETTINGS);
  const [accounts, setAccounts] = useState<AccountView[] | null>(null);
  const [fetchedAt, setFetchedAt] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [settingsTab, setSettingsTab] = useSettingsTab();
  const [selected, setSelected] = useState<NodeId | null>(null);
  // 오른쪽 칸 하나를 이벤트 콘솔과 커밋 그래프가 번갈아 쓴다.
  const [dock, setDock] = useState<'events' | 'commits' | null>(null);
  const toggleDock = (next: 'events' | 'commits') => setDock(dock === next ? null : next);
  const log = useEventLog(dock === 'events');
  // 이벤트 콘솔에서 고른 이벤트. 칸을 닫으면 끈다.
  const [highlight, setHighlight] = useState<EventRecord | null>(null);
  useEffect(() => {
    if (dock !== 'events') setHighlight(null);
  }, [dock]);
  const mode = useColorMode(settings.theme);

  useEffect(() => {
    // styles.css가 `data-theme`으로 `color-scheme`을 고른다.
    document.documentElement.dataset.theme = settings.theme;
  }, [settings.theme]);
  useEffect(() => applyLanguage(settings.language), [settings.language]);

  const refreshUsage = useCallback(async () => {
    setLoading(true);
    try {
      setAccounts(await getAccounts());
      setFetchedAt(Date.now());
    } finally {
      setLoading(false);
    }
  }, []);
  const rescan = () => getAgents().then(setAgents);
  // 화면이 먼저 바뀌고 저장은 뒤따른다. 저장이 실패하면 데몬 값으로 되돌린다.
  const saveSettings = (next: SettingsView) => {
    setSettings(next);
    putSettings(next).catch(() => getSettings().then(setSettings));
  };
  const usage: Usage = { accounts, fetchedAt, loading, refresh: refreshUsage };

  useEffect(() => {
    rescan();
    getSettings().then(setSettings);
    refreshUsage();
    const timer = window.setInterval(refreshUsage, USAGE_REFRESH_MS);
    return () => clearInterval(timer);
  }, [refreshUsage]);

  useEffect(() => {
    // 설정 창을 열 때 PATH를 다시 훑는다(목업 3번).
    if (settingsTab !== null) rescan();
  }, [settingsTab !== null]);

  useEffect(() => {
    getRepo().then(setRepo);
    const pulse = (child: NodeId) => {
      setPulses((prev) => new Map(prev).set(child, (prev.get(child) ?? 0) + 1));
      clearTimeout(pulseTimers.current.get(child));
      const timer = window.setTimeout(() => {
        setPulses((prev) => {
          const next = new Map(prev);
          next.delete(child);
          return next;
        });
      }, PULSE_MS);
      pulseTimers.current.set(child, timer);
    };
    return watchGraph((event) => {
      switch (event.kind) {
        case 'message':
          pulse(event.child);
          break;
        case 'removed':
          setNodes((prev) => {
            const next = new Map(prev);
            next.delete(event.id);
            return next;
          });
          break;
        case 'node':
          setNodes((prev) => new Map(prev).set(event.id, event));
          break;
        case 'recorded':
          // `onRecord`는 ref와 setState만 쓴다. 처음 만든 것을 계속 불러도 된다.
          log.onRecord(event.record);
      }
    });
  }, []);

  return (
    <XtermTheme.Provider value={activeTheme(settings.terminalTheme, mode)}>
      <ReactFlowProvider>
        <div className="app" data-dock={dock ?? undefined}>
          <nav className="side" aria-label={t('app.sideLabel')}>
            <div className="brand">loom</div>
            {repo && <Tree repo={repo} views={nodes} agents={agents} selected={selected} onSelect={setSelected} />}
            <div className="side-foot">
              <button className="btn ghost" onClick={() => setSettingsTab('agents')}>
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <circle cx="12" cy="12" r="3" />
                  <path d="M12 2v3M12 19v3M4.2 4.2l2.1 2.1M17.7 17.7l2.1 2.1M2 12h3M19 12h3M4.2 19.8l2.1-2.1M17.7 6.3l2.1-2.1" />
                </svg>
                {t('settings.title')}
              </button>
            </div>
          </nav>
          <main className="canvas" aria-label={t('app.graphLabel', { repo: repo?.name ?? '' })}>
            <div className="toolbar">
              <h1>{repo?.name}</h1>
              <span className="spacer" />
              <button className="btn" aria-pressed={dock === 'events'} aria-controls="dock" onClick={() => toggleDock('events')}>
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M4 6h16M4 12h16M4 18h10" />
                </svg>
                {t('events.title')}
              </button>
              <button className="btn" aria-pressed={dock === 'commits'} aria-controls="dock" onClick={() => toggleDock('commits')}>
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <circle cx="12" cy="12" r="3" />
                  <path d="M12 3v6M12 15v6" />
                </svg>
                {t('app.commitGraph')}
              </button>
            </div>
            <Graph
              views={nodes}
              pulses={pulses}
              agents={agents}
              settings={settings}
              accounts={accounts}
              selected={selected}
              onSelect={setSelected}
              highlight={highlight && new Set(involved(highlight.entry))}
              onClearHighlight={() => setHighlight(null)}
            />
          </main>
          {dock === 'events' && <EventConsole records={log.records} views={nodes} agents={agents} highlight={highlight} onHighlight={setHighlight} onClose={() => setDock(null)} />}
          {dock === 'commits' && <CommitGraph onClose={() => setDock(null)} />}
          <StatusBar
            usage={usage}
            settings={settings}
            onChoose={(provider, id) => saveSettings(withAccount(settings, provider, id))}
            onManage={() => setSettingsTab('accounts')}
          />
          {settingsTab && (
            <Settings
              tab={settingsTab}
              onTab={setSettingsTab}
              onClose={() => setSettingsTab(null)}
              agents={agents}
              onRescan={rescan}
              settings={settings}
              onSettings={saveSettings}
              usage={usage}
            />
          )}
        </div>
      </ReactFlowProvider>
    </XtermTheme.Provider>
  );
}
