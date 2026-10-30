import { Monitor, Plus, Power, RotateCcw, RotateCw, Search, SquareTerminal, X } from 'lucide-react';
import { type CSSProperties, Fragment, type RefObject, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { FieldDefaults, FieldReset } from './FieldReset';
import type { SettingsPatch } from './settingsPatch';
import { closeLogin, getLogin, loginPty, logoutSystem, removeAccount, startLogin } from './api';
import { AgentMark } from './AgentMark';
import { canBypass } from './catalog';
import type { AlwaysOn } from './gen/AlwaysOn';
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { AgentAt } from './gen/AgentAt';
import type { AgentView } from './gen/AgentView';
import type { LoginId } from './gen/LoginId';
import type { LoginTarget } from './gen/LoginTarget';
import type { Provider } from './gen/Provider';
import type { SettingsView } from './gen/SettingsView';
import type { Usage } from './StatusBar';
import { Modal } from './Modal';
import { SizingTerminal } from './Terminal';
import { DisplayTab, GraphTab, OsNotifySection, TerminalTab } from './ThemeSettings';
import type { Updates } from './Update';
import { VERSION } from './version';
import { RoleSettings } from './RoleSettings';
import { PhoneTab } from './PhoneSettings';
import { WorkspaceFields } from './WorkspaceSettings';
import { placeName, shortPlaceName } from './place';
import { accountName, activeAccount, fetchedText, limitsProblem, notInstalled, placesOf, PROVIDER_LABEL, PROVIDERS, SYSTEM_DIR, WindowLines } from './usage';

export const SETTINGS_TABS = ['general', 'phone', 'agents', 'accounts', 'display', 'graph', 'terminal', 'about'] as const;
export type SettingsTab = (typeof SETTINGS_TABS)[number];
const RESET_TAB = {
  workspaceDir: 'general',
  wslWorkspaceDirs: 'general',
  osNotify: 'general',
  bypassOff: 'agents',
  alwaysOn: 'agents',
  roles: 'agents',
  treeLimits: 'agents',
  autoSwitch: 'accounts',
  accounts: 'notReset',
  wslAccounts: 'notReset',
  theme: 'display',
  colors: 'display',
  language: 'display',
  cardView: 'graph',
  graphScope: 'graph',
  edgeStyle: 'graph',
  questionPill: 'graph',
  composerOff: 'graph',
  pillPlace: 'graph',
  terminalTheme: 'terminal',
  terminalFont: 'terminal',
  arrangeDirection: 'notReset',
  alwaysArrange: 'notReset',
  minimapFolded: 'notReset',
  repo: 'notReset',
  paneWidths: 'notReset',
  closedPanes: 'notReset',
  closedSwitchNotice: 'notReset',
  closedPill: 'notReset',
} as const satisfies Record<keyof SettingsView, SettingsTab | 'notReset'>;
const SETTING_KEYS = Object.keys(RESET_TAB) as (keyof SettingsView)[];
const ALL_SETTINGS = SETTING_KEYS.filter((key) => RESET_TAB[key] !== 'notReset');
const tabSettings = (tab: SettingsTab) => SETTING_KEYS.filter((key) => RESET_TAB[key] === tab);
const nulls = (keys: readonly (keyof SettingsView)[]): SettingsPatch => Object.fromEntries(keys.map((key) => [key, null]));

export function Settings(props: {
  tab: SettingsTab;
  onTab: (tab: SettingsTab) => void;
  onClose: () => void;
  agents: AgentView[];
  onRescan: () => void;
  settings: SettingsView;
  onSettings: (next: SettingsView) => void;
  defaults: SettingsView | null;
  onReset: (patch: SettingsPatch) => void;
  usage: Usage;
  onChoose: (provider: Provider, distro: string | null, id: AccountId | null) => void;
  updates: Updates;
  viewVersion: number;
  onStop: () => void;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const pane = useRef<HTMLDivElement>(null);
  const found = useSearch(pane, query.trim());
  const body = (tab: SettingsTab) => (
    <>
      {tab === 'general' && (
        <>
          <WorkspaceFields settings={props.settings} onSettings={props.onSettings} />
          <OsNotifySection settings={props.settings} onSettings={props.onSettings} />
        </>
      )}
      {tab === 'phone' && <PhoneTab viewVersion={props.viewVersion} />}
      {tab === 'agents' && <AgentsTab agents={props.agents} onRescan={props.onRescan} settings={props.settings} onSettings={props.onSettings} />}
      {tab === 'accounts' && <AccountsTab settings={props.settings} onSettings={props.onSettings} usage={props.usage} onChoose={props.onChoose} viewVersion={props.viewVersion} />}
      {tab === 'display' && <DisplayTab settings={props.settings} onSettings={props.onSettings} />}
      {tab === 'graph' && <GraphTab settings={props.settings} onSettings={props.onSettings} />}
      {tab === 'terminal' && <TerminalTab settings={props.settings} onSettings={props.onSettings} />}
      {tab === 'about' && <AboutTab updates={props.updates} />}
    </>
  );
  const searching = query.trim() !== '';
  return (
    <FieldDefaults.Provider value={{ settings: props.settings, defaults: props.defaults, reset: props.onReset }}>
    <Modal className="dialog" label={t('settings.title')} onClose={props.onClose}>
      <nav aria-label={t('settings.tabsLabel')}>
        <h2>{t('settings.title')}</h2>
        <label className="settings-search">
          <Search aria-hidden="true" />
          <input
            type="search"
            value={query}
            placeholder={t('settings.search')}
            aria-label={t('settings.search')}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape' && query !== '') {
                e.preventDefault();
                e.stopPropagation();
                setQuery('');
              }
            }}
          />
          {query !== '' && (
            <button type="button" aria-label={t('settings.searchClear')} onClick={() => setQuery('')}>
              <X aria-hidden="true" />
            </button>
          )}
        </label>
        {SETTINGS_TABS.map((tab) => (
          <button
            key={tab}
            className="tab"
            aria-current={!searching && props.tab === tab ? 'page' : undefined}
            onClick={() => {
              setQuery('');
              props.onTab(tab);
            }}
          >
            {t(`settings.tab.${tab}`)}
          </button>
        ))}
        <button type="button" className="btn ghost reset-all" onClick={() => confirm(t('settings.confirmResetAll')) && props.onReset(nulls(ALL_SETTINGS))}>
          <RotateCcw />
          {t('settings.resetAll')}
        </button>
        <button type="button" className="btn ghost stop" onClick={() => confirm(t('settings.confirmStop')) && props.onStop()}>
          <Power />
          {t('settings.stop')}
        </button>
      </nav>
      <div className="pane" ref={pane}>
        <div className="pane-head">
          <h3>{searching ? t('settings.searchResults', { query: query.trim() }) : t(`settings.tab.${props.tab}`)}</h3>
          <span className="spacer" />
          {!searching && tabSettings(props.tab).length > 0 && (
            <button type="button" className="btn ghost sm" onClick={() => confirm(t('settings.confirmResetTab', { tab: t(`settings.tab.${props.tab}`) })) && props.onReset(nulls(tabSettings(props.tab)))}>
              <RotateCcw />
              {t('settings.resetTab')}
            </button>
          )}
          <button className="btn ghost close" aria-label={t('common.close')} onClick={props.onClose}>
            ×
          </button>
        </div>
        {searching
          ? SETTINGS_TABS.map((tab) => (
              <div key={tab} className="search-tab" style={{ '--tab-name': JSON.stringify(t(`settings.tab.${tab}`)) } as CSSProperties}>
                {body(tab)}
              </div>
            ))
          : body(props.tab)}
        {searching && found === 0 && <p className="hint">{t('settings.searchNone')}</p>}
      </div>
    </Modal>
    </FieldDefaults.Provider>
  );
}

