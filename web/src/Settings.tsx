import { Plus, RotateCw } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { closeLogin, getLogin, loginPty, logoutSystem, removeAccount, startLogin } from './api';
import { canBypass } from './catalog';
import type { AlwaysOn } from './gen/AlwaysOn';
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { LoginId } from './gen/LoginId';
import type { LoginTarget } from './gen/LoginTarget';
import type { Provider } from './gen/Provider';
import type { SettingsView } from './gen/SettingsView';
import type { Usage } from './StatusBar';
import { Modal } from './Modal';
import { SizingTerminal } from './Terminal';
import { TerminalTab, UiTab } from './ThemeSettings';
import type { Updates } from './Update';
import { VERSION } from './version';
import { WorkspaceTab } from './WorkspaceSettings';
import { accountName, activeAccount, fetchedText, limitsProblem, notInstalled, PROVIDER_LABEL, PROVIDERS, SYSTEM_DIR, WindowLines } from './usage';

/** 탭 순서이자 URL 해시 값(`#settings/ui`)이다. */
export const SETTINGS_TABS = ['agents', 'accounts', 'workspace', 'ui', 'terminal', 'about'] as const;
export type SettingsTab = (typeof SETTINGS_TABS)[number];
/** 로그인 창이 로그인이 끝났는지 묻는 간격. */
const LOGIN_POLL_MS = 1000;

/** 설정 창(목업 3·4·6번). 열린 탭은 URL 해시(`#settings/agents`)가 갖는다. */
export function Settings(props: {
  tab: SettingsTab;
  onTab: (tab: SettingsTab) => void;
  onClose: () => void;
  agents: AgentView[];
  onRescan: () => void;
  settings: SettingsView;
  onSettings: (next: SettingsView) => void;
  usage: Usage;
  onChoose: (provider: Provider, id: AccountId | null) => void;
  updates: Updates;
}) {
  const { t } = useTranslation();
  return (
    <Modal className="dialog" label={t('settings.title')} onClose={props.onClose}>
      <nav aria-label={t('settings.tabsLabel')}>
        <h2>{t('settings.title')}</h2>
        {SETTINGS_TABS.map((tab) => (
          <button key={tab} className="tab" aria-current={props.tab === tab ? 'page' : undefined} onClick={() => props.onTab(tab)}>
            {t(`settings.tab.${tab}`)}
          </button>
        ))}
      </nav>
      <div className="pane">
        <div className="pane-head">
          <h3>{t(`settings.tab.${props.tab}`)}</h3>
          <span className="spacer" />
          <button className="btn ghost close" aria-label={t('common.close')} onClick={props.onClose}>
            ×
          </button>
        </div>
        {props.tab === 'agents' && <AgentsTab agents={props.agents} onRescan={props.onRescan} settings={props.settings} onSettings={props.onSettings} />}
        {props.tab === 'accounts' && <AccountsTab settings={props.settings} onSettings={props.onSettings} usage={props.usage} onChoose={props.onChoose} />}
        {props.tab === 'workspace' && <WorkspaceTab settings={props.settings} onSettings={props.onSettings} />}
        {props.tab === 'ui' && <UiTab settings={props.settings} onSettings={props.onSettings} />}
        {props.tab === 'terminal' && <TerminalTab settings={props.settings} onSettings={props.onSettings} />}
        {props.tab === 'about' && <AboutTab updates={props.updates} />}
      </div>
    </Modal>
  );
}

