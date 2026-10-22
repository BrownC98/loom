import { PanelLeftClose, PanelLeftOpen, Search, Settings as SettingsIcon } from 'lucide-react';
import { useCallback, useEffect, useRef, useState, type CSSProperties, type Dispatch, type ReactNode, type SetStateAction } from 'react';
import { ReactFlowProvider } from '@xyflow/react';
import { useTranslation } from 'react-i18next';
import { getAccounts, getAgents, getMovableNodes, getRepos, moveNodes, getDefaultSettings, getSettings, putSettings, newBuildServed, shutdown, watchGraph } from './api';
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import { CommitGraph } from './CommitGraph';
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
import { agentLabel, canResume } from './catalog';
import { Find, FIND_INPUT, FIND_KEY } from './Find';
import { findSpots } from './findSpots';
import { isTauri } from '@tauri-apps/api/core';
import { exit } from '@tauri-apps/plugin-process';
import { applySettingsPatch, settingsPatch, type SettingsPatch } from './settingsPatch';
import { flowId, Graph, type GraphHandle, type Hop, type Lit, type Pulse } from './Graph';
import { chosenRepo } from './repos';
import { applyLanguage, INITIAL_LANGUAGE } from './i18n';
import { applyColors, NO_COLOR, useColorMode } from './uiColors';
import { Settings, SETTINGS_TABS, type SettingsTab } from './Settings';
import { CardViewSwitch } from './ThemeSettings';
import { ArrangeButton } from './ArrangeButton';
import { Dock, DOCK_TOGGLE, DockRail, type DockTab } from './Dock';
import type { ArrangeDirection } from './layout';
import { Splitter, paneWidth, useViewportWidth, type Pane } from './Splitter';
import type { ClosedPanes } from './gen/ClosedPanes';
import { StatusBar, type Usage } from './StatusBar';
import { Inbox, InboxPanel, waitCount, waitingNodes } from './Inbox';
import { activeTheme, RESET_TERMINAL_THEME } from './terminalThemes';
import { Bookmarks } from './Bookmarks';
import { Tree } from './Tree';
import { usePhoneTheme, syncAppChrome } from './phoneTheme';
import { Phone, usePhone, usePhoneRoute } from './Phone';
import { TitleBar } from './TitleBar';
import { UpdateCard, useUpdates } from './Update';
import { shortcut } from './platform';
import { activeFont, findKey, paneKey, XtermFont, XtermTheme } from './Terminal';
import { TerminalDock } from './TerminalDock';
import { PlaceTag } from './place';
import { SwitchDialog } from './SwitchDialog';
import { accountName, withAccount } from './usage';
import { useSwitchNotice } from './useSwitchNotice';

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
const SIDE_PANE: Pane = { key: 'side', cssVar: '--side-w', initial: 260, min: 180, max: 480, grow: 1 };
/** 오른쪽 칸은 가장 좁은 너비로 시작한다(사람 지시 2026-10-20 「우측 사이드바 최소 너비를 디폴트로 설정」). */
const DOCK_MIN_WIDTH = 320;
/** `max`는 App이 창 폭으로 바꿔 넘긴다. 여기 값은 아직 창 폭을 모를 때의 자리값이다. */
const DOCK_PANE: Pane = { key: 'dock', cssVar: '--dock-w', initial: DOCK_MIN_WIDTH, min: DOCK_MIN_WIDTH, max: 800, grow: -1 };
/** 접힌 사이드바(양쪽)의 줄 폭. 아이콘 단추(30) 하나와 여백이 들어간다(목업 docs/mockups/toolbar.html 4번). */
const RAIL_WIDTH = 40;
/** 왼쪽 사이드바 접기·펴기 단추의 id. 펼친 사이드바와 접힌 줄에 하나씩만 있다. */
const SIDE_TOGGLE = 'side-toggle';
/** 왼쪽 접기·펴기 키(Terminal.tsx `paneKey`). 툴팁과 `aria-keyshortcuts`에 보인다. */
const SIDE_KEY = shortcut('[');
/** 오른쪽 칸과 그래프 사이에 늘 남기는 폭. 그래프를 다 덮어 되돌릴 수 없게 되는 것을 막는다. */
const DOCK_MIN_GRAPH = 120;
const EMPTY_SETTINGS: SettingsView = {
  bypassOff: [],
  claudeAccount: null,
  codexAccount: null,
  wslAccounts: {},
  // 데몬이 첫 페이지에 넣은 테마(`Theme`의 serde 이름)다. 설정을 받기 전에도 같은 테마를 그린다.
  theme: (document.documentElement.dataset.theme as Theme | undefined) ?? 'system',
  terminalTheme: RESET_TERMINAL_THEME,
  terminalFont: { family: null, size: null },
  cardView: 'standard',
  graphScope: 'repo',
  edgeStyle: 'step',
  questionPill: 'line',
  language: INITIAL_LANGUAGE,
  workspaceDir: null,
  wslWorkspaceDirs: {},
  // 설정을 받기 전 자리값이다. 기본값의 원천은 Rust `TreeLimits::default`다.
  treeLimits: { maxDepth: 2, maxChildren: null },
  autoSwitch: false,
  osNotify: false,
  alwaysOn: { oMode: true, userDoc: true, ste100: true, raptor3: true },
  arrangeDirection: 'down',
  alwaysArrange: false,
  minimapFolded: false,
  repo: null,
  paneWidths: { side: null, dock: null },
  closedPanes: { side: false, dock: false, bookmarks: false },
  closedSwitchNotice: null,
  colors: { warn: NO_COLOR, run: NO_COLOR, ok: NO_COLOR, err: NO_COLOR, edge: NO_COLOR, repo: NO_COLOR, worktree: NO_COLOR, accent: NO_COLOR, notice: NO_COLOR },
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

export function App() {
  const { t } = useTranslation();
  const [repos, setRepos] = useState<RepoView[]>([]);
  // 사이드바 저장소 줄의 포커스와 커밋 그래프. 바탕이나 사이드바 「저장소」 제목을 누르면 지운다. 그래프가 그리는 저장소(`repo`)는 그대로다. 2026-10-21 사람 지시.
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
  /** 저장소 하나의 새 모양(워크트리를 만들거나 지운 뒤)을 목록에 넣는다. 처음 보는 저장소면 끝에 더한다. */
  const putRepo = (next: RepoView) => setRepos((prev) => (prev.some((r) => r.id === next.id) ? prev.map((r) => (r.id === next.id ? next : r)) : [...prev, next]));
  // 노드가 모르는 워크트리에 있으면(`loom spawn --worktree`, 터미널에서 만든 워크트리) 목록을 다시 읽는다. 경로마다 한 번만 읽는다.
  const askedWorktrees = useRef(new Set<string>());
  const [nodes, setNodes] = useState<ReadonlyMap<NodeId, NodeView>>(new Map());
  // 자식 id → 강조하는 동안 그 자식의 간선으로 오간 메시지 수(가는 쪽마다).
  const [pulses, setPulses] = useState<ReadonlyMap<NodeId, Pulse>>(new Map());
  const pulseTimers = useRef(new Map<NodeId, number>());
  // 부모·자식이 아닌 두 노드 쌍(작은 id-큰 id) → 강조하는 동안 오간 메시지 수.
  const [hops, setHops] = useState<ReadonlyMap<string, Hop>>(new Map());
  const hopTimers = useRef(new Map<string, number>());
  const [agents, setAgents] = useState<AgentView[]>([]);
  // 화면 상태는 데몬 설정에 있다(프론트는 뷰어). 다른 화면이 바꾸면 `ViewChanged` 이벤트가 알려 다시 읽는다.
  const [settings, setSettingsState] = useState<SettingsView>(EMPTY_SETTINGS);
  // 가장 새 설정. 이벤트 핸들러가 낡은 렌더의 설정 위에 덮어쓰지 않게 한다.
  const settingsRef = useRef(settings);
  const setSettings = (next: SettingsView) => {
    settingsRef.current = next;
    setSettingsState(next);
  };
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  // 데몬의 기본 설정. 설정 창의 칸마다 [기본값] 단추가 견준다. 판이 바뀌기 전에는 바뀌지 않아 한 번만 읽는다.
  const [defaultSettings, setDefaultSettings] = useState<SettingsView | null>(null);
  useEffect(() => void getDefaultSettings().then(setDefaultSettings, () => {}), []);
  // 보내는 중인 칸. 다시 읽은 설정 위에 얹어, 아직 저장되지 않은 내 변경이 되돌아가 보이지 않게 한다.
  const pending = useRef<SettingsPatch[]>([]);
  const showFetched = (view: SettingsView) => setSettings(pending.current.reduce(applySettingsPatch, view));
  // 설정 밖의 화면 상태(카드 자리·크기)를 다시 읽으라는 신호. 그래프가 본다.
  const [viewVersion, setViewVersion] = useState(0);
  const updates = useUpdates(viewVersion);
  const repo = chosenRepo(repos, settings.repo);
  const [accounts, setAccounts] = useState<AccountView[] | null>(null);
  const [fetchedAt, setFetchedAt] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [settingsTab, setSettingsTab] = useSettingsTab();
  // 폰 폭 배치(목업 docs/mockups/mobile.html). 데몬 이벤트 연결이 끊기면 폰이 끊김 줄을 보인다.
  const phone = usePhone();
  const [phoneRoute, setPhoneRoute] = usePhoneRoute();
  const [online, setOnline] = useState(true);
  // 사람이 [loom 끄기]로 데몬을 끈 뒤다(목업 web-stop.html ③).
  const [stopped, setStopped] = useState(false);
  const [selected, setSelected] = useState<NodeId | null>(null);
  // 고른 묶음·저장소 상자(`flowId`). 카드와 함께 고르지 않는다(2026-10-21 사람 지시 "묶음도 클릭하면 포커싱 표시되게 해줘", "사이드바에서 클릭시에도 동작해야함").
  const [chosenBox, setChosenBox] = useState<string | null>(null);
  const direction = settings.arrangeDirection;
  // 오른쪽 사이드바 아래 칸 하나를 이벤트 콘솔, 커밋 그래프, 고른 노드의 터미널(심플 보기)이 탭으로 번갈아 쓴다.
  const [tab, setTab] = useState<DockTab>('commits');
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
  // 오른쪽 칸은 창 폭에서 왼쪽 칸과 그래프 최소 폭을 뺀 만큼까지 늘어난다. 창을 넓히면 한도도 넓어진다.
  const dockPane: Pane = { ...DOCK_PANE, max: Math.max(DOCK_PANE.min, viewport - sideShown - DOCK_MIN_GRAPH) };
  const dockWidth = paneWidth(dockPane, settings.paneWidths.dock);
  const simpleCards = settings.cardView === 'simple';
  // 그래프가 그리는 저장소(docs/design.md 「그래프와 화면」). 저장소 하나 범위면 현재 저장소만이다.
  const shown = settings.graphScope === 'all' ? repos : repo ? [repo] : [];
  // 저장소가 하나면 그래프 위에 그 이름을 보인다. 둘 이상이면 저장소 상자 머리가 이름을 보인다.
  const title = shown.length === 1 ? shown[0].name : null;
  const dockedView = simpleCards && selected !== null ? nodes.get(selected) : undefined;
  const tabs: readonly DockTab[] = dockedView ? ['events', 'commits', 'questions', 'terminal'] : ['events', 'commits', 'questions'];
  const waiting = waitingNodes(nodes);
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
  // 명령 키+Shift+`[`·`]`로 사이드바를 접고 편다. 포커스는 그대로 둔다(터미널에서 눌러도 터미널에 남는다).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // 명령 키+Shift+`F`는 사이드바를 펴고 찾기 칸으로 간다. 터미널도 이 키를 창으로 올려 보낸다.
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
  // 켜는 순간 지금 방향으로 한 번 정렬한다(목업 always-arrange.html 「정한 것」).
  const alwaysArrange = (on: boolean) => {
    editSettings((s) => ({ ...s, alwaysArrange: on }));
    if (on) graph.current?.arrange();
  };
  // 이 폰에서 고른 테마가 PC 설정보다 앞선다. 화면이 그리는 테마는 여기서 한 번만 정한다.
  const [phoneTheme, choosePhoneTheme] = usePhoneTheme();
  const theme = phoneTheme ?? settings.theme;
  const mode = useColorMode(theme);

  useEffect(() => {
    // styles.css가 `data-theme`으로 `color-scheme`을 고른다.
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  useEffect(() => syncAppChrome(mode), [mode]);
  useEffect(() => applyLanguage(settings.language), [settings.language]);
  useEffect(() => applyColors(settings.colors), [settings.colors]);

  const refreshUsage = useCallback(async () => {
    setLoading(true);
    try {
      setAccounts(await getAccounts());
      setFetchedAt(Date.now());
    } finally {
      setLoading(false);
    }
  }, []);
  // 데몬을 끈 뒤: 앱은 함께 끝난다. 앱이 남으면 아이콘이 남은 창만 앞으로 가져와 데몬을 다시 띄우지 않는다. 브라우저는 꺼짐 안내를 보인다.
  const afterStop = () => (isTauri() ? exit(0) : setStopped(true));
  const rescan = () => getAgents().then(setAgents);
  // 화면이 먼저 바뀌고 저장은 뒤따른다. 저장이 실패하면 데몬 값으로 되돌린다.
  // 바꾼 칸만 보낸다. 다른 화면이 같은 때 다른 칸을 바꿔도 데몬이 둘 다 합친다(`src/settings.rs` `Settings::patch`).
  // 저장이 끝나면 다시 읽는다. 실패했으면 데몬 값으로 돌아간다.
  const saveSettings = (next: SettingsView) => {
    const patch = settingsPatch(settingsRef.current, next);
    if (!patch) return Promise.resolve();
    setSettings(next);
    pending.current.push(patch);
    return putSettings(patch)
      .catch(() => {})
      .finally(() => {
        pending.current = pending.current.filter((p) => p !== patch);
        return getSettings().then(showFetched);
      });
  };
  const editSettings = (edit: (current: SettingsView) => SettingsView) => saveSettings(edit(settingsRef.current));
  // `null`은 칸을 지우고, 데몬은 지운 칸을 기본값으로 읽는다(`Settings::patch`). 기본값은 Rust에만 둔다.
  const resetSettings = (patch: SettingsPatch) => void putSettings(patch)
    .catch(() => {})
    .finally(() => getSettings().then(showFetched));
  // 데몬이 스스로 계정을 바꿨다. 쓰는 계정과 사용량을 다시 읽는다.
  const notice = useSwitchNotice(settings.closedSwitchNotice, (id) => editSettings((s) => ({ ...s, closedSwitchNotice: id })), () => {
    getSettings().then(showFetched);
    refreshUsage();
  });
  // 계정 고르기: 그 계정이 아닌 노드가 없으면 바로 저장한다. 있으면 확인 창이 묻는다(취소하면 아무것도 바꾸지 않는다).
  // 「사용 중」은 위치(이 PC, WSL 배포판)마다 하나다. 그 위치 저장소의 노드만 옮긴다(목업 wsl-place.html ③).
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
  const usage: Usage = { accounts, fetchedAt, loading, refresh: refreshUsage };
  /** 심플 보기에서는 노드를 고를 때 오른쪽 사이드바를 펴고 그 터미널 탭을 연다. 카드에는 터미널이 없다. */
  const chooseBox = (key: string | null) => {
    setChosenBox(key);
    if (key !== null) setSelected(null);
  };
  /** 노드를 고르고 그 터미널에 입력 초점을 준다. 이미 고른 노드여도 다시 잡는다(질문 목록의 질문 줄, 2026-10-22 사람 지시 "질문영역 … 클릭하면 해당 터미널 포커싱"). */
  const [focusAsk, setFocusAsk] = useState(0);
  const focusNode = (id: NodeId) => {
    selectNode(id);
    setFocusAsk((n) => n + 1);
  };
  const selectNode = (id: NodeId | null) => {
    setSelected(id);
    if (id !== null) setChosenBox(null);
    // 다른 저장소의 노드(트리, 이벤트 기록, 모든 저장소 그래프)를 고르면 현재 저장소를 그 저장소로 바꾼다.
    const place = id === null ? undefined : nodes.get(id)?.place;
    if (place) selectRepo(place.repo);
    if (simpleCards && id !== null) {
      setTab('terminal');
      setDockOpen(true);
    }
  };

  useEffect(() => {
    rescan();
    getSettings()
      .then(setSettings)
      .finally(() => setSettingsLoaded(true));
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
    /** `key`의 값을 `bump`로 바꾸고 `PULSE_MS` 뒤에 지운다. 그 사이에 다시 오면 시간을 다시 잰다. */
    const flash = <K, V>(set: Dispatch<SetStateAction<ReadonlyMap<K, V>>>, timers: Map<K, number>, key: K, bump: (prev: V | undefined) => V) => {
      set((prev) => new Map(prev).set(key, bump(prev.get(key))));
      clearTimeout(timers.get(key));
      const timer = window.setTimeout(() => {
        set((prev) => {
          const next = new Map(prev);
          next.delete(key);
          return next;
        });
      }, PULSE_MS);
      timers.set(key, timer);
    };
    const pulse = (child: NodeId, toward: Toward) =>
      flash(setPulses, pulseTimers.current, child, (prev) => {
        const counts = prev ?? { parent: 0, child: 0 };
        return { ...counts, [toward]: counts[toward] + 1 };
      });
    const hop = (from: NodeId, to: NodeId) => {
      const [source, target] = from < to ? [from, to] : [to, from];
      const toward = to === target ? 'target' : 'source';
      flash(setHops, hopTimers.current, `${source}-${target}`, (prev) => {
        const counts = prev?.counts ?? { source: 0, target: 0 };
        return { source, target, counts: { ...counts, [toward]: counts[toward] + 1 } };
      });
    };
    return watchGraph((event) => {
      switch (event.kind) {
        case 'message':
          pulse(event.child, event.toward);
          break;
        case 'hop':
          hop(event.from, event.to);
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
          // 연결할 때마다 온다. 데몬을 새 판으로 바꾼 뒤 다시 붙으면 한 번 다시 읽는다. 옛 화면 코드가 새 데몬에 옛 규칙으로 자리를 쓰지 않게 한다.
          // 근거: 2026-10-21 총괄 결정. 사람 시험에서 밀린 저장소 상자가 밀기 전 자리 없이 저장됐고, 지금 코드는 시험 데몬에서 그렇게 저장하지 않았다(되돌리기3).
          newBuildServed()
            .then((stale) => stale && location.reload())
            .catch(() => {});
          break;
        case 'node':
          setNodes((prev) => new Map(prev).set(event.id, event));
          break;
        case 'viewChanged':
          getSettings().then(showFetched);
          setViewVersion((n) => n + 1);
          break;
        case 'recorded':
          // 두 `onRecord`는 ref와 setState만 쓴다(알림의 `onAuto`도 setState와 안정된 함수만 부른다). 처음 만든 것을 계속 불러도 된다.
          log.onRecord(event.record);
          notice.onRecord(event.record);
      }
    }, setOnline);
  }, []);

  if (stopped)
    return (
      <main className="stopped">
        <b>{t('app.stopped')}</b>
        <span>{t('app.stoppedHint')}</span>
      </main>
    );

  // 설정을 받기 전에는 그리지 않는다. 기본 너비와 펼침으로 먼저 그리면 저장한 모양으로 튄다.
  if (!settingsLoaded) return null;
  // 찾기 칸과 트리는 사이드바와 폰 노드 탭이 함께 쓴다. 폰에서는 노드를 고르면 그 노드의 전체 화면 터미널로 간다.
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
      selected={selected}
      onSelect={select}
      chosenBox={chosenBox}
      onChooseBox={chooseBox}
      settings={settings}
      accounts={accounts}
      onRepoChanged={putRepo}
      // 그래프가 그리던 저장소면 `useSelectedRepo`가 남은 첫 저장소로 바꾼다.
      onRepoRemoved={(id) => setRepos((prev) => prev.filter((r) => r.id !== id))}
      find={!finding ? null : { query: find.query, found, at: foundAt, onAt: (at) => setFind((prev) => ({ ...prev, at })) }}
    />
  );
  const openNode = (id: NodeId) => setPhoneRoute({ screen: 'node', id });
  const providers = (children: ReactNode) => (
    <XtermTheme.Provider value={activeTheme(settings.terminalTheme, mode)}>
      <XtermFont.Provider value={activeFont(settings.terminalFont)}>
        <ReactFlowProvider>{children}</ReactFlowProvider>
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
            {/* 사이드바처럼 북마크를 저장소 위에 둔다(2026-10-23 사람 "앱 노드리스트에 저장소 섹션 추가", Q453 A). */}
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
              {/* 사이드바 맨 위 줄: 찾기 칸과 접기 단추(목업 docs/mockups/sidebar-head.html). */}
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
              {/* 글자가 남아 그래프가 강조 중이면 점을 찍는다(목업 sidebar-find.html ④). */}
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
            <Inbox waiting={waiting} notify={settings.osNotify} pill={settings.questionPill} onSelect={selectNode} onFocus={focusNode} />
            {shown.length > 0 && <Graph
              repos={shown}
              viewVersion={viewVersion}
              onMinimapFold={(minimapFolded) => editSettings((s) => ({ ...s, minimapFolded }))}
              onRepo={putRepo}
              views={new Map([...nodes].filter(([, v]) => shown.some((r) => r.id === v.place.repo)))}
              pulses={pulses}
              hops={hops}
              agents={agents}
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
              cardView={settings.cardView}
              direction={direction}
              ref={graph}
            />}
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
              {shownTab === 'events' && <EventConsole records={log.records} views={nodes} agents={agents} highlight={highlight} onHighlight={setHighlight} />}
              {shownTab === 'commits' && repo && repoFocused && <CommitGraph repo={repo.id} name={repo.name} />}
              {shownTab === 'questions' && <InboxPanel waiting={waiting} onSelect={selectNode} onFocus={focusNode} />}
              {shownTab === 'terminal' && dockedView && (
                <TerminalDock
                  view={dockedView}
                  agentLabel={agentLabel(agents, dockedView.agent)}
                  resumable={canResume(agents, dockedView.agent)}
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
