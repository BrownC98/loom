import { PanelLeftClose, PanelLeftOpen, Search, Settings as SettingsIcon } from 'lucide-react';
import { useCallback, useEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { ReactFlowProvider } from '@xyflow/react';
import { useTranslation } from 'react-i18next';
import { getAccounts, getAgents, getCursorUsage, getMovableNodes, getNodes, getRepos, moveNodes, getDefaultSettings, getSettings, putSettings, newBuildServed, shutdown, watchGraph } from './api';
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { PlaceUsage } from './gen/PlaceUsage';
import type { AgentView } from './gen/AgentView';
import { CommitGraph } from './CommitGraph';
import { DocList } from './DocList';
import { EventConsole, involved, useEventLog } from './EventConsole';
import type { EventRecord } from './gen/EventRecord';
import type { MovableNode } from './gen/MovableNode';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import type { Provider } from './gen/Provider';
import type { RepoId } from './gen/RepoId';
import type { RepoView } from './gen/RepoView';
import type { SettingsView } from './gen/SettingsView';
import type { Theme } from './gen/Theme';
import type { Toward } from './gen/Toward';
import { agentLabel } from './catalog';
import { Find, FIND_INPUT, FIND_KEY } from './Find';
import { findSpots } from './findSpots';
import { isTauri } from '@tauri-apps/api/core';
import { exit } from '@tauri-apps/plugin-process';
import { settingsPatch, type SettingsPatch } from './settingsPatch';
import { pickHue } from './hopLanes';
import { Graph, type GraphHandle, type Hop, type Lit, type Pulse } from './Graph';
import { CameraProvider, flowId } from './camera';
import { chosenRepo } from './repos';
import { applyLanguage, INITIAL_LANGUAGE } from './i18n';
import { applyColors, NO_COLOR, useColorMode } from './uiColors';
import { Settings, SETTINGS_TABS, type SettingsTab } from './Settings';
import { CardViewSwitch } from './ThemeSettings';
import { cardViewOf } from './cardView';
import { ArrangeButton } from './ArrangeButton';
import { keepSame } from './keepSame';
import { Dock, DOCK_TOGGLE, DockRail, type DockTab } from './Dock';
import type { ArrangeDirection } from './layout';
import { Splitter, paneWidth, useViewportWidth, type Pane } from './Splitter';
import type { ClosedPanes } from './gen/ClosedPanes';
import { StatusBar, type Usage } from './StatusBar';
import { Inbox, InboxPanel, waitCount, waitingNodes } from './Inbox';
import { InboxTabs } from './InboxHistory';
import { activeTheme, RESET_TERMINAL_THEME } from './terminalThemes';
import { Bookmarks } from './Bookmarks';
import { Tree } from './Tree';
import { usePhoneTheme, syncAppChrome } from './phoneTheme';
import { Phone, usePhoneRoute } from './Phone';
import { usePhone } from './sheet';
import { TitleBar } from './TitleBar';
import { UpdateCard, useUpdates } from './Update';
import { shortcut } from './platform';
import { activeFont, XtermFont, XtermTheme } from './Terminal';
import { findKey, paneKey } from './keys';
import { TerminalDock } from './TerminalDock';
import { CanvasComposer, deskKeys } from './Composer';
import { PlaceTag } from './place';
import { SwitchDialog } from './SwitchDialog';
import { accountName, withAccount } from './usage';
import { useSwitchNotice } from './useSwitchNotice';

type Live = Readonly<{ pulses: ReadonlyMap<NodeId, Pulse>; hops: ReadonlyMap<string, Hop> }>;

const PULSE_MS = 3000;
const USAGE_REFRESH_MS = 5 * 60 * 1000;
const REPOS_REFRESH_MS = 5000;
const SIDE_PANE: Pane = { key: 'side', cssVar: '--side-w', initial: 260, min: 180, max: 480, grow: 1 };
const DOCK_MIN_WIDTH = 320;
const DOCK_PANE: Pane = { key: 'dock', cssVar: '--dock-w', initial: DOCK_MIN_WIDTH, min: DOCK_MIN_WIDTH, max: 800, grow: -1 };
const RAIL_WIDTH = 40;
const SIDE_TOGGLE = 'side-toggle';
const SIDE_KEY = shortcut('[');
const DOCK_MIN_GRAPH = 120;
const EMPTY_SETTINGS: SettingsView = {
  bypassOff: [],
  accounts: {},
  wslAccounts: {},
  theme: (document.documentElement.dataset.theme as Theme | undefined) ?? 'system',
  terminalTheme: RESET_TERMINAL_THEME,
  terminalFont: { family: null, size: null },
  cardView: 'standard',
  graphScope: 'repo',
  edgeStyle: 'step',
  questionPill: 'line',
  composerOff: false,
  language: INITIAL_LANGUAGE,
  workspaceDir: null,
  wslWorkspaceDirs: {},
  treeLimits: { maxDepth: 2, maxChildren: null },
  autoSwitch: false,
  osNotify: false,
  alwaysOn: { oMode: true, userDoc: true, steOrwell: true, raptor3: true },
  roles: [],
  arrangeDirection: 'down',
  alwaysArrange: false,
  minimapFolded: false,
  repo: null,
  paneWidths: { side: null, dock: null },
  pillPlace: null,
  composerPlace: null,
  pillSize: 'normal',
  composerSize: 'normal',
  closedPanes: { side: false, dock: false, bookmarks: false },
  closedSwitchNotice: null,
  closedPill: null,
  colors: { warn: NO_COLOR, run: NO_COLOR, ok: NO_COLOR, err: NO_COLOR, edge: NO_COLOR, repo: NO_COLOR, worktree: NO_COLOR, accent: NO_COLOR, notice: NO_COLOR },
};
const SETTINGS_HASH = '#settings/';

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

export function App() {
  const { t } = useTranslation();
  const [repos, setRepos] = useState<RepoView[]>([]);
  const [repoFocused, setRepoFocused] = useState(true);
  const selectRepo = (id: RepoId) => {
    editSettings((s) => ({ ...s, repo: id }));
    setRepoFocused(true);
  };
  const clearFocus = () => {
    setSelected(null);
    setChosenBox(null);
    setRepoFocused(false);
  };
  const putRepo = (next: RepoView) => setRepos((prev) => (prev.some((r) => r.id === next.id) ? prev.map((r) => (r.id === next.id ? next : r)) : [...prev, next]));
  const askedWorktrees = useRef(new Set<string>());
  const [nodes, setNodes] = useState<ReadonlyMap<NodeId, NodeView>>(new Map());
  const [live, setLive] = useState<Live>({ pulses: new Map(), hops: new Map() });
  const liveTimers = useRef(new Map<string, number>());
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [settings, setSettings] = useState<SettingsView>(EMPTY_SETTINGS);
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [defaultSettings, setDefaultSettings] = useState<SettingsView | null>(null);
  useEffect(() => void getDefaultSettings().then(setDefaultSettings, () => {}), []);
  const [viewVersion, setViewVersion] = useState(0);
  const updates = useUpdates(viewVersion);
  const repo = chosenRepo(repos, settings.repo);
  const [accounts, setAccounts] = useState<AccountView[] | null>(null);
  const [cursor, setCursor] = useState<PlaceUsage[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [settingsTab, setSettingsTab] = useSettingsTab();
  const phone = usePhone();
  const [phoneRoute, setPhoneRoute] = usePhoneRoute();
  const [online, setOnline] = useState(true);
  const [stopped, setStopped] = useState(false);
  const [selected, setSelected] = useState<NodeId | null>(null);
  const [chosenBox, setChosenBox] = useState<string | null>(null);
  const direction = settings.arrangeDirection;
  const [tab, setTab] = useState<DockTab>('questions');
  const sideOpen = !settings.closedPanes.side;
  const dockOpen = !settings.closedPanes.dock;
  const setPaneOpen = (pane: keyof ClosedPanes, open: boolean) => editSettings((s) => ({ ...s, closedPanes: { ...s.closedPanes, [pane]: !open } }));
  const setSideOpen = (open: boolean) => setPaneOpen('side', open);
  const setDockOpen = (open: boolean) => setPaneOpen('dock', open);
  const graph = useRef<GraphHandle>(null);
  const saveWidth = (pane: Pane) => (width: number) => editSettings((s) => ({ ...s, paneWidths: { ...s.paneWidths, [pane.key]: width } }));
  const sideWidth = paneWidth(SIDE_PANE, settings.paneWidths.side);
  const sideShown = sideOpen ? sideWidth : RAIL_WIDTH;
  const viewport = useViewportWidth();
  const dockPane: Pane = { ...DOCK_PANE, max: Math.max(DOCK_PANE.min, viewport - sideShown - DOCK_MIN_GRAPH) };
  const dockWidth = paneWidth(dockPane, settings.paneWidths.dock);
  const shown = settings.graphScope === 'all' ? repos : repo ? [repo] : [];
  const title = shown.length === 1 ? shown[0].name : null;
  const simpleNode = (id: NodeId | null) => {
    const view = id === null ? undefined : nodes.get(id);
    return view && cardViewOf(view, settings.cardView) === 'simple' ? view : undefined;
  };
  const dockedView = simpleNode(selected);
  const tabs: readonly DockTab[] = dockedView ? ['questions', 'docs', 'events', 'commits', 'terminal'] : ['questions', 'docs', 'events', 'commits'];
  const waiting = waitingNodes(nodes);
  const shownTab = tabs.includes(tab) ? tab : 'questions';
  const log = useEventLog(dockOpen && shownTab === 'events');
  const [highlight, setHighlight] = useState<EventRecord | null>(null);
  useEffect(() => {
    if (!dockOpen || shownTab !== 'events') setHighlight(null);
  }, [dockOpen, shownTab]);
  const [find, setFind] = useState({ query: '', at: 0 });
  const found = findSpots(repos, [...nodes.values()], find.query);
  const foundAt = Math.min(find.at, Math.max(found.length - 1, 0));
  const finding = find.query.trim() !== '';
  const lit: Lit | null =
    found.length > 0
      ? { keys: new Set(found.map(flowId)), current: flowId(found[foundAt]) }
      : highlight && { keys: new Set(involved(highlight.entry).map(String)), current: null };
  const focusToggle = (id: string) => requestAnimationFrame(() => document.getElementById(id)?.focus());
  const focusFind = () => {
    setSideOpen(true);
    requestAnimationFrame(() => (document.getElementById(FIND_INPUT) as HTMLInputElement | null)?.select());
  };
  const clearFind = () => {
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
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
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
    editSettings((s) => ({ ...s, arrangeDirection: next }));
    graph.current?.arrange();
  };
  const alwaysArrange = (on: boolean) => {
    editSettings((s) => ({ ...s, alwaysArrange: on }));
    if (on) graph.current?.arrange();
  };
  const [phoneTheme, choosePhoneTheme] = usePhoneTheme();
  const theme = phoneTheme ?? settings.theme;
  const mode = useColorMode(theme);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  useEffect(() => syncAppChrome(mode), [mode]);
  useEffect(() => applyLanguage(settings.language), [settings.language]);
  useEffect(() => applyColors(settings.colors), [settings.colors]);

  const refreshUsage = useCallback(async () => {
    setLoading(true);
    try {
      const [read, cursorRead] = await Promise.all([getAccounts(), getCursorUsage().catch(() => [])]);
      setAccounts(read);
      setCursor(cursorRead);
    } finally {
      setLoading(false);
    }
  }, []);
  const afterStop = () => (isTauri() ? exit(0) : setStopped(true));
  const rescan = () => getAgents().then(setAgents);
  const settingsReads = useRef(0);
  const readSettings = () => {
    const asked = ++settingsReads.current;
    return getSettings().then((next) => asked === settingsReads.current && setSettings(next));
  };
  const sendSettings = (patch: SettingsPatch) =>
    putSettings(patch)
      .catch(() => {})
      .finally(readSettings);
  const saveSettings = (next: SettingsView) => {
    const patch = settingsPatch(settings, next);
    return patch ? sendSettings(patch) : Promise.resolve();
  };
  const editSettings = (edit: (current: SettingsView) => SettingsView) => saveSettings(edit(settings));
  const resetSettings = (patch: SettingsPatch) => void sendSettings(patch);
  const notice = useSwitchNotice(settings.closedSwitchNotice, (id) => editSettings((s) => ({ ...s, closedSwitchNotice: id })), () => {
    void readSettings();
    refreshUsage();
  });
  const [switching, setSwitching] = useState<{ provider: Provider; distro: string | null; to: AccountId | null; nodes: MovableNode[] } | null>(null);
  const chooseAccount = async (provider: Provider, distro: string | null, to: AccountId | null) => {
    const nodes = await getMovableNodes(provider, distro, to).catch(() => []);
    if (nodes.length > 0) setSwitching({ provider, distro, to, nodes });
    else saveSettings(withAccount(settings, provider, distro, to));
  };
  const switchingName = (s: NonNullable<typeof switching>) => {
    const own = accounts?.filter((a) => a.provider === s.provider && a.distro === s.distro) ?? [];
    const a = own.find((x) => x.id === s.to);
    return a ? accountName(a) : t('usage.systemDefault');
  };
  const usage: Usage = { accounts, cursor, loading, refresh: refreshUsage };
  const chooseBox = (key: string | null) => {
    setChosenBox(key);
    if (key !== null) setSelected(null);
  };
  const [focusAsk, setFocusAsk] = useState(0);
  const composerView = selected === null ? undefined : nodes.get(selected);
  const focusNode = (id: NodeId) => {
    selectNode(id);
    setFocusAsk((n) => n + 1);
  };
  const selectNode = (id: NodeId | null) => {
    setSelected(id);
    if (id !== null) setChosenBox(null);
    const place = id === null ? undefined : nodes.get(id)?.place;
    if (place) selectRepo(place.repo);
    if (simpleNode(id)) {
      setTab('terminal');
      setDockOpen(true);
    }
  };

  useEffect(() => {
    rescan();
    readSettings().finally(() => setSettingsLoaded(true));
    refreshUsage();
    const timer = window.setInterval(refreshUsage, USAGE_REFRESH_MS);
    return () => clearInterval(timer);
  }, [refreshUsage]);

  useEffect(() => {
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
    const refreshRepos = () =>
      getRepos().then((next) => setRepos((prev) => (JSON.stringify(prev) === JSON.stringify(next) ? prev : next)));
    refreshRepos();
    const timer = window.setInterval(() => {
      if (!document.hidden) refreshRepos();
    }, REPOS_REFRESH_MS);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    const flash = (timer: string, bump: (now: Live) => Live, drop: (now: Live) => Live) => {
      setLive(bump);
      clearTimeout(liveTimers.current.get(timer));
      liveTimers.current.set(timer, window.setTimeout(() => setLive(drop), PULSE_MS));
    };
    const without = <K, V>(map: ReadonlyMap<K, V>, key: K) => {
      const next = new Map(map);
      next.delete(key);
      return next;
    };
    const hueOf = (now: Live, prev: { hue: number } | undefined) => prev?.hue ?? pickHue([...now.pulses.values(), ...now.hops.values()].map((edge) => edge.hue));
    const pulse = (child: NodeId, toward: Toward) =>
      flash(
        `pulse-${child}`,
        (now) => {
          const prev = now.pulses.get(child);
          const counts = prev?.counts ?? { parent: 0, child: 0 };
          return { ...now, pulses: new Map(now.pulses).set(child, { hue: hueOf(now, prev), last: toward, counts: { ...counts, [toward]: counts[toward] + 1 } }) };
        },
        (now) => ({ ...now, pulses: without(now.pulses, child) }),
      );
    const hop = (from: NodeId, to: NodeId) => {
      const [source, target] = from < to ? [from, to] : [to, from];
      const toward = to === target ? 'target' : 'source';
      const key = `${source}-${target}`;
      flash(
        `hop-${key}`,
        (now) => {
          const prev = now.hops.get(key);
          const counts = prev?.counts ?? { source: 0, target: 0 };
          return { ...now, hops: new Map(now.hops).set(key, { source, target, hue: hueOf(now, prev), last: toward, counts: { ...counts, [toward]: counts[toward] + 1 } }) };
        },
        (now) => ({ ...now, hops: without(now.hops, key) }),
      );
    };
    let reading = false;
    let again = false;
    const readNodes = () => {
      if (reading) return void (again = true);
      reading = true;
      getNodes()
        .then(
          (views) =>
            again ||
            setNodes((old) => {
              const before = [...old.values()];
              const kept = keepSame(before, views);
              return kept === before ? old : new Map(kept.map((view) => [view.id, view]));
            }),
          () => again || graph.reopen(),
        )
        .finally(() => {
          reading = false;
          if (!again) return;
          again = false;
          readNodes();
        });
    };
    const linked = (up: boolean) => {
      setOnline(up);
      if (!up) return;
      newBuildServed()
        .then((stale) => stale && location.reload())
        .catch(() => {});
      void readSettings();
      setViewVersion((n) => n + 1);
    };
    const graph = watchGraph((event) => {
      switch (event.kind) {
        case 'message':
          pulse(event.child, event.toward);
          break;
        case 'hop':
          hop(event.from, event.to);
          break;
        case 'nodes':
          readNodes();
          break;
        case 'viewChanged':
          void readSettings();
          setViewVersion((n) => n + 1);
          break;
        case 'recorded':
          log.onRecord(event.record);
          notice.onRecord(event.record);
          break;
        default:
          event satisfies never;
      }
    }, linked);
    return graph.close;
  }, []);

  if (stopped)
    return (
      <main className="stopped">
        <b>{t('app.stopped')}</b>
        <span>{t('app.stoppedHint')}</span>
      </main>
    );

  if (!settingsLoaded) return null;
  const findView = (
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
  );
  const treeView = (select: (id: NodeId) => void) => (
    <Tree
      repos={repos}
      current={repoFocused ? (repo?.id ?? null) : null}
      onRepo={selectRepo}
      onClear={clearFocus}
      onAdded={(added) => {
        putRepo(added);
        selectRepo(added.id);
      }}
      views={nodes}
      agents={agents}
      onRescanAgents={rescan}
      selected={selected}
      onSelect={select}
      chosenBox={chosenBox}
      onChooseBox={chooseBox}
      settings={settings}
      accounts={accounts}
      onRepoChanged={putRepo}
      onRepoRemoved={(id) => setRepos((prev) => prev.filter((r) => r.id !== id))}
      find={!finding ? null : { query: find.query, found, at: foundAt, onAt: (at) => setFind((prev) => ({ ...prev, at })) }}
    />
  );
  const openNode = (id: NodeId) => setPhoneRoute({ screen: 'node', id });
  const providers = (children: ReactNode) => (
    <XtermTheme.Provider value={activeTheme(settings.terminalTheme, mode)}>
      <XtermFont.Provider value={activeFont(settings.terminalFont)}>
        <ReactFlowProvider>
          <CameraProvider>{children}</CameraProvider>
        </ReactFlowProvider>
      </XtermFont.Provider>
    </XtermTheme.Provider>
  );
  if (phone)
    return providers(
      <Phone
        route={phoneRoute}
        theme={theme}
        onTheme={choosePhoneTheme}
        onRoute={setPhoneRoute}
        waiting={waiting}
        nodes={nodes}
        agentLabel={(v) => agentLabel(agents, v.agent)}
        online={online}
        inbox={<InboxPanel waiting={waiting} onSelect={openNode} onFocus={openNode} />}
        tree={
          <>
            {findView}
            <Bookmarks views={nodes} repos={repos} selected={selected} onSelect={openNode} open={!settings.closedPanes.bookmarks} onOpen={(open) => setPaneOpen('bookmarks', open)} />
            {treeView(openNode)}
          </>
        }
        foot={<StatusBar usage={usage} settings={settings} distro={repo?.wsl?.distro ?? null} onChoose={chooseAccount} notice={{ record: notice.record, onClose: notice.close }} />}
      />,
    );
  return providers(
      <>
        <TitleBar />
        <div className="app" style={{ [SIDE_PANE.cssVar]: `${sideShown}px`, [DOCK_PANE.cssVar]: `${dockOpen ? dockWidth : RAIL_WIDTH}px` } as CSSProperties}>
          {sideOpen && <Splitter pane={SIDE_PANE} width={sideWidth} onCommit={saveWidth(SIDE_PANE)} label={t('app.resize', { pane: t('app.sideLabel') })} />}
          {dockOpen && <Splitter pane={dockPane} width={dockWidth} onCommit={saveWidth(dockPane)} label={t('app.resize', { pane: t('dock.label') })} />}
          {sideOpen ? (
            <nav className="side" id="side" aria-label={t('app.sideLabel')}>
              <div className="side-top">
                {findView}
                <button id={SIDE_TOGGLE} className="btn ghost sm icon" aria-expanded aria-controls="side" aria-label={t('app.sideCollapse')} aria-keyshortcuts={SIDE_KEY.aria} title={`${t('app.sideCollapse')} (${SIDE_KEY.label})`} onClick={() => openSide(false)}>
                  <PanelLeftClose />
                </button>
              </div>
              <Bookmarks views={nodes} repos={repos} selected={selected} onSelect={selectNode} open={!settings.closedPanes.bookmarks} onOpen={(open) => setPaneOpen('bookmarks', open)} />
              {treeView(selectNode)}
              <div className="side-foot">
                <button className="btn ghost" onClick={() => setSettingsTab(SETTINGS_TABS[0])}>
                  <SettingsIcon />
                  {t('settings.title')}
                </button>
              </div>
            </nav>
          ) : (
            <nav className="rail side-rail" id="side" aria-label={t('app.sideLabel')}>
              <button id={SIDE_TOGGLE} className="btn ghost icon" aria-expanded={false} aria-controls="side" aria-label={t('app.sideExpand')} aria-keyshortcuts={SIDE_KEY.aria} title={`${t('app.sideExpand')} (${SIDE_KEY.label})`} onClick={() => openSide(true)}>
                <PanelLeftOpen />
              </button>
              <button className={finding ? 'btn ghost icon finding' : 'btn ghost icon'} aria-label={t('find.open')} aria-keyshortcuts={FIND_KEY.aria} title={`${t('find.open')} (${FIND_KEY.label})`} onClick={focusFind}>
                <Search />
              </button>
              <button className="btn ghost icon rail-end" aria-label={t('settings.title')} title={t('settings.title')} onClick={() => setSettingsTab(SETTINGS_TABS[0])}>
                <SettingsIcon />
              </button>
            </nav>
          )}
          <main className="canvas" aria-label={title === null ? t('app.allGraphLabel') : t('app.graphLabel', { repo: title })}>
            {title !== null && (
              <h1 className="canvas-title">
                {title}
                <PlaceTag wsl={shown[0].wsl} />
              </h1>
            )}
            <Inbox waiting={waiting} notify={settings.osNotify} pill={settings.questionPill} place={settings.pillPlace} size={settings.pillSize} onPlace={(pillPlace) => editSettings((s) => ({ ...s, pillPlace }))} closed={settings.closedPill} onClose={(closedPill) => editSettings((s) => ({ ...s, closedPill }))} onSelect={selectNode} onFocus={focusNode} />
            {shown.length > 0 && <Graph
              repos={shown}
              viewVersion={viewVersion}
              onMinimapFold={(minimapFolded) => editSettings((s) => ({ ...s, minimapFolded }))}
              onRepo={putRepo}
              views={new Map([...nodes].filter(([, v]) => shown.some((r) => r.id === v.place.repo)))}
              pulses={live.pulses}
              hops={live.hops}
              agents={agents}
              onRescanAgents={rescan}
              settings={settings}
              accounts={accounts}
              selected={selected}
              focusAsk={focusAsk}
              onSelect={selectNode}
              chosenBox={chosenBox}
              onChooseBox={chooseBox}
              onClear={clearFocus}
              highlight={lit}
              onClearHighlight={() => setHighlight(null)}
              direction={direction}
              ref={graph}
            />}
            {!settings.composerOff && composerView && <CanvasComposer key={composerView.id} node={composerView.id} name={composerView.name} keys={deskKeys(() => focusNode(composerView.id))} place={settings.composerPlace} size={settings.composerSize} onPlace={(composerPlace) => editSettings((s) => ({ ...s, composerPlace }))} views={nodes} repos={repos} onPick={selectNode} />}
          </main>
          {dockOpen ? (
            <Dock
              tabs={tabs}
              tab={shownTab}
              waiting={waitCount(waiting)}
              onTab={setTab}
              onCollapse={() => openDock(false)}
              tools={
                <>
                  <CardViewSwitch value={settings.cardView} onChange={(cardView) => saveSettings({ ...settings, cardView })} />
                  {repo && <ArrangeButton direction={direction} onArrange={arrange} alwaysArrange={settings.alwaysArrange} onAlwaysArrange={alwaysArrange} hasSizes={() => graph.current?.hasSizes() ?? false} onResetSizes={() => graph.current?.resetSizes()} />}
                </>
              }
            >
              {shownTab === 'events' && <EventConsole records={log.records} views={nodes} agents={agents} graphRepos={shown.map((r) => r.id)} onRepo={selectRepo} highlight={highlight} onHighlight={setHighlight} />}
              {shownTab === 'commits' && repo && repoFocused && <CommitGraph repo={repo.id} name={repo.name} />}
              {shownTab === 'docs' && <DocList />}
              {shownTab === 'questions' && <InboxTabs waiting={waiting} onSelect={selectNode} onFocus={focusNode} />}
              {shownTab === 'terminal' && dockedView && (
                <TerminalDock
                  view={dockedView}
                  defaultView={settings.cardView}
                  agentLabel={agentLabel(agents, dockedView.agent)}
                  focusAsk={focusAsk}
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
              waiting={waitCount(waiting)}
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
            distro={repo?.wsl?.distro ?? null}
            onChoose={chooseAccount}
            notice={{ record: notice.record, onClose: notice.close }}
            onManage={() => setSettingsTab('accounts')}
          />
          <UpdateCard updates={updates} />
          {settingsTab && (
            <Settings
              tab={settingsTab}
              onTab={setSettingsTab}
              onClose={() => setSettingsTab(null)}
              agents={agents}
              onRescan={rescan}
              settings={settings}
              onSettings={saveSettings}
              defaults={defaultSettings}
              onReset={resetSettings}
              usage={usage}
              onChoose={chooseAccount}
              updates={updates}
              viewVersion={viewVersion}
              onStop={() => void shutdown().then(afterStop)}
            />
          )}
          {switching && (
            <SwitchDialog
              provider={switching.provider}
              to={switchingName(switching)}
              nodes={switching.nodes}
              onKeep={() => {
                saveSettings(withAccount(settings, switching.provider, switching.distro, switching.to));
                setSwitching(null);
              }}
              onMoveAll={async () => {
                await saveSettings(withAccount(settings, switching.provider, switching.distro, switching.to));
                await moveNodes(switching.provider, switching.distro);
                setSwitching(null);
              }}
              onCancel={() => setSwitching(null)}
            />
          )}
        </div>
      </>,
  );
}
