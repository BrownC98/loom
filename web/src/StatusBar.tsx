import { ArrowLeftRight, ChevronDown, ChevronUp, RotateCw } from 'lucide-react';
import { Fragment, useRef, useState, type ReactNode, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { CursorView } from './gen/CursorView';
import type { Limits } from './gen/Limits';
import type { Provider } from './gen/Provider';
import type { SettingsView } from './gen/SettingsView';
import { noticeOf } from './accountSwitch';
import { weekdayAndTime } from './i18n';
import type { SwitchRecord } from './useSwitchNotice';
import { accountName, activeAccount, cliMissing, fetchedText, InlineWindows, limitsProblem, oldestRead, PROVIDER_LABEL, PROVIDERS, resetText, WindowLines, windowsText } from './usage';
import { AgentMark } from './AgentMark';
import { Sheet, usePhone } from './Phone';
import { useDismiss } from './popover';
import { VERSION } from './version';

export type Usage = {
  /** 처음 읽기 전에는 null이다. */
  accounts: AccountView[] | null;
  /** 위치마다 Cursor CLI 로그인. 처음 읽기 전에는 null이다. */
  cursor: CursorView[] | null;
  loading: boolean;
  refresh: () => void;
};

/** 화면 맨 아래에 늘 보이는 줄. Orca처럼 모든 공급자의 사용량이 버튼 하나다(목업 usage.html). [설정]은 사이드바에 있다(graph-panels.html 1번). */
export function StatusBar(props: {
  usage: Usage;
  settings: SettingsView;
  /** 고른 저장소의 WSL 배포판. 이 PC 저장소면 null이다. 사용량은 그 위치의 「사용 중」 계정이다(목업 wsl-place.html ⑦). */
  distro: string | null;
  onChoose: (provider: Provider, distro: string | null, id: AccountId | null) => void;
  onManage?: () => void;
  notice: { record: SwitchRecord | null; onClose: () => void };
}) {
  const { t } = useTranslation();
  const phone = usePhone();
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  const { usage } = props;

  const toggle = () => {
    if (!open) usage.refresh();
    setOpen(!open);
  };
  // 고른 저장소의 위치가 아직 목록에 없으면(배포판이 꺼져 있음) 이 PC를 보인다.
  const distro = usage.accounts?.some((a) => a.distro === props.distro) ? props.distro : null;
  const active = (provider: Provider) => (usage.accounts ? activeAccount(usage.accounts, props.settings, provider, distro) : undefined);
  // Cursor는 Codex 뒤다. loom은 Cursor 계정을 바꾸지 않고 그 위치의 CLI 로그인만 보인다(목업 cursor-usage.html, Q666 A).
  const cursor = usage.cursor?.find((c) => c.distro === distro);
  const cursorName = cursor && (cursor.usage.email ?? t('usage.systemDefault'));
  const summary = [
    ...PROVIDERS.map((provider) => {
      const a = active(provider);
      return `${PROVIDER_LABEL[provider]} ${a ? `${accountName(a)}, ${windowsText(a.usage.limits)}` : t('statusBar.summaryLoading')}`;
    }),
    ...(cursor ? [`${CURSOR_LABEL} ${cursorName}, ${windowsText(cursor.usage.limits)}`] : []),
  ].join('; ');

  const body = usage.accounts && (
    <UsageBody
      accounts={usage.accounts.filter((a) => a.distro === distro)}
      active={active}
      usage={usage}
      cursor={cursor}
      onChoose={(provider, id) => props.onChoose(provider, distro, id)}
      onManage={
        props.onManage &&
        (() => {
          setOpen(false);
          props.onManage?.();
        })
      }
    />
  );

  return (
    <footer className="statusbar" aria-label={t('statusBar.label')}>
      <button ref={button} className="usage-btn" data-loading={usage.loading || undefined} aria-expanded={open} aria-label={t('statusBar.buttonLabel', { summary })} onClick={toggle}>
        {PROVIDERS.map((provider, i) => {
          const a = active(provider);
          return (
            <Fragment key={provider}>
              {i > 0 && <span className="sep" aria-hidden="true" />}
              <AgentMark agent={provider} label={PROVIDER_LABEL[provider]} />
              {a === undefined ? <span>{t('statusBar.loading')}</span> : <Segment name={accountName(a)} limits={a.usage.limits} />}
            </Fragment>
          );
        })}
        {cursor && cursorName && (
          <>
            <span className="sep" aria-hidden="true" />
            <AgentMark agent="cursor" label={CURSOR_LABEL} />
            <Segment name={cursorName} limits={cursor.usage.limits} />
          </>
        )}
      </button>
      {/* 팝오버를 열지 않고 사용량을 다시 읽는다(2026-10-20 사람 지시 "하단 사용량 표시 섹션에 사용량 조회 새로고침 버튼 만들어"). */}
      <button className="btn ghost sm icon refresh" data-loading={usage.loading || undefined} disabled={usage.loading} aria-label={t('statusBar.refresh')} title={t('statusBar.refresh')} onClick={usage.refresh}>
        <RotateCw />
      </button>
      {props.notice.record && <SwitchNotice record={props.notice.record} accounts={usage.accounts} onClose={props.notice.onClose} />}
      <span className="app-version">v{VERSION}</span>
      {open && usage.accounts && (phone ? <Sheet label={t('statusBar.usage')} onClose={() => setOpen(false)}>{body}</Sheet> : <UsagePopover anchor={button} onClose={() => setOpen(false)}>{body}</UsagePopover>)}
    </footer>
  );
}

const CURSOR_LABEL = 'Cursor';

/** 상태 줄의 공급자 하나: 이름과 창, 또는 볼 수 없는 이유. */
function Segment(props: { name: string; limits: Limits }) {
  const { t } = useTranslation();
  const { limits } = props;
  if (limits.kind === 'needsLogin') return <span>{t('statusBar.loginToSee')}</span>;
  if (cliMissing(limits)) return <span>{t('accounts.notInstalled')}</span>;
  return (
    <>
      <span className="acct-name">{props.name}</span>
      <InlineWindows limits={limits} />
      {limits.kind === 'failed' && <span className="seg-err">{t('statusBar.unavailable')}</span>}
    </>
  );
}

/** 가장 새 전환 기록 한 줄. 닫거나 다음 기록이 덮기 전에는 남는다(목업 account-switch.html ③). */
function SwitchNotice(props: { record: SwitchRecord; accounts: AccountView[] | null; onClose: () => void }) {
  const { t } = useTranslation();
  const name = (provider: Provider, id: AccountId | null) => {
    const a = props.accounts?.find((x) => x.provider === provider && x.distro === props.record.entry.distro && x.id === id);
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

/** 데스크톱 팝오버. 폰은 같은 몸통을 공용 `Sheet`에 넣는다. */
function UsagePopover(props: { anchor: RefObject<HTMLButtonElement | null>; onClose: () => void; children: ReactNode }) {
  const { t } = useTranslation();
  const box = useDismiss<HTMLDivElement>(props.onClose, { anchor: props.anchor });
  return (
    <div ref={box} className="pop usage-pop" role="dialog" aria-label={t('statusBar.usage')}>
      <h3>
        {t('statusBar.usage')}
        <span className="spacer" />
        <button type="button" className="collapse" aria-label={t('statusBar.collapse')} title={t('statusBar.collapse')} onClick={props.onClose}>
          <ChevronDown aria-hidden="true" />
        </button>
      </h3>
      {props.children}
    </div>
  );
}

function UsageBody(props: {
  accounts: AccountView[];
  active: (provider: Provider) => AccountView | undefined;
  cursor: CursorView | undefined;
  usage: Usage;
  onChoose: (provider: Provider, id: AccountId | null) => void;
  /** 없으면 [계정 관리]를 두지 않는다. 폰에는 설정 창이 없다(목업 mobile.html ⑧). */
  onManage?: () => void;
}) {
  const { t } = useTranslation();
  const { usage } = props;
  const read = oldestRead(usage.accounts, usage.cursor);
  /** 계정 목록을 펼친 공급자. Orca는 공급자 줄을 누르면 계정 바꾸기를 연다. */
  const [expanded, setExpanded] = useState<Provider | null>(null);
  return (
    <>
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
      {props.cursor && <CursorSection usage={props.cursor.usage} />}
      <div className="usage-foot">
        {read !== null && fetchedText(read, Date.now())}
        {props.onManage && (
          <button className="btn ghost sm" onClick={props.onManage}>
            {t('statusBar.manage')}
          </button>
        )}
      </div>
    </>
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

/** Cursor 절. 계정 목록이 없어 펼치지 않는다. 창은 Orca 순서다: 묶음 뒤에 플랜(M18 v2, Q666 A). */
function CursorSection(props: { usage: CursorView['usage'] }) {
  const { t } = useTranslation();
  const { limits } = props.usage;
  const problem = limitsProblem(limits);
  const reset = resetText(limits);
  return (
    <section className="prov-sec" aria-label={t('statusBar.usageLabel', { provider: CURSOR_LABEL })}>
      <div className="prov plain">
        <AgentMark agent="cursor" label={CURSOR_LABEL} />
        <span className="acct-name">{props.usage.email ?? t('usage.systemDefault')}</span>
      </div>
      <div className="win" aria-live="polite">
        <WindowLines limits={limits} />
        {problem && <span className="error">{problem}</span>}
        {reset && <span className="reset">{reset}</span>}
      </div>
    </section>
  );
}