const SEARCH_HIGHLIGHT = 'settings-search';

function useSearch(pane: RefObject<HTMLDivElement | null>, query: string): number | null {
  const [found, setFound] = useState<number | null>(null);
  useLayoutEffect(() => {
    const root = pane.current;
    if (!root || query === '') {
      CSS.highlights?.delete(SEARCH_HIGHLIGHT);
      setFound(null);
      return;
    }
    const needle = query.toLocaleLowerCase();
    const filter = () => {
      const ranges: Range[] = [];
      let count = 0;
      for (const section of root.querySelectorAll<HTMLElement>('.search-tab section')) {
        const hit = (section.textContent ?? '').toLocaleLowerCase().includes(needle);
        section.hidden = !hit;
        if (!hit) continue;
        count++;
        const walk = document.createTreeWalker(section, NodeFilter.SHOW_TEXT);
        while (walk.nextNode()) {
          const text = walk.currentNode.nodeValue?.toLocaleLowerCase() ?? '';
          for (let at = text.indexOf(needle); at >= 0; at = text.indexOf(needle, at + needle.length)) {
            const range = new Range();
            range.setStart(walk.currentNode, at);
            range.setEnd(walk.currentNode, at + needle.length);
            ranges.push(range);
          }
        }
      }
      CSS.highlights?.set(SEARCH_HIGHLIGHT, new Highlight(...ranges));
      setFound(count);
    };
    filter();
    const watch = new MutationObserver(filter);
    watch.observe(root, { childList: true, subtree: true, characterData: true });
    return () => {
      watch.disconnect();
      CSS.highlights?.delete(SEARCH_HIGHLIGHT);
    };
  }, [pane, query]);
  return found;
}

