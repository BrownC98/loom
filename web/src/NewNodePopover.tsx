import { LoaderCircle } from 'lucide-react';
import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { createNode } from './api';
import { useEscapeClose } from './popover';
import { canBypass } from './catalog';
import type { AccountView } from './gen/AccountView';
import type { AgentId } from './gen/AgentId';
import type { AgentView } from './gen/AgentView';
import type { NodeId } from './gen/NodeId';
import type { Place } from './gen/Place';
import type { SettingsView } from './gen/SettingsView';
import type { Start } from './gen/Start';
import { shortPlaceName, useStarting } from './place';
import { accountName, activeAccount, windowsText } from './usage';

/** 목록 한 줄. `agent`가 없으면 기본 셸이다. */
type Choice = { agent: AgentView | null; installed: boolean };

/** 고를 수 있는 항목의 키. 기본 셸은 `null`이다. */
const keyOf = (c: Choice): AgentId | null => c.agent?.id ?? null;

/** 그 위치(배포판, null은 이 PC)에서 찾은 경로. 없으면 그 위치에 설치하지 않은 것이다. */
const pathAt = (agent: AgentView, distro: string | null) => agent.places.find((p) => p.distro === distro)?.path ?? null;

/** 설치됨(카탈로그 순서) → 기본 셸 → 설치하지 않음. 카탈로그가 유일한 원천이다(목업 settings.html 5번).
 *  설치는 저장소 위치에서 본다(목업 wsl-place.html ⑥a). */
function choices(agents: AgentView[], distro: string | null): Choice[] {
  return [
    ...agents.filter((a) => pathAt(a, distro) !== null).map((agent) => ({ agent, installed: true })),
    { agent: null, installed: true },
    ...agents.filter((a) => pathAt(a, distro) === null).map((agent) => ({ agent, installed: false })),
  ];
}


