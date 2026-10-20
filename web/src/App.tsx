import { AlignLeft, GitBranch, GitCommitVertical, Settings as SettingsIcon } from 'lucide-react';
import { useCallback, useEffect, useRef, useState, useSyncExternalStore, type CSSProperties } from 'react';
import { ReactFlowProvider } from '@xyflow/react';
import { useTranslation } from 'react-i18next';
import { getAccounts, getAgents, getRepos, getSettings, putSettings, watchGraph } from './api';
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
import { agentLabel } from './catalog';
import { Graph, type GraphHandle } from './Graph';
import { NewWorktreePopover } from './NewWorktreePopover';
import { useSelectedRepo } from './repos';
import { applyLanguage, INITIAL_LANGUAGE } from './i18n';
import { Settings, SETTINGS_TABS, type SettingsTab } from './Settings';
import { CardViewSwitch } from './ThemeSettings';
import { ArrangeButton, loadDirection, saveDirection } from './ArrangeButton';
import { Splitter, usePaneWidth, useViewportWidth, type Pane } from './Splitter';
import { StatusBar, type Usage } from './StatusBar';
import { activeTheme, RESET_TERMINAL_THEME, type ColorMode } from './terminalThemes';
import { Tree } from './Tree';
import { UpdateCard } from './Update';
import { activeFont, XtermFont, XtermTheme } from './Terminal';
import { TerminalDock } from './TerminalDock';
import { withAccount } from './usage';

/** 메시지가 오간 간선을 강조하는 시간. 마지막 메시지부터 잰다. */
const PULSE_MS = 3000;
/** 사용량을 다시 읽는 간격. 근거: docs/design.md 「계정과 사용량」. */
const USAGE_REFRESH_MS = 5 * 60 * 1000;
/**
 * 저장소·워크트리 목록을 다시 읽는 간격. git CLI로 지운 워크트리는 데몬이 알리지 않으므로 몇 초 안에 화면에서 빼려고 읽는다.
 * 한 번에 저장소마다 git 명령 세 개라 싸다. 창이 숨으면 읽지 않는다.
 */
const REPOS_REFRESH_MS = 5000;
/**
 * 기본 너비는 목업 graph-panels.html의 값이다. 왼쪽 칸은 480이 상한이라 1440 화면에서도 그래프 칸이 160px 남는다.
 * 오른쪽 칸(터미널·이벤트·커밋)은 상한을 창 폭에 맞춘다: `dockPane`이 창 폭에서 왼쪽 칸을 뺀 만큼을 넘긴다(사람 지시 2026-10-20 「심플 모드의 사이드바 터미널 너비는 원하는 만큼」).
 */