function AboutTab({ updates }: { updates: Updates }) {
  const { t } = useTranslation();
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<{ kind: 'latest' } | { kind: 'new' } | { kind: 'error'; message: string } | null>(null);
  const runCheck = () => {
    setChecking(true);
    updates
      .check()
      .then(
        (latest) => setResult({ kind: latest ? 'new' : 'latest' }),
        (err: unknown) => setResult({ kind: 'error', message: err instanceof Error ? err.message : String(err) }),
      )
      .finally(() => setChecking(false));
  };
  const busy = updates.step?.kind === 'downloading' || updates.step?.kind === 'restarting';
  return (
    <section>
      <h4>{t('about.version')}</h4>
      <div className="about-versions" translate="no">
        <span>{t('about.daemon')}</span>
        <b>loom v{VERSION}</b>
        {updates.appVersion && updates.appVersion !== VERSION && (
          <>
            <span>{t('about.app')}</span>
            <b>loom v{updates.appVersion}</b>
          </>
        )}
      </div>
      {updates.checks && (
        <div className="about-actions" aria-live="polite">
          <button type="button" className="btn sm" disabled={checking} onClick={runCheck}>
            {t('about.check')}
          </button>
          {result?.kind === 'latest' && !updates.latest && <span className="hint">{t('about.upToDate')}</span>}
          {result?.kind === 'error' && <span className="error">{result.message}</span>}
          {updates.latest && (
            <>
              <span className="hint">{t('about.newVersion', { version: updates.latest })}</span>
              <button type="button" className="btn primary sm" disabled={busy} onClick={updates.install}>
                {t('update.update')}
              </button>
            </>
          )}
        </div>
      )}
    </section>
  );
}

