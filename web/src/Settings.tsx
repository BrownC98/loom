import { useEffect, useRef, useState, type ReactNode } from 'react';
import { closeLogin, getLogin, loginPty, removeAccount, startLogin } from './api';
import { defaultAgent, enabledAgents } from './catalog';
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { AgentView } from './gen/AgentView';
import type { LoginId } from './gen/LoginId';
import type { Provider } from './gen/Provider';
import type { SettingsView } from './gen/SettingsView';
import type { Usage } from './StatusBar';
import { Terminal } from './Terminal';
import { TerminalTab, UiTab } from './ThemeSettings';
import { accountName, activeAccount, fetchedText, limitsProblem, PROVIDER_LABEL, PROVIDERS, SYSTEM_DIR, WindowLines, withAccount } from './usage';

/** 탭 순서이자 URL 해시 값(`#settings/ui`)이다. */
export const SETTINGS_TABS = ['agents', 'accounts', 'ui', 'terminal'] as const;
export type SettingsTab = (typeof SETTINGS_TABS)[number];
const TAB_LABEL: Record<SettingsTab, string> = { agents: '에이전트', accounts: '계정', ui: 'UI', terminal: '터미널' };
/** 로그인 창이 로그인이 끝났는지 묻는 간격. */
const LOGIN_POLL_MS = 1000;

/** 화면 최상층에 띄운 창(`<dialog>`). Esc와 포커스 가두기는 브라우저가 한다. */
function Modal(props: { className: string; label: string; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null);
  const onClose = useRef(props.onClose);
  onClose.current = props.onClose;
  useEffect(() => {
    const dialog = ref.current!;
    dialog.showModal();
    const close = (e: Event) => {
      e.preventDefault();
      onClose.current();
    };
    dialog.addEventListener('cancel', close);
    return () => dialog.removeEventListener('cancel', close);
  }, []);
  return (
    <dialog ref={ref} className={props.className} aria-label={props.label}>
      {props.children}
    </dialog>
  );
}

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
}) {
  return (
    <Modal className="dialog" label="설정" onClose={props.onClose}>
      <nav aria-label="설정 탭">
        <h2>설정</h2>
        {SETTINGS_TABS.map((tab) => (
          <button key={tab} className="tab" aria-current={props.tab === tab ? 'page' : undefined} onClick={() => props.onTab(tab)}>
            {TAB_LABEL[tab]}
          </button>
        ))}
      </nav>
      <div className="pane">
        <div className="pane-head">
          <h3>{TAB_LABEL[props.tab]}</h3>
          <span className="spacer" />
          <button className="btn ghost close" aria-label="닫기" onClick={props.onClose}>
            ×
          </button>
        </div>
        {props.tab === 'agents' && <AgentsTab agents={props.agents} onRescan={props.onRescan} settings={props.settings} onSettings={props.onSettings} />}
        {props.tab === 'accounts' && <AccountsTab settings={props.settings} onSettings={props.onSettings} usage={props.usage} />}
        {props.tab === 'ui' && <UiTab settings={props.settings} onSettings={props.onSettings} />}
        {props.tab === 'terminal' && <TerminalTab settings={props.settings} onSettings={props.onSettings} />}
      </div>
    </Modal>
  );
}

function AgentsTab(props: { agents: AgentView[]; onRescan: () => void; settings: SettingsView; onSettings: (next: SettingsView) => void }) {
  const { agents, settings } = props;
  const installed = agents.filter((a) => a.path !== null);
  const missing = agents.filter((a) => a.path === null);
  const enabled = enabledAgents(agents, settings);
  const chosen = defaultAgent(agents, settings);
  const toggle = (agent: AgentView, on: boolean) => {
    const rest = settings.disabledAgents.filter((id) => id !== agent.id);
    props.onSettings({ ...settings, disabledAgents: on ? rest : [...rest, agent.id] });
  };
  return (
    <>
      <section>
        <h4>
          <label htmlFor="s-default">기본 에이전트</label>
        </h4>
        <div className="field narrow">
          <select id="s-default" value={chosen?.id ?? ''} disabled={enabled.length === 0} onChange={(e) => props.onSettings({ ...settings, defaultAgent: e.target.value })}>
            {enabled.length === 0 && <option value="">켠 에이전트가 없다</option>}
            {enabled.map((a) => (
              <option key={a.id} value={a.id}>
                {a.label}
              </option>
            ))}
          </select>
        </div>
        <span className="hint">새 노드 창이 처음에 고른 에이전트다.</span>
      </section>
      <section>
        <h4>
          설치됨 {installed.length}개
          <button className="btn ghost sm" onClick={props.onRescan}>
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5" />
            </svg>
            다시 찾기
          </button>
        </h4>
        {installed.length === 0 ? (
          <p className="hint">데몬 PATH에서 찾은 에이전트가 없다. 아래 목록에서 하나를 설치한 뒤 [다시 찾기]를 누른다.</p>
        ) : (
          <div className="rows">
            {installed.map((a) => {
              const on = !settings.disabledAgents.includes(a.id);
              return (
                <div className="row" key={a.id}>
                  <span className="row-main">
                    <span className="name">{a.label}</span>
                    {a.id === chosen?.id && <span className="badge">기본</span>}
                    {!a.takesBrief && <span className="badge warn">첫 지시문 없음</span>}
                    {a.versionWarning && <span className="badge warn">{a.versionWarning}</span>}
                    <span className="meta" translate="no">
                      {a.path} · {a.profile ? '메시지 연동' : 'loom wait만'}
                    </span>
                  </span>
                  <a className="hint" href={a.homepage} target="_blank" rel="noreferrer">
                    문서
                  </a>
                  <label className="switch">
                    <input type="checkbox" checked={on} aria-label={`${a.label} 켜기`} onChange={(e) => toggle(a, e.target.checked)} />
                    {on ? '켜짐' : '꺼짐'}
                  </label>
                </div>
              );
            })}
          </div>
        )}
      </section>
      <section>
        <details className="more">
          <summary>설치하지 않음 {missing.length}개</summary>
          <div className="cols-2">
            {missing.map((a) => (
              <a key={a.id} href={a.homepage} target="_blank" rel="noreferrer">
                {a.label}
              </a>
            ))}
          </div>
        </details>
      </section>
    </>
  );
}