/** 판과 업데이트(docs/mockups/web-update.html ④). [업데이트]는 알림 카드와 같은 일을 하고, 진행은 그 카드가 보인다. */
function AboutTab({ updates }: { updates: Updates }) {
  const { t } = useTranslation();
  const [checking, setChecking] = useState(false);
  // 이 탭에서 확인한 결과. 확인하기 전에는 아무것도 보이지 않는다.
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
  const installed = agents.filter((a) => a.path !== null);
  const missing = agents.filter((a) => a.path === null);
  const toggle = (agent: AgentView, on: boolean) => {
    const rest = settings.bypassOff.filter((id) => id !== agent.id);
    props.onSettings({ ...settings, bypassOff: on ? rest : [...rest, agent.id] });
  };
  return (
    <>
      <section>
        <h4>
          {t('settings.installed', { n: installed.length })}
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
                  <span className="name">{a.label}</span>
                  {a.profile && a.versionWarning && <span className="badge warn">{t(`settings.versionWarning.${a.profile}`, a.versionWarning)}</span>}
                  <span className="meta" translate="no">
                    {a.path} · {a.profile ? t('settings.messaging') : t('settings.terminalOnly')}
                  </span>
                </span>
                <a className="hint" href={a.homepage} target="_blank" rel="noreferrer">
                  {t('settings.docs')}
                </a>
                {a.canBypass ? (
                  <label className="switch">
                    <input type="checkbox" checked={canBypass(a, settings)} aria-label={t('settings.bypassLabel', { agent: a.label })} onChange={(e) => toggle(a, e.target.checked)} />
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
                {a.label}
              </a>
            ))}
          </div>
        </details>
      </section>
      <SkillsSection settings={settings} onSettings={props.onSettings} />
      <TreeLimitsSection settings={settings} onSettings={props.onSettings} />
    </>
  );
}

/** 「내장 스킬」 줄 순서(목업 skill-options.html ①). */
const SKILLS = ['oMode', 'userDoc', 'ste100', 'raptor3'] as const satisfies readonly (keyof AlwaysOn)[];