function AgentsTab(props: { agents: AgentView[]; onRescan: () => void; settings: SettingsView; onSettings: (next: SettingsView) => void }) {
  const { t } = useTranslation();
  const { agents, settings } = props;
  const places = agents[0]?.places.map((p) => p.distro) ?? [null];
  const multi = places.length > 1;
  const installed = agents.filter((a) => a.places.some((p) => p.path !== null));
  const missing = agents.filter((a) => a.places.every((p) => p.path === null));
  const toggle = (agent: AgentView, on: boolean) => {
    const rest = settings.bypassOff.filter((id) => id !== agent.id);
    props.onSettings({ ...settings, bypassOff: on ? rest : [...rest, agent.id] });
  };
  const versionText = (a: AgentView, at: AgentAt) => at.versionWarning && t(`settings.versionWarning.${a.profile}`, at.versionWarning);
  const heading = multi
    ? `${t('settings.installedHead')} ${places.map((d, i) => t('settings.installedAt', { place: shortPlaceName(d), n: agents.filter((a) => a.places[i].path !== null).length })).join(' · ')}`
    : t('settings.installed', { n: installed.length });
  return (
    <>
      <section>
        <h4>
          {heading}
          <FieldReset path={['bypassOff']} />
          <button className="btn ghost sm" onClick={props.onRescan}>
            <RotateCw />
            {t('settings.rescan')}
          </button>
        </h4>
        {installed.length === 0 ? (
          <p className="hint">{t('settings.noneInstalled')}</p>
        ) : (
          <div className="rows">
            {installed.map((a) => (
              <div className="row" key={a.id}>
                <span className="row-main">
                  <AgentMark agent={a.id} label={a.label} />
                  <span className="name">{a.label}</span>
                  {multi ? (
                    a.places.map((at) => (
                      <Fragment key={at.distro ?? ''}>
                        <span className="meta" translate="no">
                          <span className="where">{shortPlaceName(at.distro)}</span>
                          {at.path === null ? <span className="none">{t('newNode.notInstalled')}</span> : `${at.path} · ${t('settings.messaging')}`}
                        </span>
                        {versionText(a, at) && (
                          <span className="meta warnline" translate="no">
                            <span className="where">{shortPlaceName(at.distro)}</span>
                            {versionText(a, at)}
                          </span>
                        )}
                      </Fragment>
                    ))
                  ) : (
                    <>
                      {versionText(a, a.places[0]) && <span className="badge warn">{versionText(a, a.places[0])}</span>}
                      <span className="meta" translate="no">
                        {a.places[0].path} · {t('settings.messaging')}
                      </span>
                    </>
                  )}
                </span>
                <a className="hint" href={a.homepage} target="_blank" rel="noreferrer">
                  {t('settings.docs')}
                </a>
                {a.canBypass ? (
                  <label className="switch">
                    <input type="checkbox" checked={canBypass(a, settings.bypassOff)} aria-label={t('settings.bypassLabel', { agent: a.label })} onChange={(e) => toggle(a, e.target.checked)} />
                    {t('settings.bypass')}
                  </label>
                ) : (
                  <span className="off">{t('settings.bypassUnsupported')}</span>
                )}
              </div>
            ))}
          </div>
        )}
        <span className="hint">{t('settings.bypassHint')}</span>
      </section>
      <section>
        <details className="more">
          <summary>{t('settings.notInstalled', { n: missing.length })}</summary>
          <div className="cols-2">
            {missing.map((a) => (
              <a key={a.id} href={a.homepage} target="_blank" rel="noreferrer">
                <AgentMark agent={a.id} label={a.label} />
                {a.label}
              </a>
            ))}
          </div>
        </details>
      </section>
      <SkillsSection settings={settings} onSettings={props.onSettings} />
      <RoleSettings agents={agents} roles={settings.roles} oMode={settings.alwaysOn.oMode} onRoles={(roles) => props.onSettings({ ...settings, roles })} />
      <TreeLimitsSection settings={settings} onSettings={props.onSettings} />
    </>
  );
}

const SKILLS = ['oMode', 'userDoc', 'steOrwell', 'raptor3'] as const satisfies readonly (keyof AlwaysOn)[];

