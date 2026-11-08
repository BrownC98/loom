import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { dismissWait } from './api';
import { KindChip } from './AlertLine';
import { PROMPT, startClosing, type Target } from './alertTarget';
import type { ScreenWait } from './gen/ScreenWait';
import type { WaitId } from './gen/WaitId';
import { clockTime } from './i18n';
import { Choices, ConfirmButton, DismissButton, entryClick, Instruction, ToTerminal, WaitContent } from './Inbox';
import { ICON } from './notice';

function ConfirmAll(props: { place: ScreenWait; waits: readonly WaitId[] }) {
  const { t } = useTranslation();
  const [refusal, setRefusal] = useState<string | null>(null);
  const Icon = ICON.done;
  return (
    <button
      type="button"
      className="bulk-confirm"
      title={refusal ?? undefined}
      data-refused={refusal !== null || undefined}
      onClick={() => {
        setRefusal(null);
        const cancel = startClosing(props.place, props.waits);
        void Promise.allSettled(props.waits.map(dismissWait)).then((all) => {
          const failed = all.find((r) => r.status === 'rejected');
          if (failed) {
            cancel();
            setRefusal((failed.reason as Error).message);
          }
        });
      }}
    >
      <Icon aria-hidden="true" />
      {t('composer.confirmAll', { count: props.waits.length })}
    </button>
  );
}

export function AlertCard(props: { target: Target; repo: string; onChoose: (key: string) => void; onGo: () => void; onFocus: () => void }) {
  const { t } = useTranslation();
  const { target } = props;
  const wait = target.card;
  if (!wait) return null;
  const done = target.stream.filter((s) => s.wait.tone === 'done').map((s) => s.wait.id);
  const Icon = ICON[wait.tone];
  const keyed = target.key !== PROMPT;
  return (
    <div className="entry alert-card" data-tone={wait.tone} onClick={entryClick(props.onFocus)}>
      <div className="top">
        <Icon className="kind-mark" aria-hidden="true" />
        <KindChip wait={wait} />
        <span className="alert-who">
          <span className="repo">{props.repo} ›</span> {target.view.name}
        </span>
        <time className="published" dateTime={new Date(wait.since * 1000).toISOString()}>
          {clockTime(wait.since * 1000)}
        </time>
        {target.key === PROMPT && <span className="alert-aside">{t('composer.notAnswer')}</span>}
        <span className="alert-pos">{t('composer.alertTotal', { total: target.stream.length, ask: target.stream.length - done.length, done: done.length })}</span>
        {done.length > 0 && <ConfirmAll place={wait} waits={done} />}
        {wait.tone === 'done' ? <ConfirmButton wait={wait.id} onClose={() => startClosing(wait, [wait.id])} /> : <DismissButton wait={wait.id} onClose={() => startClosing(wait, [wait.id])} />}
      </div>
      {wait.instruction && <Instruction text={wait.instruction} />}
      <div className="wait-body" data-tone={wait.tone}>
        <WaitContent node={target.view.id} wait={wait} hints={keyed} />
        {wait.answerable ? wait.tone !== 'done' && <Choices list={wait.choices ?? []} onPick={props.onChoose} hints={keyed} /> : <ToTerminal onGo={props.onGo} />}
      </div>
    </div>
  );
}