const SIDE_PANE: Pane = { key: 'loom.sideWidth', initial: 260, min: 180, max: 480, grow: 1 };
/** `max`는 App이 창 폭으로 바꿔 넘긴다. 여기 값은 아직 창 폭을 모를 때의 자리값이다. */
const DOCK_PANE: Pane = { key: 'loom.dockWidth', initial: 520, min: 320, max: 800, grow: -1 };
/** 오른쪽 칸과 그래프 사이에 늘 남기는 폭. 그래프를 다 덮어 되돌릴 수 없게 되는 것을 막는다. */
const DOCK_MIN_GRAPH = 120;
const EMPTY_SETTINGS: SettingsView = {
  bypassOff: [],
  claudeAccount: null,
  codexAccount: null,
  // 데몬이 첫 페이지에 넣은 테마(`Theme`의 serde 이름)다. 설정을 받기 전에도 같은 테마를 그린다.
  theme: (document.documentElement.dataset.theme as Theme | undefined) ?? 'system',
  terminalTheme: RESET_TERMINAL_THEME,
  terminalFont: { family: null, size: null },
  cardView: 'standard',
  language: INITIAL_LANGUAGE,
  workspaceDir: null,
  // 설정을 받기 전 자리값이다. 기본값의 원천은 Rust `TreeLimits::default`다.
  treeLimits: { maxDepth: 2, maxChildren: null },
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
  const [repos, setRepos] = useState<RepoView[]>([]);
  const [repo, selectRepo] = useSelectedRepo(repos);
  const [creatingWorktree, setCreatingWorktree] = useState(false);
  /** 저장소 하나의 새 모양(워크트리를 만들거나 지운 뒤)을 목록에 넣는다. 처음 보는 저장소면 끝에 더한다. */
  const putRepo = (next: RepoView) => setRepos((prev) => (prev.some((r) => r.id === next.id) ? prev.map((r) => (r.id === next.id ? next : r)) : [...prev, next]));
  // 노드가 모르는 워크트리에 있으면(`loom spawn --worktree`, 터미널에서 만든 워크트리) 목록을 다시 읽는다. 경로마다 한 번만 읽는다.
  const askedWorktrees = useRef(new Set<string>());
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
  // 오른쪽 칸 하나를 이벤트 콘솔, 커밋 그래프, 고른 노드의 터미널(카드를 머리만 볼 때)이 번갈아 쓴다.
  const [direction, setDirection] = useState(loadDirection);
  const [dock, setDock] = useState<'events' | 'commits' | 'terminal' | null>(null);
  const graph = useRef<GraphHandle>(null);
  const [sideWidth, setSideWidth] = usePaneWidth(SIDE_PANE);
  const viewport = useViewportWidth();
  // 오른쪽 칸은 창 폭에서 왼쪽 칸과 그래프 최소 폭을 뺀 만큼까지 늘어난다. 창을 넓히면 한도도 넓어진다.
  const dockPane: Pane = { ...DOCK_PANE, max: Math.max(DOCK_PANE.min, viewport - sideWidth - DOCK_MIN_GRAPH) };
  const [dockWidth, setDockWidth] = usePaneWidth(dockPane);
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
  const simpleCards = settings.cardView === 'simple';
  /** 심플 보기에서는 노드를 고를 때 오른쪽 칸에 그 터미널을 연다. 카드에는 터미널이 없다. */
  const selectNode = (id: NodeId | null) => {
    setSelected(id);
    // 다른 저장소의 노드(트리, 이벤트 기록)를 고르면 그래프를 그 저장소로 바꾼다.
    const place = id === null ? undefined : nodes.get(id)?.place;
    if (place && place.repo !== repo?.id) selectRepo(place.repo);
    if (simpleCards && id !== null) setDock('terminal');
  };
  const dockedView = dock === 'terminal' && simpleCards && selected !== null ? nodes.get(selected) : undefined;
  const shownDock = dock === 'terminal' && !dockedView ? null : dock;

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
    for (const v of nodes.values()) {
      const known = repos.find((r) => r.id === v.place.repo)?.worktrees.some((w) => w.path === v.place.worktree);
      if (known === false && !askedWorktrees.current.has(v.place.worktree)) {
        askedWorktrees.current.add(v.place.worktree);
        getRepos().then(setRepos);
      }
    }
  }, [nodes, repos]);

  useEffect(() => {
    // 같은 목록이면 그대로 둔다. 새 배열은 그래프 배치를 다시 그린다.
    const refreshRepos = () =>
      getRepos().then((next) => setRepos((prev) => (JSON.stringify(prev) === JSON.stringify(next) ? prev : next)));
    refreshRepos();
    const timer = window.setInterval(() => {
      if (!document.hidden) refreshRepos();
    }, REPOS_REFRESH_MS);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
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
      <XtermFont.Provider value={activeFont(settings.terminalFont)}>
      <ReactFlowProvider>
        <div className="app" data-dock={shownDock ?? undefined} style={{ '--side-w': `${sideWidth}px`, '--dock-w': `${dockWidth}px` } as CSSProperties}>
          <Splitter pane={SIDE_PANE} width={sideWidth} onWidth={setSideWidth} label={t('app.resize', { pane: t('app.sideLabel') })} style={{ left: sideWidth }} />
          {shownDock && (
            <Splitter
              pane={dockPane}
              width={dockWidth}
              onWidth={setDockWidth}
              label={t('app.resize', { pane: shownDock === 'events' ? t('events.title') : shownDock === 'commits' ? t('app.commitGraph') : dockedView!.name })}
              style={{ right: dockWidth }}
            />
          )}
          <nav className="side" aria-label={t('app.sideLabel')}>
            <div className="brand">loom</div>
            <Tree
              repos={repos}
              current={repo?.id ?? null}
              onRepo={selectRepo}
              onAdded={(added) => {
                putRepo(added);
                selectRepo(added.id);
              }}
              views={nodes}
              agents={agents}
              selected={selected}
              onSelect={selectNode}
              settings={settings}
              accounts={accounts}
              onRepoChanged={putRepo}
              // 그래프가 그리던 저장소면 `useSelectedRepo`가 남은 첫 저장소로 바꾼다.
              onRepoRemoved={(id) => setRepos((prev) => prev.filter((r) => r.id !== id))}
            />
            <div className="side-foot">
              <button className="btn ghost" onClick={() => setSettingsTab('agents')}>
                <SettingsIcon />
                {t('settings.title')}
              </button>
            </div>
          </nav>
          <main className="canvas" aria-label={t('app.graphLabel', { repo: repo?.name ?? '' })}>
            <div className="toolbar">
              <h1>{repo?.name}</h1>
              <span className="spacer" />
              <CardViewSwitch value={settings.cardView} onChange={(cardView) => saveSettings({ ...settings, cardView })} />
              {repo && (
                <ArrangeButton
                  direction={direction}
                  onArrange={(next) => {
                    setDirection(next);
                    saveDirection(next);
                    graph.current?.arrange();
                  }}
                />
              )}
              {repo && !repo.hasCommits && (
                <span className="hint" id="no-commit">
                  {t('repo.noCommit')}
                </span>
              )}
              {repo && (
                <button className="btn" aria-expanded={creatingWorktree} disabled={!repo.hasCommits} aria-describedby={repo.hasCommits ? undefined : 'no-commit'} onClick={() => setCreatingWorktree(true)}>
                  <GitBranch />
                  {t('repo.newWorktree')}
                </button>
              )}
              <button className="btn" aria-pressed={dock === 'events'} aria-controls="dock" onClick={() => toggleDock('events')}>
                <AlignLeft />
                {t('events.title')}
              </button>
              <button className="btn" aria-pressed={dock === 'commits'} aria-controls="dock" onClick={() => toggleDock('commits')}>
                <GitCommitVertical />
                {t('app.commitGraph')}
              </button>
            </div>
            {repo && creatingWorktree && (
              <NewWorktreePopover
                repo={repo}
                workspaceDir={settings.workspaceDir ?? ''}
                onClose={() => setCreatingWorktree(false)}
                onCreated={(next) => {
                  setCreatingWorktree(false);
                  putRepo(next);
                }}
              />
            )}
            {repo && <Graph
              repo={repo}
              onRepo={putRepo}
              views={new Map([...nodes].filter(([, v]) => v.place.repo === repo.id))}
              pulses={pulses}
              agents={agents}
              settings={settings}
              accounts={accounts}
              selected={selected}
              onSelect={selectNode}
              highlight={highlight && new Set(involved(highlight.entry))}
              onClearHighlight={() => setHighlight(null)}
              cardView={settings.cardView}
              direction={direction}
              ref={graph}
            />}
          </main>
          {dock === 'events' && <EventConsole records={log.records} views={nodes} agents={agents} highlight={highlight} onHighlight={setHighlight} onClose={() => setDock(null)} />}
          {dock === 'commits' && repo && <CommitGraph repo={repo.id} onClose={() => setDock(null)} />}
          {dockedView && (
            <TerminalDock
              view={dockedView}
              agentLabel={agentLabel(agents, dockedView.agent)}
              onNavigate={(dir) => {
                const to = graph.current?.neighbor(dockedView.id, dir);
                if (to != null) selectNode(to);
              }}
              onClose={() => setDock(null)}
            />
          )}
          <StatusBar
            usage={usage}
            settings={settings}
            onChoose={(provider, id) => saveSettings(withAccount(settings, provider, id))}
            onManage={() => setSettingsTab('accounts')}
          />
          <UpdateCard />
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
      </XtermFont.Provider>
    </XtermTheme.Provider>
  );
}
