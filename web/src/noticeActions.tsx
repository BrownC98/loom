import { RotateCcw } from 'lucide-react';
import { createContext, useContext, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { dismissWait, pressKeys, ProblemError, relaunchNode } from './api';
import type { AccountView } from './gen/AccountView';
import type { NodeId } from './gen/NodeId';
import type { Press } from './gen/Press';
import type { ScreenWait } from './gen/ScreenWait';
import type { WaitId } from './gen/WaitId';
import type { WaitLimit } from './gen/WaitLimit';
import i18next, { hourMinute } from './i18n';
import { screenOf } from './notice';
import { accountName } from './usage';

export type AccountMoves = { others: (limit: WaitLimit) => AccountView[]; move: (to: AccountView) => Promise<void> };

export const AccountMovesContext = createContext<AccountMoves>({ others: () => [], move: () => Promise.resolve() });

const SCREEN_KEYS: readonly { label: string; press: Press }[] = [
  { label: 'Enter', press: { kind: 'key', value: 'enter' } },
  { label: 'Esc', press: { kind: 'key', value: 'escape' } },
  { label: '←', press: { kind: 'key', value: 'left' } },
  { label: '→', press: { kind: 'key', value: 'right' } },
  { label: '↑', press: { kind: 'key', value: 'up' } },
  { label: '↓', press: { kind: 'key', value: 'down' } },
  { label: 'y', press: { kind: 'char', value: 'y' } },
  { label: 'n', press: { kind: 'char', value: 'n' } },
];

export function pressScreen(wait: WaitId, screen: string[], press: Press): Promise<null> {
  return pressKeys(wait, { screen, press }).catch((err: unknown) => {
    const changed = err instanceof ProblemError && err.problem.code === 'refused' && err.problem.value.code === 'promptChanged';
    throw changed ? new Error(i18next.t('inbox.screenChanged')) : err;
  });
}

type Close = (() => () => void) | undefined;

function useRefusal() {
  const [refusal, setRefusal] = useState<string | null>(null);
  const run = (send: () => Promise<unknown>, onClose?: Close) => {
    setRefusal(null);
    const cancel = onClose?.();
    send().catch((err: Error) => {
      cancel?.();
      setRefusal(err.message);
    });
  };
  const shown = refusal && (
    <p className="refusal" role="alert">
      {refusal}
    </p>
  );
  return { run, shown };
}

function ScreenKeys(props: { wait: WaitId; screen: string[] }) {
  const { t } = useTranslation();
  const { run, shown } = useRefusal();
  return (
    <>
      <pre className="prompt screen" data-find>
        {props.screen.join('\n')}
      </pre>
      <div className="choices keys" role="group" aria-label={t('phone.keys')}>
        {SCREEN_KEYS.map((k) => (
          <button key={k.label} type="button" className="choice" onClick={() => run(() => pressScreen(props.wait, props.screen, k.press))}>
            {k.label}
          </button>
        ))}
      </div>
      {shown}
    </>
  );
}

function CloseChoice(props: { wait: WaitId; onClose: Close; run: ReturnType<typeof useRefusal>['run'] }) {
  const { t } = useTranslation();
  return (
    <button type="button" className="choice" onClick={() => props.run(() => dismissWait(props.wait), props.onClose)}>
      {t('common.close')}
    </button>
  );
}

function StoppedActions(props: { node: NodeId; wait: WaitId; onClose: Close }) {
  const { t } = useTranslation();
  const { run, shown } = useRefusal();
  return (
    <>
      <div className="choices">
        <button type="button" className="choice" data-recommended onClick={() => run(() => relaunchNode(props.node), props.onClose)}>
          <RotateCcw className="choice-icon" aria-hidden="true" />
          {t('node.relaunch')}
        </button>
        <CloseChoice wait={props.wait} onClose={props.onClose} run={run} />
      </div>
      {shown}
    </>
  );
}

const sessionUsed = (a: AccountView) => (a.usage.limits.kind === 'ready' ? (a.usage.limits.session?.used ?? null) : null);

function LimitActions(props: { wait: WaitId; limit: WaitLimit; onClose: Close }) {
  const { t } = useTranslation();
  const moves = useContext(AccountMovesContext);
  const { run, shown } = useRefusal();
  const others = moves.others(props.limit).flatMap((a) => {
    const used = sessionUsed(a);
    return used === null ? [] : [{ a, used }];
  });
  const lowest = Math.min(...others.map((o) => o.used));
  const resets = props.limit.resets_at;
  return (
    <>
      {resets !== null && <p className="limit-resets">{t('inbox.limitResets', { time: hourMinute(resets * 1000) })}</p>}
      <div className="choices">
        {others.map(({ a, used }) => (
          <button key={a.id ?? 'system'} type="button" className="choice" data-recommended={used === lowest || undefined} onClick={() => run(() => moves.move(a), props.onClose)}>
            <span className="label">
              {t('inbox.limitMove', { name: accountName(a), used })}
              {used === lowest && <span className="mark">{t('inbox.recommended')}</span>}
            </span>
          </button>
        ))}
        <CloseChoice wait={props.wait} onClose={props.onClose} run={run} />
      </div>
      {shown}
    </>
  );
}

export function NoticeActions(props: { node: NodeId; wait: ScreenWait; onClose?: () => () => void }) {
  const { wait } = props;
  const screen = screenOf(wait);
  return (
    <>
      {wait.kind === 'stopped' && <StoppedActions node={props.node} wait={wait.id} onClose={props.onClose} />}
      {wait.kind === 'limit' && wait.limit && <LimitActions wait={wait.id} limit={wait.limit} onClose={props.onClose} />}
      {screen && <ScreenKeys wait={wait.id} screen={screen} />}
    </>
  );
}