type OpenLogin = { provider: Provider; id: LoginId; title: string };

function AccountsTab(props: { settings: SettingsView; onSettings: (next: SettingsView) => void; usage: Usage }) {
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
  const openLogin = (provider: Provider, id: AccountId | null, title: string) =>
    run(async () => setLogin({ provider, title, id: await startLogin(provider, { id }) }));
  const remove = (a: AccountView, id: AccountId) => {
    if (!confirm(`${accountName(a)} 계정을 지운다. loom이 만든 로그인 폴더를 지운다.`)) return;
    run(async () => {
      await removeAccount(a.provider, id);
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
      {usage.accounts === null && <p className="hint">계정을 불러오는 중이다.</p>}
      {usage.accounts &&
        PROVIDERS.map((provider) => {
          const label = PROVIDER_LABEL[provider];
          const accounts = usage.accounts!.filter((a) => a.provider === provider);
          const active = activeAccount(usage.accounts!, settings, provider);
          return (
            <section key={provider} aria-busy={usage.loading}>
              <h4>
                {label}
                <button className="btn ghost sm" onClick={() => openLogin(provider, null, `${label} 계정 추가`)}>
                  <svg viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M12 5v14M5 12h14" />
                  </svg>
                  계정 추가
                </button>
              </h4>
              <div className="rows">
                {accounts.map((a) => {
                  const current = a === active;
                  const problem = limitsProblem(a.usage.limits);
                  const needsLogin = a.usage.limits.kind === 'needsLogin';
                  const meta = [a.id === null ? `${SYSTEM_DIR[provider]} · loom이 고치지 않는다` : null, a.usage.plan, usage.fetchedAt !== null ? fetchedText(usage.fetchedAt, Date.now()) : null].filter(Boolean).join(' · ');
                  return (
                    <div className="acct-row" key={a.id ?? 'system'} aria-current={current || undefined}>
                      <span className="row-main">
                        <span className="name acct-name">{accountName(a)}</span>
                        {current && <span className="badge">쓰는 중</span>}
                        {needsLogin && <span className="badge warn">다시 로그인 필요</span>}
                        <span className="meta" translate="no">
                          {meta}
                        </span>
                      </span>
                      <span className="win">{problem && !needsLogin ? <span className="hint">{problem}</span> : <WindowLines limits={a.usage.limits} />}</span>
                      <span className="row-actions">
                        {!current && !needsLogin && (
                          <button className="btn sm" onClick={() => props.onSettings(withAccount(settings, provider, a.id))}>
                            쓰기
                          </button>
                        )}
                        {needsLogin && a.id !== null && (
                          <button className="btn sm" onClick={() => openLogin(provider, a.id, `${accountName(a)} 다시 로그인`)}>
                            다시 로그인
                          </button>
                        )}
                        {a.id !== null && (
                          <button className="btn ghost sm" aria-label={`${accountName(a)} 지우기`} onClick={() => remove(a, a.id!)}>
                            지우기
                          </button>
                        )}
                      </span>
                    </div>
                  );
                })}
              </div>
              {active?.id === null && needsLoginHint(active, provider)}
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

/** 시스템 기본이 로그인하지 않았으면 사용자 셸에서 로그인하는 명령을 보인다. loom은 그 폴더를 고치지 않는다. */
function needsLoginHint(account: AccountView, provider: Provider) {
  if (account.usage.limits.kind !== 'needsLogin') return null;
  const command = provider === 'claude' ? 'claude auth login' : 'codex login';
  return (
    <span className="hint">
      시스템 기본은 사용자 셸에서 <code translate="no">{command}</code>로 로그인한다. 또는 [계정 추가]를 누른다.
    </span>
  );
}

/** CLI 로그인을 띄운 터미널. 로그인이 끝나면 닫힌다. 취소하면 데몬이 새로 만든 폴더를 지운다. */
function LoginDialog(props: { login: OpenLogin; onDone: () => void }) {
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
          {failed ? '닫기' : '취소'}
        </button>
      </h3>
      <div className="login-term" translate="no">
        <Terminal src={loginPty(login.id)} focused onFocus={() => {}} onNavigate={() => {}} />
      </div>
      {failed ? (
        <p className="error" role="alert">
          로그인이 끝나지 않았다. 창을 닫고 다시 시도한다.
        </p>
      ) : (
        <span className="hint">로그인이 끝나면 창이 닫히고 계정이 목록에 생긴다. 취소하면 만든 폴더를 지운다.</span>
      )}
    </Modal>
  );
}
