import { Fragment, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { Provider } from './gen/Provider';
import type { SettingsView } from './gen/SettingsView';
import { accountName, activeAccount, fetchedText, InlineWindows, limitsProblem, PROVIDER_LABEL, PROVIDERS, resetText, WindowLines, windowsText } from './usage';
import { AgentMark } from './AgentMark';
import { VERSION } from './version';

export type Usage = {
  /** 처음 읽기 전에는 null이다. */
  accounts: AccountView[] | null;
  /** 마지막으로 읽은 시각(ms). */
  fetchedAt: number | null;
  loading: boolean;
  refresh: () => void;
};

/** 화면 맨 아래에 늘 보이는 줄. Orca처럼 모든 공급자의 사용량이 버튼 하나다(목업 usage.html). [설정]은 사이드바에 있다(graph-panels.html 1번). */
export function StatusBar(props: { usage: Usage; settings: SettingsView; onChoose: (provider: Provider, id: AccountId | null) => void; onManage: () => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const bar = useRef<HTMLElement>(null);
  const { usage } = props;

  useEffect(() => {
    if (!open) return;
    const close = (e: Event) => {
      if (e instanceof KeyboardEvent ? e.key === 'Escape' : !bar.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', close);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', close);
    };
  }, [open]);

  const toggle = () => {
    if (!open) usage.refresh();
    setOpen(!open);
  };
  const active = (provider: Provider) => (usage.accounts ? activeAccount(usage.accounts, props.settings, provider) : undefined);
  const summary = PROVIDERS.map((provider) => {
    const a = active(provider);
    return `${PROVIDER_LABEL[provider]} ${a ? `${accountName(a)}, ${windowsText(a.usage.limits)}` : t('statusBar.summaryLoading')}`;
  }).join('; ');

  return (
    <footer className="statusbar" aria-label={t('statusBar.label')} ref={bar}>
      <button className="usage-btn" data-loading={usage.loading || undefined} aria-expanded={open} aria-label={t('statusBar.buttonLabel', { summary })} onClick={toggle}>
        {PROVIDERS.map((provider, i) => {
          const a = active(provider);
          return (
            <Fragment key={provider}>
              {i > 0 && <span className="sep" aria-hidden="true" />}
              <AgentMark agent={provider} label={PROVIDER_LABEL[provider]} />
              {a === undefined ? (
                <span>{t('statusBar.loading')}</span>
              ) : a.usage.limits.kind === 'needsLogin' ? (
                <span>{t('statusBar.loginToSee')}</span>
              ) : (
                <>
                  <span className="acct-name">{accountName(a)}</span>
                  <InlineWindows limits={a.usage.limits} />
                  {a.usage.limits.kind === 'failed' && <span className="seg-err">{t('statusBar.unavailable')}</span>}
                </>
              )}
            </Fragment>
          );
        })}
      </button>
      {/* 팝오버를 열지 않고 사용량을 다시 읽는다(2026-10-20 사람 지시 "하단 사용량 표시 섹션에 사용량 조회 새로고침 버튼 만들어"). */}
      <button className="btn ghost sm icon refresh" data-loading={usage.loading || undefined} disabled={usage.loading} aria-label={t('statusBar.refresh')} title={t('statusBar.refresh')} onClick={usage.refresh}>
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5" />
        </svg>
      </button>
      <span className="app-version">v{VERSION}</span>
      {open && usage.accounts && (
        <UsagePopover
          accounts={usage.accounts}
          active={active}
          usage={usage}
          onChoose={props.onChoose}
          onManage={() => {
            setOpen(false);
            props.onManage();
          }}
        />
      )}
    </footer>
  );
}

function UsagePopover(props: {
  accounts: AccountView[];
  active: (provider: Provider) => AccountView | undefined;
  usage: Usage;
  onChoose: (provider: Provider, id: AccountId | null) => void;
  onManage: () => void;
}) {
  const { t } = useTranslation();
  const { usage } = props;
  /** 계정 목록을 펼친 공급자. Orca는 공급자 줄을 누르면 계정 바꾸기를 연다. */
  const [expanded, setExpanded] = useState<Provider | null>(null);
  return (
    <div className="pop usage-pop" role="dialog" aria-label={t('statusBar.usage')}>
      <h3>{t('statusBar.usage')}</h3>
      {PROVIDERS.map((provider) => (
        <ProviderSection
          key={provider}
          provider={provider}
          accounts={props.accounts.filter((a) => a.provider === provider)}
          active={props.active(provider)}
          expanded={expanded === provider}
          onToggle={() => setExpanded(expanded === provider ? null : provider)}
          onChoose={(id) => props.onChoose(provider, id)}
        />
      ))}
      <div className="usage-foot">
        {usage.fetchedAt !== null && fetchedText(usage.fetchedAt, Date.now())}
        <button className="btn ghost sm" onClick={props.onManage}>
          {t('statusBar.manage')}
        </button>
      </div>
    </div>
  );
}

function ProviderSection(props: {
  provider: Provider;
  accounts: AccountView[];
  active: AccountView | undefined;
  expanded: boolean;
  onToggle: () => void;
  onChoose: (id: AccountId | null) => void;
}) {
  const { t } = useTranslation();
  const { active } = props;
  const label = PROVIDER_LABEL[props.provider];
  const problem = active && limitsProblem(active.usage.limits);
  const reset = active && resetText(active.usage.limits);
  return (
    <section className="prov-sec" aria-label={t('statusBar.usageLabel', { provider: label })}>
      <button className="prov" aria-expanded={props.expanded} onClick={props.onToggle}>
        <AgentMark agent={props.provider} label={label} />
        {active && <span className="acct-name">{accountName(active)}</span>}
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d={props.expanded ? 'M6 15l6-6 6 6' : 'M6 9l6 6 6-6'} />
        </svg>
      </button>
      <div className="win" aria-live="polite">
        {active && <WindowLines limits={active.usage.limits} />}
        {problem && <span className="error">{problem}</span>}
        {reset && <span className="reset">{reset}</span>}
      </div>
      {props.expanded && (
        <ul className="acct-list" aria-label={t('statusBar.accountsLabel', { provider: label })}>
          {props.accounts.map((a) => {
            const current = a === active;
            const session = a.usage.limits.kind === 'ready' ? a.usage.limits.session : null;
            return (
              <li key={a.id ?? 'system'}>
                <button className="acct" aria-current={current || undefined} onClick={() => props.onChoose(a.id)}>
                  <span className="acct-name">{accountName(a)}</span>
                  <span className="sub">{current ? t('accounts.current') : session ? t('usage.used', { n: session.used }) : a.usage.limits.kind === 'needsLogin' ? t('usage.loginRequired') : ''}</span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