/** 「내장 스킬」 구역(목업 skill-options.html ①). 끈 스킬은 새로 띄우는 노드에 주지 않는다. */
function SkillsSection({ settings, onSettings }: { settings: SettingsView; onSettings: (next: SettingsView) => void }) {
  const { t } = useTranslation();
  return (
    <section>
      <h4>{t('settings.skills')}</h4>
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

/** 숫자 칸이 받는 범위(목업 settings.html 9번). */
const DEPTH_RANGE = { min: 0, max: 9 };
const CHILDREN_RANGE = { min: 1, max: 99 };

/** 범위 안의 정수면 그 값, 아니면 null. */
function inRange(text: string, range: { min: number; max: number }): number | null {
  const n = Number(text);
  return Number.isInteger(n) && n >= range.min && n <= range.max && text.trim() !== '' ? n : null;
}

/** 「하위 노드 상한」 구역(목업 settings.html 9번). 범위 안의 값으로 바꾸면 바로 저장한다.
 *  칸은 처음 값만 받는다. 저장할 때마다 다시 그리면 입력 중 커서를 잃는다. */
function TreeLimitsSection({ settings, onSettings }: { settings: SettingsView; onSettings: (next: SettingsView) => void }) {
  const { t } = useTranslation();
  const limits = settings.treeLimits;
  const setDepth = (text: string) => {
    // 비우면 상한이 없다(2026-10-20 사람 지시 "에이전트 중첩깊이 공란이면 상한 없음으로 처리"). 자식 수 칸과 같다.
    const maxDepth = text.trim() === '' ? null : inRange(text, DEPTH_RANGE);
    if (maxDepth !== null || text.trim() === '') onSettings({ ...settings, treeLimits: { ...limits, maxDepth } });
  };
  const setChildren = (text: string) => {
    const maxChildren = text.trim() === '' ? null : inRange(text, CHILDREN_RANGE);
    if (maxChildren !== null || text.trim() === '') onSettings({ ...settings, treeLimits: { ...limits, maxChildren } });
  };
  return (
    <section>
      <h4>{t('settings.treeLimits')}</h4>
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

function AccountsTab(props: { settings: SettingsView; onSettings: (next: SettingsView) => void; usage: Usage; onChoose: (provider: Provider, id: AccountId | null) => void }) {
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
  const openLogin = (provider: Provider, target: LoginTarget, title: string) =>
    run(async () => setLogin({ target, title, id: await startLogin(provider, target) }));
  const remove = (a: AccountView, id: AccountId) => {
    if (!confirm(t('accounts.confirmRemove', { name: accountName(a) }))) return;
    run(async () => {
      await removeAccount(a.provider, id);
      usage.refresh();
    });
  };
  const logout = (a: AccountView) => {
    if (!confirm(t('accounts.confirmLogout', { provider: PROVIDER_LABEL[a.provider], name: accountName(a) }))) return;
    run(async () => {
      await logoutSystem(a.provider);
      usage.refresh();
    });
  };

  return (
    <>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="auto-switch">
        <label className="switch">
          <input type="checkbox" checked={settings.autoSwitch} onChange={(e) => props.onSettings({ ...settings, autoSwitch: e.target.checked })} />
          {t('accounts.autoSwitch')}
        </label>
        <span className="hint">{t('accounts.autoSwitchHint')}</span>
      </div>
      {usage.accounts === null && <p className="hint">{t('accounts.loading')}</p>}
      {usage.accounts &&
        PROVIDERS.map((provider) => {
          const label = PROVIDER_LABEL[provider];
          const accounts = usage.accounts!.filter((a) => a.provider === provider);
          const active = activeAccount(usage.accounts!, settings, provider);
          // CLI가 없으면 모든 줄이 같은 문제다. 시스템 기본 줄로 본다.
          const missing = accounts.some((a) => a.id === null && notInstalled(a));
          return (
            <section key={provider} aria-busy={usage.loading}>
              <h4>
                {label}
                <button
                  className="btn ghost sm"
                  disabled={missing}
                  title={missing ? t('accounts.addNeedsCli', { provider: label }) : undefined}
                  onClick={() => openLogin(provider, { kind: 'new' }, t('accounts.addTitle', { provider: label }))}
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
                  const meta = [system ? SYSTEM_DIR[provider] : null, a.usage.plan, usage.fetchedAt !== null && !absent ? fetchedText(usage.fetchedAt, Date.now()) : null].filter(Boolean).join(' · ');
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
                          <button className="btn sm" onClick={() => props.onChoose(provider, a.id)}>
                            {t('accounts.use')}
                          </button>
                        )}
                        {needsLogin && a.id !== null && (
                          <button className="btn sm" onClick={() => openLogin(provider, { kind: 'account', id: a.id! }, t('accounts.reloginTitle', { name: accountName(a) }))}>
                            {t('accounts.relogin')}
                          </button>
                        )}
                        {needsLogin && system && (
                          <button className="btn sm" onClick={() => openLogin(provider, { kind: 'system' }, t('accounts.systemLoginTitle', { provider: label }))}>
                            {t('accounts.login')}
                          </button>
                        )}
                        {system && !needsLogin && !absent && (
                          <button className="btn ghost sm" aria-label={t('accounts.logoutLabel', { name: accountName(a) })} onClick={() => logout(a)}>
                            {t('accounts.logout')}
                          </button>
                        )}
                        {a.id !== null && (
                          <button className="btn ghost sm" aria-label={t('accounts.removeLabel', { name: accountName(a) })} onClick={() => remove(a, a.id!)}>
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
        })}
      {login && (
        <LoginDialog
          login={login}
          onDone={() => {
            setLogin(null);
            usage.refresh();
          }}
        />
      )}
    </>
  );
}

/** CLI 로그인을 띄운 터미널. 로그인이 끝나면 닫힌다. 취소하면 데몬이 새로 만든 폴더를 지운다. */
function LoginDialog(props: { login: OpenLogin; onDone: () => void }) {
  const { t } = useTranslation();
  const { login } = props;
  const [failed, setFailed] = useState(false);
  const onDone = useRef(props.onDone);
  onDone.current = props.onDone;

  useEffect(() => {
    let stopped = false;
    const finish = () => {
      stopped = true;
      closeLogin(login.id).finally(() => onDone.current());
    };
    const timer = window.setInterval(async () => {
      const state = await getLogin(login.id).catch(() => 'failed' as const);
      if (stopped) return;
      if (state === 'succeeded') finish();
      else if (state === 'failed') {
        stopped = true;
        setFailed(true);
      }
    }, LOGIN_POLL_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [login.id]);

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
        <SizingTerminal src={loginPty(login.id)} focused onFocus={() => {}} onNavigate={() => {}} />
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