function SkillsSection({ settings, onSettings }: { settings: SettingsView; onSettings: (next: SettingsView) => void }) {
  const { t } = useTranslation();
  return (
    <section>
      <h4>
        {t('settings.skills')}
        <FieldReset path={['alwaysOn']} />
      </h4>
      <div className="rows">
        {SKILLS.map((key) => {
          const on = settings.alwaysOn[key];
          const name = t(`settings.skill.${key}.name`);
          return (
            <div className="row" key={key}>
              <span className="row-main">
                <span className="name">{name}</span>
                <span className="desc">{t(`settings.skill.${key}.desc`)}</span>
                {key === 'oMode' && <span className="desc">{t('settings.oModeOff')}</span>}
              </span>
              <label className="switch">
                <input type="checkbox" checked={on} aria-label={t('settings.skillLabel', { skill: name })} onChange={(e) => onSettings({ ...settings, alwaysOn: { ...settings.alwaysOn, [key]: e.target.checked } })} />
                {t(on ? 'settings.skillOn' : 'settings.skillOff')}
              </label>
            </div>
          );
        })}
      </div>
      <span className="hint">{t('settings.skillsHint')}</span>
    </section>
  );
}

const DEPTH_RANGE = { min: 0, max: 9 };
const CHILDREN_RANGE = { min: 1, max: 99 };

function inRange(text: string, range: { min: number; max: number }): number | null {
  const n = Number(text);
  return Number.isInteger(n) && n >= range.min && n <= range.max && text.trim() !== '' ? n : null;
}

function TreeLimitsSection({ settings, onSettings }: { settings: SettingsView; onSettings: (next: SettingsView) => void }) {
  const { t } = useTranslation();
  const limits = settings.treeLimits;
  const setDepth = (text: string) => {
    const maxDepth = text.trim() === '' ? null : inRange(text, DEPTH_RANGE);
    if (maxDepth !== null || text.trim() === '') onSettings({ ...settings, treeLimits: { ...limits, maxDepth } });
  };
  const setChildren = (text: string) => {
    const maxChildren = text.trim() === '' ? null : inRange(text, CHILDREN_RANGE);
    if (maxChildren !== null || text.trim() === '') onSettings({ ...settings, treeLimits: { ...limits, maxChildren } });
  };
  return (
    <section>
      <h4>
        {t('settings.treeLimits')}
        <FieldReset path={['treeLimits']} />
      </h4>
      <div className="limit-fields">
        <div className="field">
          <label htmlFor="tree-depth">{t('settings.maxDepth')}</label>
          <input
            id="tree-depth"
            type="number"
            {...DEPTH_RANGE}
            placeholder={t('settings.noLimit')}
            defaultValue={limits.maxDepth ?? ''}
            onChange={(e) => setDepth(e.target.value)}
          />
        </div>
        <div className="field">
          <label htmlFor="tree-children">{t('settings.maxChildren')}</label>
          <input
            id="tree-children"
            type="number"
            {...CHILDREN_RANGE}
            placeholder={t('settings.noLimit')}
            defaultValue={limits.maxChildren ?? ''}
            onChange={(e) => setChildren(e.target.value)}
          />
        </div>
      </div>
      <span className="hint">{t('settings.treeLimitsHint')}</span>
    </section>
  );
}

type OpenLogin = { target: LoginTarget; id: LoginId; title: string };

