import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { createNode } from './api';
import { canBypass } from './catalog';
import type { AccountView } from './gen/AccountView';
import type { AgentId } from './gen/AgentId';
import type { AgentView } from './gen/AgentView';
import type { NodeId } from './gen/NodeId';
import type { SettingsView } from './gen/SettingsView';
import type { Start } from './gen/Start';
import { accountName, activeAccount, windowsText } from './usage';

/** 목록 한 줄. `agent`가 없으면 기본 셸이다. */
type Choice = { agent: AgentView | null; installed: boolean };

/** 고를 수 있는 항목의 키. 기본 셸은 `null`이다. */
const keyOf = (c: Choice): AgentId | null => c.agent?.id ?? null;

/** 설치됨(카탈로그 순서) → 기본 셸 → 설치하지 않음. 카탈로그가 유일한 원천이다(목업 settings.html 5번). */
function choices(agents: AgentView[]): Choice[] {
  return [
    ...agents.filter((a) => a.path !== null).map((agent) => ({ agent, installed: true })),
    { agent: null, installed: true },
    ...agents.filter((a) => a.path === null).map((agent) => ({ agent, installed: false })),
  ];
}


/** 프로필만 고른다. 이름은 데몬이 짓고, 첫 지시문은 띄운 뒤 카드 터미널에 입력한다. `recent`는 사람이 마지막으로 띄운 노드의 에이전트다. */
export function NewNodePopover(props: {
  branch: string;
  agents: AgentView[];
  recent: AgentId | null | undefined;
  settings: SettingsView;
  accounts: AccountView[] | null;
  onCreated: (id: NodeId) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const labelOf = (c: Choice) => c.agent?.label ?? t('newNode.shell');
  // 카탈로그는 늘 비어 있지 않다. 비었으면 데몬이 아직 PATH를 찾는 중이다(약 2초).
  const loading = props.agents.length === 0;
  const all = loading ? [] : choices(props.agents);
  const installed = all.filter((c) => c.installed);
  const [query, setQuery] = useState('');
  // 처음 값: 최근 프로필이 설치돼 있으면 그것, 아니면 설치한 첫째. 목록이 늦게 와도 맞게 정해지도록 렌더마다 계산한다.
  const [picked, setPicked] = useState<{ key: AgentId | null } | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const listRef = useRef<HTMLUListElement>(null);

  const needle = query.trim().toLowerCase();
  const shown = all.filter((c) => labelOf(c).toLowerCase().includes(needle));
  const selectable = shown.filter((c) => c.installed);
  const fallback = installed.find((c) => props.recent !== undefined && keyOf(c) === props.recent) ?? installed[0];
  const wanted = picked ? picked.key : fallback && keyOf(fallback);
  const active = selectable.find((c) => keyOf(c) === wanted) ?? selectable[0];
  const activeIndex = active ? shown.indexOf(active) : -1;
  const agent = active?.agent ?? null;
  const account = agent?.provider && props.accounts ? activeAccount(props.accounts, props.settings, agent.provider) : undefined;

  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [activeIndex]);

  async function create() {
    if (!active || busy) return;
    setBusy(true);
    const start: Start = agent ? { kind: 'agent', agent: agent.id, brief: '' } : { kind: 'command', line: '' };
    try {
      props.onCreated(await createNode({ start }));
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    void create();
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
        onClick={() => c.installed && setPicked({ key: keyOf(c) })}
        onDoubleClick={() => c.installed && void create()}
      >
        <span translate="no">{labelOf(c)}</span>
        {c.installed && props.recent !== undefined && keyOf(c) === props.recent && <span className="recent">{t('newNode.recent')}</span>}
        {!c.installed ? <span className="tag">{t('newNode.notInstalled')}</span> : !c.agent?.profile && <span className="tag">{t('newNode.terminalOnly')}</span>}
      </li>
    );
  };

  return (
    <form className="pop nodrag nowheel" aria-label={t('graph.newNode')} onSubmit={submit} onKeyDown={(e) => e.key === 'Escape' && props.onCancel()}>
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
              {t('newNode.installed', { n: installedShown.filter((c) => c.agent).length })}
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
          <span className="hint">{t('newNode.account', { name: accountName(account), usage: windowsText(account.usage.limits) })}</span>
        )}
        {agent && canBypass(agent, props.settings) && <span className="hint">{t('newNode.bypass')}</span>}
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="pop-actions">
        <button type="button" className="btn ghost" onClick={props.onCancel}>
          {t('common.cancel')}
        </button>
        <button type="submit" className="btn primary" disabled={busy || !active}>
          {t('newNode.create')}
        </button>
      </div>
    </form>
  );
}
