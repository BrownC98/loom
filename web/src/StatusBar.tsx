import { ArrowLeftRight, ChevronDown, ChevronUp, RotateCw } from 'lucide-react';
import { Fragment, useRef, useState, type ReactNode, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import type { AccountId } from './gen/AccountId';
import type { AccountView } from './gen/AccountView';
import type { PlaceUsage } from './gen/PlaceUsage';
import type { Limits } from './gen/Limits';
import type { Provider } from './gen/Provider';
import type { SettingsView } from './gen/SettingsView';
import { noticeOf } from './accountSwitch';
import { weekdayAndTime } from './i18n';
import type { SwitchRecord } from './useSwitchNotice';
import { accountName, activeAccount, cliMissing, fetchedText, InlineWindows, limitsProblem, oldestRead, PROVIDER_LABEL, PROVIDERS, resetText, WindowLines, windowsText } from './usage';
import { AgentMark } from './AgentMark';
import { Sheet, usePhone } from './sheet';
import { useDismiss } from './popover';
import { VERSION } from './version';

export type Usage = {
  accounts: AccountView[] | null;
  cursor: PlaceUsage[] | null;
  loading: boolean;
  refresh: () => void;
};

export function StatusBar(props: {
  usage: Usage;
  settings: SettingsView;
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
  const distro = usage.accounts?.some((a) => a.distro === props.distro) ? props.distro : null;
  const active = (provider: Provider) => (usage.accounts ? activeAccount(usage.accounts, props.settings, provider, distro) : undefined);
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
  cursor: PlaceUsage | undefined;
  usage: Usage;
  onChoose: (provider: Provider, id: AccountId | null) => void;
  onManage?: () => void;
}) {
  const { t } = useTranslation();
  const { usage } = props;
  const read = oldestRead(usage.accounts, usage.cursor);
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

function CursorSection(props: { usage: PlaceUsage['usage'] }) {
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