function AccountsTab(props: { settings: SettingsView; onSettings: (next: SettingsView) => void; usage: Usage; onChoose: (provider: Provider, distro: string | null, id: AccountId | null) => void; viewVersion: number }) {
  const { t } = useTranslation();
  const { settings, usage } = props;
  const [login, setLogin] = useState<OpenLogin | null>(null);
  const [error, setError] = useState('');

  const run = async (action: () => Promise<unknown>) => {
    setError('');
    try {
      await action();
    } catch (err) {
      setError((err as Error).message);
    }
  };
  const actions: AccountActions = {
    openLogin: (provider, distro, target, title) => run(async () => setLogin({ target, title, id: await startLogin(provider, distro, target) })),
    remove: (a, id) => {
      if (!confirm(t('accounts.confirmRemove', { name: accountName(a) }))) return;
      run(async () => {
        await removeAccount(a.provider, a.distro, id);
        usage.refresh();
      });
    },
    logout: (a) => {
      if (!confirm(t('accounts.confirmLogout', { provider: PROVIDER_LABEL[a.provider], name: accountName(a) }))) return;
      run(async () => {
        await logoutSystem(a.provider, a.distro);
        usage.refresh();
      });
    },
    choose: props.onChoose,
  };
  const places = usage.accounts ? placesOf(usage.accounts) : [];
  const providers = (distro: string | null) =>
    PROVIDERS.map((provider) => <ProviderAccounts key={provider} provider={provider} distro={distro} settings={settings} usage={usage} actions={actions} />);

  return (
    <>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <section className="auto-switch">
        <h4>
          {t('accounts.autoSwitchHeading')}
          <FieldReset path={['autoSwitch']} />
        </h4>
        <label className="switch">
          <input type="checkbox" checked={settings.autoSwitch} onChange={(e) => props.onSettings({ ...settings, autoSwitch: e.target.checked })} />
          {t('accounts.autoSwitch')}
        </label>
        <span className="hint">{t(places.length > 1 ? 'accounts.autoSwitchHintPlaces' : 'accounts.autoSwitchHint')}</span>
      </section>
      {usage.accounts === null && <p className="hint">{t('accounts.loading')}</p>}
      {places.length === 1 && providers(places[0])}
      {places.length > 1 &&
        places.map((distro) => (
          <section key={distro ?? ''} className="place-group">
            <div className="place-head" translate="no">
              {distro === null ? <Monitor aria-hidden="true" /> : <SquareTerminal aria-hidden="true" />}
              {placeName(distro)}
              <span className="hint">{t(distro === null ? 'place.pcUses' : 'place.wslUses')}</span>
            </div>
            {providers(distro)}
          </section>
        ))}
      {login && (
        <LoginDialog
          login={login}
          viewVersion={props.viewVersion}
          onDone={() => {
            setLogin(null);
            usage.refresh();
          }}
        />
      )}
    </>
  );
}

type AccountActions = {
  openLogin: (provider: Provider, distro: string | null, target: LoginTarget, title: string) => void;
  remove: (a: AccountView, id: AccountId) => void;
  logout: (a: AccountView) => void;
  choose: (provider: Provider, distro: string | null, id: AccountId | null) => void;
};

