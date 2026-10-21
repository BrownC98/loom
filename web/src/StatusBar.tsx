import { ArrowLeftRight, ChevronDown, ChevronUp, RotateCw } from 'lucide-react';
import { Fragment, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { Provider } from './gen/Provider';
import type { SettingsView } from './gen/SettingsView';
import { noticeOf } from './accountSwitch';
import { weekdayAndTime } from './i18n';
import type { SwitchRecord } from './useSwitchNotice';
import { accountName, activeAccount, fetchedText, InlineWindows, limitsProblem, notInstalled, PROVIDER_LABEL, PROVIDERS, resetText, WindowLines, windowsText } from './usage';
import { AgentMark } from './AgentMark';
import { useOutsideClose } from './popover';
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
export function StatusBar(props: { usage: Usage; settings: SettingsView; onChoose: (provider: Provider, id: AccountId | null) => void; onManage: () => void; notice: { record: SwitchRecord | null; onClose: () => void } }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  // 그래프(React Flow)와 터미널은 mousedown 전파를 막는다. 그래서 다른 팝오버처럼 pointerdown을 먼저 받는다.
  const bar = useOutsideClose<HTMLElement>(() => setOpen(false));
  const { usage } = props;

  useEffect(() => {
    if (!open) return;
    const close = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('keydown', close);
    return () => document.removeEventListener('keydown', close);
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
              ) : notInstalled(a) ? (
                <span>{t('accounts.notInstalled')}</span>
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
        <RotateCw />
      </button>
      {props.notice.record && <SwitchNotice record={props.notice.record} accounts={usage.accounts} onClose={props.notice.onClose} />}
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

/** 가장 새 전환 기록 한 줄. 닫거나 다음 기록이 덮기 전에는 남는다(목업 account-switch.html ③). */
function SwitchNotice(props: { record: SwitchRecord; accounts: AccountView[] | null; onClose: () => void }) {
  const { t } = useTranslation();
  const name = (provider: Provider, id: AccountId | null) => {
    const a = props.accounts?.find((x) => x.provider === provider && x.id === id);
    return a ? accountName(a) : t(id === null ? 'usage.systemDefault' : 'usage.notLoggedIn');
  };
  const { text, warn } = noticeOf(props.record.entry, name, (at) => weekdayAndTime(at * 1000), t);
  return (
    <span className="notice" data-warn={warn || undefined} role="status">
      {!warn && <ArrowLeftRight aria-hidden="true" />}
      {text}
      <button className="x" aria-label={t('switch.notice.close')} onClick={props.onClose}>
        ×
      </button>
    </span>
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
        {props.expanded ? <ChevronUp /> : <ChevronDown />}
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