/** 프로필만 고른다. 목록 줄을 누르거나 Enter를 누르면 바로 띄운다(사람 지시 2026-10-20 「새노드는 리스트 아이템을 클릭하면 바로 해당 노드가 생성되는걸로 변경」). 이름은 데몬이 짓고, 첫 지시문은 띄운 뒤 카드 터미널에 입력한다. `recent`는 사람이 마지막으로 띄운 노드의 에이전트다. */
export function NewNodePopover(props: {
  branch: string;
  /** 새 노드가 도는 묶음. */
  place: Place;
  /** 묶음이 WSL에 있으면 그 배포판. 띄우기가 오래 걸리면 「시작하는 중」을 보인다(목업 wsl-place.html ⑥b). */
  distro: string | null;
  agents: AgentView[];
  recent: AgentId | null | undefined;
  settings: SettingsView;
  accounts: AccountView[] | null;
  onCreated: (id: NodeId) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const box = useEscapeClose<HTMLFormElement>(props.onCancel);
  const labelOf = (c: Choice) => c.agent?.label ?? t('newNode.shell');
  // 카탈로그는 늘 비어 있지 않다. 비었으면 데몬이 아직 PATH를 찾는 중이다(약 2초).
  const loading = props.agents.length === 0;
  // 저장소 위치를 아직 찾지 못했으면(배포판이 꺼져 있음) 이 PC 목록으로 고른다. 띄우기는 데몬이 그 위치에서 다시 찾는다.
  const places = props.agents[0]?.places.map((p) => p.distro) ?? [null];
  const distro = places.includes(props.distro) ? props.distro : null;
  const multi = places.length > 1;
  const all = loading ? [] : choices(props.agents, distro);
  const installed = all.filter((c) => c.installed);
  const [query, setQuery] = useState('');
  // 처음 값: 최근 프로필이 설치돼 있으면 그것, 아니면 설치한 첫째. 목록이 늦게 와도 맞게 정해지도록 렌더마다 계산한다.
  const [picked, setPicked] = useState<{ key: AgentId | null } | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const starting = useStarting(busy && props.distro !== null);
  const listRef = useRef<HTMLUListElement>(null);

  const needle = query.trim().toLowerCase();
  const shown = all.filter((c) => labelOf(c).toLowerCase().includes(needle));
  const selectable = shown.filter((c) => c.installed);
  const fallback = installed.find((c) => props.recent !== undefined && keyOf(c) === props.recent) ?? installed[0];
  const wanted = picked ? picked.key : fallback && keyOf(fallback);
  const active = selectable.find((c) => keyOf(c) === wanted) ?? selectable[0];
  const activeIndex = active ? shown.indexOf(active) : -1;
  const agent = active?.agent ?? null;
  const account = agent?.provider && props.accounts ? activeAccount(props.accounts, props.settings, agent.provider, distro) : undefined;

  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [activeIndex]);

  async function create(choice: Choice | undefined) {
    if (!choice?.installed || busy) return;
    setBusy(true);
    const start: Start = choice.agent ? { kind: 'agent', agent: choice.agent.id, brief: '' } : { kind: 'command', line: '' };
    try {
      props.onCreated(await createNode({ start, place: props.place }));
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    void create(active);
  }

  function move(e: KeyboardEvent) {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    if (selectable.length === 0) return;
    const at = active ? selectable.indexOf(active) : -1;
    const next = selectable[(at + (e.key === 'ArrowDown' ? 1 : -1) + selectable.length) % selectable.length];
    setPicked({ key: keyOf(next) });
  }

  const optionId = (i: number) => `n-opt-${i}`;
  const installedShown = shown.filter((c) => c.installed);
  const missingShown = shown.filter((c) => !c.installed);
  const option = (c: Choice) => {
    const i = shown.indexOf(c);
    const selected = c === active;
    return (
      <li
        key={keyOf(c) ?? ''}
        id={optionId(i)}
        role="option"
        aria-selected={selected}
        aria-disabled={!c.installed || undefined}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => void create(c)}
      >
        <span translate="no">{labelOf(c)}</span>
        {c.installed && props.recent !== undefined && keyOf(c) === props.recent && <span className="recent">{t('newNode.recent')}</span>}
        {!c.installed ? <span className="tag">{multi && c.agent?.places.some((p) => p.path !== null) ? t('place.missingAt', { place: distro === null ? t('place.pcShort') : t('place.wsl') }) : t('newNode.notInstalled')}</span> : !c.agent?.profile && <span className="tag">{t('newNode.terminalOnly')}</span>}
      </li>
    );
  };

  return (
    <form ref={box} className="pop nodrag nowheel" aria-label={t('graph.newNode')} onSubmit={submit}>
      <h2>
        {t('graph.newNode')}
        <span translate="no">{props.branch}</span>
      </h2>
      <div className="field">
        <label htmlFor="n-find">{t('newNode.profile')}</label>
        <input
          id="n-find"
          role="combobox"
          aria-expanded="true"
          aria-controls="n-list"
          aria-activedescendant={activeIndex >= 0 ? optionId(activeIndex) : undefined}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={move}
          placeholder={t('newNode.search')}
          autoComplete="off"
          spellCheck={false}
          autoFocus
        />
        <ul id="n-list" ref={listRef} className="pick" role="listbox" aria-label={t('newNode.profile')}>
          {shown.length === 0 && (
            <li className="grp" role="presentation">
              {loading ? t('newNode.loading') : t('newNode.noMatch')}
            </li>
          )}
          {installedShown.length > 0 && (
            <li className="grp" role="presentation">
              {multi
                ? t('newNode.installedAt', { n: installedShown.filter((c) => c.agent).length, place: shortPlaceName(distro) })
                : t('newNode.installed', { n: installedShown.filter((c) => c.agent).length })}
            </li>
          )}
          {installedShown.map(option)}
          {missingShown.length > 0 && (
            <li className="grp" role="presentation">
              {t('newNode.missing', { n: missingShown.length })}
            </li>
          )}
          {missingShown.map(option)}
        </ul>
        {account && (
          <span className="hint">
            {multi
              ? t('newNode.accountAt', { name: accountName(account), place: shortPlaceName(distro), usage: windowsText(account.usage.limits) })
              : t('newNode.account', { name: accountName(account), usage: windowsText(account.usage.limits) })}
          </span>
        )}
        {agent && canBypass(agent, props.settings) && <span className="hint">{t('newNode.bypass')}</span>}
      </div>
      {starting && (
        <span className="hint" role="status">
          {t('place.starting', { distro: props.distro })}
        </span>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="pop-actions">
        <button type="button" className="btn ghost" onClick={props.onCancel}>
          {t('common.cancel')}
        </button>
        {starting && (
          <button type="button" className="btn primary" disabled>
            <LoaderCircle className="spin" />
            {t('place.startingButton')}
          </button>
        )}
      </div>
    </form>
  );
}