function ProviderAccounts(props: { provider: Provider; distro: string | null; settings: SettingsView; usage: Usage; actions: AccountActions }) {
  const { t } = useTranslation();
  const { provider, distro, usage, actions } = props;
  const label = PROVIDER_LABEL[provider];
  const all = usage.accounts!;
  const accounts = all.filter((a) => a.provider === provider && a.distro === distro);
  const active = activeAccount(all, props.settings, provider, distro);
  const missing = accounts.some((a) => a.id === null && notInstalled(a));
  return (
    <section aria-busy={usage.loading}>
      <h4>
        {label}
        <button
          className="btn ghost sm"
          disabled={missing}
          title={missing ? t('accounts.addNeedsCli', { provider: label }) : undefined}
          onClick={() => actions.openLogin(provider, distro, { kind: 'new' }, t('accounts.addTitle', { provider: label }))}
        >
          <Plus />
          {t('accounts.add')}
        </button>
      </h4>
      <div className="rows">
        {accounts.map((a) => {
          const current = a === active;
          const problem = limitsProblem(a.usage.limits);
          const needsLogin = a.usage.limits.kind === 'needsLogin';
          const system = a.id === null;
          const absent = notInstalled(a);
          const meta = [system ? SYSTEM_DIR[provider] : null, a.usage.plan, !absent ? fetchedText(a.usage.readAt, Date.now()) : null].filter(Boolean).join(' · ');
          return (
            <div className="acct-row" key={a.id ?? 'system'} aria-current={current || undefined}>
              <span className="row-main">
                <span className="name acct-name">{accountName(a)}</span>
                {system && a.usage.email && <span className="badge">{t('usage.systemDefault')}</span>}
                {current && <span className="badge">{t('accounts.current')}</span>}
                {needsLogin && <span className="badge warn">{system ? t('usage.loginRequired') : t('accounts.needsLogin')}</span>}
                {absent && <span className="badge warn">{t('accounts.notInstalled')}</span>}
                <span className="meta" translate="no">
                  {meta}
                </span>
              </span>
              <span className="win">{needsLogin ? <span className="hint">{t('statusBar.loginToSee')}</span> : problem ? <span className="hint">{problem}</span> : <WindowLines limits={a.usage.limits} />}</span>
              <span className="row-actions">
                {!current && !needsLogin && !absent && (
                  <button className="btn sm" onClick={() => actions.choose(provider, distro, a.id)}>
                    {t('accounts.use')}
                  </button>
                )}
                {needsLogin && a.id !== null && (
                  <button className="btn sm" onClick={() => actions.openLogin(provider, distro, { kind: 'account', id: a.id! }, t('accounts.reloginTitle', { name: accountName(a) }))}>
                    {t('accounts.relogin')}
                  </button>
                )}
                {needsLogin && system && (
                  <button className="btn sm" onClick={() => actions.openLogin(provider, distro, { kind: 'system' }, t('accounts.systemLoginTitle', { provider: label }))}>
                    {t('accounts.login')}
                  </button>
                )}
                {system && !needsLogin && !absent && (
                  <button className="btn ghost sm" aria-label={t('accounts.logoutLabel', { name: accountName(a) })} onClick={() => actions.logout(a)}>
                    {t('accounts.logout')}
                  </button>
                )}
                {a.id !== null && (
                  <button className="btn ghost sm" aria-label={t('accounts.removeLabel', { name: accountName(a) })} onClick={() => actions.remove(a, a.id!)}>
                    {t('accounts.remove')}
                  </button>
                )}
              </span>
            </div>
          );
        })}
      </div>
    </section>
  );
}

function LoginDialog(props: { login: OpenLogin; viewVersion: number; onDone: () => void }) {
  const { t } = useTranslation();
  const { login } = props;
  const [failed, setFailed] = useState(false);
  const onDone = useRef(props.onDone);
  onDone.current = props.onDone;

  useEffect(() => {
    let stopped = false;
    getLogin(login.id).then((state) => {
      if (stopped) return;
      if (state === 'succeeded') closeLogin(login.id).finally(() => onDone.current());
      else if (state === 'failed') setFailed(true);
    }, () => {});
    return () => {
      stopped = true;
    };
  }, [login.id, props.viewVersion]);

  const cancel = () => closeLogin(login.id).finally(() => onDone.current());

  return (
    <Modal className="pop login" label={login.title} onClose={cancel}>
      <h3>
        {login.title}
        <span className="spacer" />
        <button className="btn ghost sm" onClick={cancel}>
          {failed ? t('common.close') : t('common.cancel')}
        </button>
      </h3>
      <div className="login-term" translate="no">
        <SizingTerminal src={loginPty(login.id)} node={null} focused onFocus={() => {}} onNavigate={() => {}} />
      </div>
      {failed ? (
        <p className="error" role="alert">
          {t('accounts.loginFailed')}
        </p>
      ) : (
        <span className="hint">{t(login.target.kind === 'new' ? 'accounts.loginHint' : 'accounts.reloginHint')}</span>
      )}
    </Modal>
  );
}
