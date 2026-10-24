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

type Choice = { agent: AgentView | null; installed: boolean };

const keyOf = (c: Choice): AgentId | null => c.agent?.id ?? null;

const pathAt = (agent: AgentView, distro: string | null) => agent.places.find((p) => p.distro === distro)?.path ?? null;

function choices(agents: AgentView[], distro: string | null): Choice[] {
  return [
    { agent: null, installed: true },
    ...agents.filter((a) => pathAt(a, distro) !== null).map((agent) => ({ agent, installed: true })),
    ...agents.filter((a) => pathAt(a, distro) === null).map((agent) => ({ agent, installed: false })),
  ];
}


export function NewNodePopover(props: {
  branch: string;
  place: Place;
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
  const loading = props.agents.length === 0;
  const places = props.agents[0]?.places.map((p) => p.distro) ?? [null];
  const distro = places.includes(props.distro) ? props.distro : null;
  const multi = places.length > 1;
  const all = loading ? [] : choices(props.agents, distro);
  const installed = all.filter((c) => c.installed);
  const [query, setQuery] = useState('');
  const [picked, setPicked] = useState<{ key: AgentId | null } | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const starting = useStarting(busy && props.distro !== null);
  const listRef = useRef<HTMLUListElement>(null);

  const needle = query.trim().toLowerCase();
  const shown = all.filter((c) => labelOf(c).toLowerCase().includes(needle));
  const selectable = shown.filter((c) => c.installed);
  const fallback = installed.find((c) => props.recent !== undefined && keyOf(c) === props.recent) ?? installed.find((c) => c.agent) ?? installed[0];
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
