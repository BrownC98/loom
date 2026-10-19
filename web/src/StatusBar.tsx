import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { Provider } from './gen/Provider';
import type { SettingsView } from './gen/SettingsView';
import { accountName, activeAccount, fetchedText, InlineWindows, limitsProblem, PROVIDER_LABEL, PROVIDERS, resetText, WindowLines, windowsText } from './usage';

export type Usage = {
  /** 처음 읽기 전에는 null이다. */
  accounts: AccountView[] | null;
  /** 마지막으로 읽은 시각(ms). */
  fetchedAt: number | null;
  loading: boolean;
  refresh: () => void;
};

/** 화면 맨 아래에 늘 보이는 줄: 공급자마다 계정과 남은 사용량(목업 1·2번). [설정]은 사이드바에 있다(graph-panels.html 1번). */
export function StatusBar(props: { usage: Usage; settings: SettingsView; onChoose: (provider: Provider, id: AccountId | null) => void; onManage: () => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState<Provider | null>(null);
  const bar = useRef<HTMLElement>(null);
  const { usage } = props;

  useEffect(() => {
    if (open === null) return;
    const close = (e: Event) => {
      if (e instanceof KeyboardEvent ? e.key === 'Escape' : !bar.current?.contains(e.target as Node)) setOpen(null);
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', close);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', close);
    };
  }, [open]);

  const toggle = (provider: Provider) => {
    if (open === provider) return setOpen(null);
    setOpen(provider);
    usage.refresh();
  };

  return (
    <footer className="statusbar" aria-label={t('statusBar.label')} ref={bar}>
      {PROVIDERS.map((provider, i) => {
        const active = usage.accounts ? activeAccount(usage.accounts, props.settings, provider) : undefined;
        const label = PROVIDER_LABEL[provider];
        const summary = active ? `${accountName(active)}, ${windowsText(active.usage.limits)}` : t('statusBar.summaryLoading');
        return (
          <div className="seg" key={provider}>
            {i > 0 && <span className="sep" aria-hidden="true" />}
            <button
              className="seg-btn"
              data-loading={usage.loading || undefined}
              aria-expanded={open === provider}
              aria-label={t('statusBar.segLabel', { provider: label, summary })}
              onClick={() => toggle(provider)}
            >
              <span className="who">{label}</span>
              {active === undefined ? (
                <span>{t('statusBar.loading')}</span>
              ) : active.usage.limits.kind === 'needsLogin' ? (
                <span>{t('statusBar.loginToSee')}</span>
              ) : (
                <>
                  <span className="acct-name">{accountName(active)}</span>
                  <InlineWindows limits={active.usage.limits} />
                  {active.usage.limits.kind === 'failed' && <span className="seg-err">{t('statusBar.unavailable')}</span>}
                </>
              )}
            </button>
            {open === provider && usage.accounts && (
              <UsagePopover
                provider={provider}
                accounts={usage.accounts.filter((a) => a.provider === provider)}
                active={active}
                usage={usage}
                onChoose={(id) => props.onChoose(provider, id)}
                onManage={() => {
                  setOpen(null);
                  props.onManage();
                }}
              />
            )}
          </div>
        );
      })}
    </footer>
  );
}

function UsagePopover(props: { provider: Provider; accounts: AccountView[]; active: AccountView | undefined; usage: Usage; onChoose: (id: AccountId | null) => void; onManage: () => void }) {
  const { t } = useTranslation();
  const { active, usage } = props;
  const label = PROVIDER_LABEL[props.provider];
  const problem = active && limitsProblem(active.usage.limits);
  const notes = [active && resetText(active.usage.limits), usage.fetchedAt !== null && fetchedText(usage.fetchedAt, Date.now())].filter(Boolean);
  return (
    <div className="pop usage-pop" role="dialog" aria-label={t('statusBar.usageLabel', { provider: label })}>
      <h3>
        {label}
        <span className="spacer" />
        <button className="btn ghost sm" aria-label={t('statusBar.refresh')} disabled={usage.loading} onClick={usage.refresh}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5" />
          </svg>
        </button>
      </h3>
      <div className="win" aria-live="polite">
        {active && <WindowLines limits={active.usage.limits} />}
        {problem && <span className="error">{problem}</span>}
        {notes.length > 0 && <span className="reset">{notes.join(' · ')}</span>}
      </div>
      <hr className="divider" />
      <ul className="acct-list" aria-label={t('statusBar.accountsLabel', { provider: label })}>
        {props.accounts.map((a) => {
          const current = a === active;
          const session = a.usage.limits.kind === 'ready' ? a.usage.limits.session : null;
          return (
            <li key={a.id ?? 'system'}>
              <button className="acct" aria-current={current || undefined} onClick={() => props.onChoose(a.id)}>
                <span className="acct-name">{accountName(a)}</span>
                <span className="sub">{current ? t('accounts.current') : session ? t('usage.left', { n: session.remaining }) : a.usage.limits.kind === 'needsLogin' ? t('usage.loginRequired') : ''}</span>
              </button>
            </li>
          );
        })}
      </ul>
      <hr className="divider" />
      <button className="btn ghost sm manage" onClick={props.onManage}>
        {t('statusBar.manage')}
      </button>
    </div>
  );
}
