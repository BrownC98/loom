import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { getAccounts, getAgents, getRepo, getSettings, putSettings, watchGraph } from './api';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { RepoView } from './gen/RepoView';
import type { SettingsView } from './gen/SettingsView';
import type { Theme } from './gen/Theme';
import { Graph } from './Graph';
import { Settings, SETTINGS_TABS, type SettingsTab } from './Settings';
import { StatusBar, type Usage } from './StatusBar';
import { activeTheme, RESET_TERMINAL_THEME, type ColorMode } from './terminalThemes';
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
  const mode = useColorMode(settings.theme);

  useEffect(() => {
    // styles.css가 `data-theme`으로 `color-scheme`을 고른다.
    document.documentElement.dataset.theme = settings.theme;
  }, [settings.theme]);

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
      if (event.kind === 'message') pulse(event.child);
      else setNodes((prev) => new Map(prev).set(event.id, event));
    });
  }, []);

  return (
    <XtermTheme.Provider value={activeTheme(settings.terminalTheme, mode)}>
      <div className="app">
        <nav className="side" aria-label="저장소">
          <div className="brand">loom</div>
          <h2>저장소</h2>
          {repo && (
            <ul className="repos">
              <li>
                <button className="repo" aria-current="page">
                  <b>{repo.name}</b>
                  <span translate="no">{repo.path}</span>
                </button>
              </li>
            </ul>
          )}
        </nav>
        <main className="canvas" aria-label={`${repo?.name ?? ''} 그래프`}>
          <div className="toolbar">
            <h1>{repo?.name}</h1>
            <span className="path" translate="no">
              {repo?.path}
            </span>
          </div>
          <Graph views={nodes} pulses={pulses} agents={agents} settings={settings} accounts={accounts} />
        </main>
        <StatusBar
          usage={usage}
          settings={settings}
          onChoose={(provider, id) => saveSettings(withAccount(settings, provider, id))}
          onManage={() => setSettingsTab('accounts')}
          onSettings={() => setSettingsTab('agents')}
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
    </XtermTheme.Provider>
  );
}
