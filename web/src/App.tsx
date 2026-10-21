import { PanelLeftClose, PanelLeftOpen, Search, Settings as SettingsIcon } from 'lucide-react';
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
import type { Toward } from './gen/Toward';
import { agentLabel } from './catalog';
import { Find, FIND_ARIA_KEY, FIND_INPUT, FIND_KEY } from './Find';
import { findSpots } from './findSpots';
import { flowId, Graph, type GraphHandle, type Lit, type Pulse } from './Graph';
import { useSelectedRepo } from './repos';
import { applyLanguage, INITIAL_LANGUAGE } from './i18n';
import { Settings, SETTINGS_TABS, type SettingsTab } from './Settings';
import { CardViewSwitch } from './ThemeSettings';
import { ArrangeButton, loadDirection, saveDirection } from './ArrangeButton';
import { Dock, DOCK_TOGGLE, DockRail, type DockTab } from './Dock';
import type { ArrangeDirection } from './layout';
import { Splitter, usePaneOpen, usePaneWidth, useViewportWidth, type Pane } from './Splitter';
import { StatusBar, type Usage } from './StatusBar';
import { activeTheme, RESET_TERMINAL_THEME, type ColorMode } from './terminalThemes';
import { Tree } from './Tree';
import { TitleBar } from './TitleBar';
import { UpdateCard } from './Update';
import { activeFont, findKey, paneKey, XtermFont, XtermTheme } from './Terminal';
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
/** 오른쪽 칸은 가장 좁은 너비로 시작한다(사람 지시 2026-10-20 「우측 사이드바 최소 너비를 디폴트로 설정」). */
const DOCK_MIN_WIDTH = 320;
/** `max`는 App이 창 폭으로 바꿔 넘긴다. 여기 값은 아직 창 폭을 모를 때의 자리값이다. */
const DOCK_PANE: Pane = { key: 'loom.dockWidth', initial: DOCK_MIN_WIDTH, min: DOCK_MIN_WIDTH, max: 800, grow: -1 };
/** 접힌 사이드바(양쪽)의 줄 폭. 아이콘 단추(30) 하나와 여백이 들어간다(목업 docs/mockups/toolbar.html 4번). */
const RAIL_WIDTH = 40;
/** 왼쪽 사이드바 접기·펴기 단추의 id. 펼친 사이드바와 접힌 줄에 하나씩만 있다. */
const SIDE_TOGGLE = 'side-toggle';
/** 왼쪽 접기·펴기 키(Terminal.tsx `paneKey`). 툴팁과 `aria-keyshortcuts`에 보인다. */
const SIDE_KEY = 'Ctrl+Shift+[';
const SIDE_ARIA_KEY = 'Control+Shift+[';
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
  graphScope: 'repo',
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
  /** 저장소 하나의 새 모양(워크트리를 만들거나 지운 뒤)을 목록에 넣는다. 처음 보는 저장소면 끝에 더한다. */
  const putRepo = (next: RepoView) => setRepos((prev) => (prev.some((r) => r.id === next.id) ? prev.map((r) => (r.id === next.id ? next : r)) : [...prev, next]));
  // 노드가 모르는 워크트리에 있으면(`loom spawn --worktree`, 터미널에서 만든 워크트리) 목록을 다시 읽는다. 경로마다 한 번만 읽는다.
  const askedWorktrees = useRef(new Set<string>());
  const [nodes, setNodes] = useState<ReadonlyMap<NodeId, NodeView>>(new Map());
  // 자식 id → 강조하는 동안 그 자식의 간선으로 오간 메시지 수(가는 쪽마다).
  const [pulses, setPulses] = useState<ReadonlyMap<NodeId, Pulse>>(new Map());
  const pulseTimers = useRef(new Map<NodeId, number>());
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [settings, setSettings] = useState<SettingsView>(EMPTY_SETTINGS);
  const [accounts, setAccounts] = useState<AccountView[] | null>(null);
  const [fetchedAt, setFetchedAt] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [settingsTab, setSettingsTab] = useSettingsTab();
  const [selected, setSelected] = useState<NodeId | null>(null);
  const [direction, setDirection] = useState(loadDirection);
  // 오른쪽 사이드바 아래 칸 하나를 이벤트 콘솔, 커밋 그래프, 고른 노드의 터미널(심플 보기)이 탭으로 번갈아 쓴다.
  const [tab, setTab] = useState<DockTab>('commits');
  const [sideOpen, setSideOpen] = usePaneOpen('loom.sideOpen');
  const [dockOpen, setDockOpen] = usePaneOpen('loom.dockOpen');
  const graph = useRef<GraphHandle>(null);
  const [sideWidth, setSideWidth] = usePaneWidth(SIDE_PANE);
  const sideShown = sideOpen ? sideWidth : RAIL_WIDTH;
  const viewport = useViewportWidth();
  // 오른쪽 칸은 창 폭에서 왼쪽 칸과 그래프 최소 폭을 뺀 만큼까지 늘어난다. 창을 넓히면 한도도 넓어진다.
  const dockPane: Pane = { ...DOCK_PANE, max: Math.max(DOCK_PANE.min, viewport - sideShown - DOCK_MIN_GRAPH) };
  const [dockWidth, setDockWidth] = usePaneWidth(dockPane);
  const simpleCards = settings.cardView === 'simple';
  // 그래프가 그리는 저장소(docs/design.md 「그래프와 화면」). 저장소 하나 범위면 현재 저장소만이다.
  const shown = settings.graphScope === 'all' ? repos : repo ? [repo] : [];
  // 저장소가 하나면 그래프 위에 그 이름을 보인다. 둘 이상이면 저장소 상자 머리가 이름을 보인다.
  const title = shown.length === 1 ? shown[0].name : null;
  const dockedView = simpleCards && selected !== null ? nodes.get(selected) : undefined;
  const tabs: readonly DockTab[] = dockedView ? ['events', 'commits', 'terminal'] : ['events', 'commits'];
  // 터미널 탭을 보다가 노드 고르기를 풀면 커밋 그래프로 돌아간다.
  const shownTab = tabs.includes(tab) ? tab : 'commits';
  const log = useEventLog(dockOpen && shownTab === 'events');
  // 이벤트 콘솔에서 고른 이벤트. 칸을 접거나 다른 탭으로 가면 끈다.
  const [highlight, setHighlight] = useState<EventRecord | null>(null);
  useEffect(() => {
    if (!dockOpen || shownTab !== 'events') setHighlight(null);
  }, [dockOpen, shownTab]);
  // 사이드바 찾기(목업 docs/mockups/sidebar-find.html). 빈 글자면 꺼졌다. `at`은 현재 일치 번호다. 모든 저장소에서 찾는다.
  const [find, setFind] = useState({ query: '', at: 0 });
  const found = findSpots(repos, [...nodes.values()], find.query);
  // 노드가 사라져 일치가 줄면 마지막 일치에 머문다.
  const foundAt = Math.min(find.at, Math.max(found.length - 1, 0));
  const finding = find.query.trim() !== '';
  // 찾기 일치가 있으면 이벤트 강조보다 앞선다.
  const lit: Lit | null =
    found.length > 0
      ? { keys: new Set(found.map(flowId)), current: flowId(found[foundAt]) }
      : highlight && { keys: new Set(involved(highlight.entry).map(String)), current: null };
  /** 접기·펴기 단추를 누른 뒤 포커스를 바뀐 자리의 같은 단추(접힌 줄 ↔ 사이드바)로 옮긴다. */
  const focusToggle = (id: string) => requestAnimationFrame(() => document.getElementById(id)?.focus());
  /** 사이드바를 펴고 찾기 칸의 글자를 모두 고른다. 접혀 있었으면 칸이 그려진 다음 프레임에 고른다. */
  const focusFind = () => {
    setSideOpen(true);
    requestAnimationFrame(() => (document.getElementById(FIND_INPUT) as HTMLInputElement | null)?.select());
  };
  const clearFind = () => {
    // 지울 때 현재 일치가 노드면 그 노드를 고른다. 찾는 동안 고르면 그 터미널이 포커스를 가져가 입력이 끊긴다.
    const current = found[foundAt];
    setFind({ query: '', at: 0 });
    if (current?.kind === 'node') selectNode(current.node);
  };
  const openSide = (open: boolean) => {
    setSideOpen(open);
    focusToggle(SIDE_TOGGLE);
  };
  const openDock = (open: boolean) => {
    setDockOpen(open);
    focusToggle(DOCK_TOGGLE);
  };
  // `Ctrl+Shift+[`·`]`로 사이드바를 접고 편다. 포커스는 그대로 둔다(터미널에서 눌러도 터미널에 남는다).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // `Ctrl+Shift+F`는 사이드바를 펴고 찾기 칸으로 간다. 터미널도 이 키를 창으로 올려 보낸다.
      if (findKey(e)) {
        e.preventDefault();
        return focusFind();
      }
      const side = paneKey(e);
      if (!side) return;
      e.preventDefault();
      if (e.repeat) return;
      if (side === 'left') setSideOpen(!sideOpen);
      else setDockOpen(!dockOpen);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [sideOpen, dockOpen]);
  const arrange = (next: ArrangeDirection) => {
    setDirection(next);
    saveDirection(next);
    graph.current?.arrange();
  };
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
  /** 심플 보기에서는 노드를 고를 때 오른쪽 사이드바를 펴고 그 터미널 탭을 연다. 카드에는 터미널이 없다. */
  const selectNode = (id: NodeId | null) => {
    setSelected(id);
    // 다른 저장소의 노드(트리, 이벤트 기록, 모든 저장소 그래프)를 고르면 현재 저장소를 그 저장소로 바꾼다.
    const place = id === null ? undefined : nodes.get(id)?.place;
    if (place && place.repo !== repo?.id) selectRepo(place.repo);
    if (simpleCards && id !== null) {
      setTab('terminal');
      setDockOpen(true);
    }
  };

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
    const pulse = (child: NodeId, toward: Toward) => {
      setPulses((prev) => {
        const counts = prev.get(child) ?? { parent: 0, child: 0 };
        return new Map(prev).set(child, { ...counts, [toward]: counts[toward] + 1 });
      });
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
          pulse(event.child, event.toward);
          break;
        case 'removed':
          setNodes((prev) => {
            const next = new Map(prev);
            next.delete(event.id);
            return next;
          });
          break;
        case 'snapshot':
          setNodes(new Map(event.nodes.map((view) => [view.id, view])));
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
        <TitleBar />
        <div className="app" style={{ '--side-w': `${sideShown}px`, '--dock-w': `${dockOpen ? dockWidth : RAIL_WIDTH}px` } as CSSProperties}>
          {sideOpen && <Splitter pane={SIDE_PANE} width={sideWidth} onWidth={setSideWidth} label={t('app.resize', { pane: t('app.sideLabel') })} style={{ left: sideWidth }} />}
          {dockOpen && <Splitter pane={dockPane} width={dockWidth} onWidth={setDockWidth} label={t('app.resize', { pane: t('dock.label') })} style={{ right: dockWidth }} />}
          {sideOpen ? (
            <nav className="side" id="side" aria-label={t('app.sideLabel')}>
              <div className="brand">
                loom
                <button id={SIDE_TOGGLE} className="btn ghost sm icon" aria-expanded aria-controls="side" aria-label={t('app.sideCollapse')} aria-keyshortcuts={SIDE_ARIA_KEY} title={`${t('app.sideCollapse')} (${SIDE_KEY})`} onClick={() => openSide(false)}>
                  <PanelLeftClose />
                </button>
              </div>
              <Find
                query={find.query}
                found={found}
                at={foundAt}
                repo={repo?.id ?? null}
                onRepo={selectRepo}
                onQuery={(query) => setFind({ query, at: 0 })}
                onStep={(step) => setFind((prev) => (found.length > 0 ? { ...prev, at: (foundAt + step + found.length) % found.length } : prev))}
                onClear={clearFind}
              />
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
                find={!finding ? null : { query: find.query, found, at: foundAt, onAt: (at) => setFind((prev) => ({ ...prev, at })) }}
              />
              <div className="side-foot">
                <button className="btn ghost" onClick={() => setSettingsTab('agents')}>
                  <SettingsIcon />
                  {t('settings.title')}
                </button>
              </div>
            </nav>
          ) : (
            <nav className="rail side-rail" id="side" aria-label={t('app.sideLabel')}>
              <button id={SIDE_TOGGLE} className="btn ghost icon" aria-expanded={false} aria-controls="side" aria-label={t('app.sideExpand')} aria-keyshortcuts={SIDE_ARIA_KEY} title={`${t('app.sideExpand')} (${SIDE_KEY})`} onClick={() => openSide(true)}>
                <PanelLeftOpen />
              </button>
              {/* 글자가 남아 그래프가 강조 중이면 점을 찍는다(목업 sidebar-find.html ④). */}
              <button className={finding ? 'btn ghost icon finding' : 'btn ghost icon'} aria-label={t('find.open')} aria-keyshortcuts={FIND_ARIA_KEY} title={`${t('find.open')} (${FIND_KEY})`} onClick={focusFind}>
                <Search />
              </button>
              <button className="btn ghost icon rail-end" aria-label={t('settings.title')} title={t('settings.title')} onClick={() => setSettingsTab('agents')}>
                <SettingsIcon />
              </button>
            </nav>
          )}
          <main className="canvas" aria-label={title === null ? t('app.allGraphLabel') : t('app.graphLabel', { repo: title })}>
            {title !== null && <h1 className="canvas-title">{title}</h1>}
            {shown.length > 0 && <Graph
              repos={shown}
              onRepo={putRepo}
              views={new Map([...nodes].filter(([, v]) => shown.some((r) => r.id === v.place.repo)))}
              pulses={pulses}
              agents={agents}
              settings={settings}
              accounts={accounts}
              selected={selected}
              onSelect={selectNode}
              highlight={lit}
              onClearHighlight={() => setHighlight(null)}
              cardView={settings.cardView}
              direction={direction}
              ref={graph}
            />}
          </main>
          {dockOpen ? (
            <Dock
              tabs={tabs}
              tab={shownTab}
              onTab={setTab}
              onCollapse={() => openDock(false)}
              tools={
                <>
                  <CardViewSwitch value={settings.cardView} onChange={(cardView) => saveSettings({ ...settings, cardView })} />
                  {repo && <ArrangeButton direction={direction} onArrange={arrange} />}
                </>
              }
            >
              {shownTab === 'events' && <EventConsole records={log.records} views={nodes} agents={agents} highlight={highlight} onHighlight={setHighlight} />}
              {shownTab === 'commits' && repo && <CommitGraph repo={repo.id} name={repo.name} />}
              {shownTab === 'terminal' && dockedView && (
                <TerminalDock
                  view={dockedView}
                  agentLabel={agentLabel(agents, dockedView.agent)}
                  onNavigate={(dir) => {
                    const to = graph.current?.neighbor(dockedView.id, dir);
                    if (to != null) selectNode(to);
                  }}
                />
              )}
            </Dock>
          ) : (
            <DockRail
              tabs={tabs}
              onArrange={() => arrange(direction)}
              onExpand={(next) => {
                if (next) setTab(next);
                openDock(true);
              }}
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
