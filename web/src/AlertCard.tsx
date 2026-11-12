import { useRef, useState } from 'react';
import { ChevronDown, ChevronsDownUp } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { dismissWait } from './api';
import { KindChip } from './AlertLine';
import { startClosing, type Target } from './alertTarget';
import type { ScreenWait } from './gen/ScreenWait';
import type { WaitId } from './gen/WaitId';
import { clockTime } from './i18n';
import { Choices, ConfirmButton, entryClick, Instruction, WaitContent } from './Inbox';
import { Menu } from './Menu';
import { ICON, kindIcon } from './notice';
import { NoticeActions } from './noticeActions';

function ConfirmAll(props: { place: ScreenWait; waits: readonly WaitId[] }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  const arrow = useRef<HTMLButtonElement>(null);
  const Icon = ICON.done;
  return (
    <>
      <button
        ref={arrow}
        type="button"
        className="choice confirm more"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t('composer.confirmMenu')}
        title={refusal ?? undefined}
        data-refused={refusal !== null || undefined}
        onClick={() => setOpen(!open)}
      >
        <ChevronDown aria-hidden="true" />
      </button>
      {open && (
        <Menu anchor={arrow} label={t('composer.confirmMenu')} onClose={() => setOpen(false)}>
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setOpen(false);
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
            <Icon className="done-mark" aria-hidden="true" />
            {t('composer.confirmAll', { count: props.waits.length })}
          </button>
        </Menu>
      )}
    </>
  );
}

export function AlertCard(props: { target: Target; repo: string; onChoose: (key: string) => void; onFold: () => void; onFocus: () => void }) {
  const { t } = useTranslation();
  const { target } = props;
  const wait = target.card;
  if (!wait) return null;
  const done = target.stream.filter((s) => s.wait.tone === 'done').map((s) => s.wait.id);
  const kinds = (kind: ScreenWait['kind']) => target.stream.filter((s) => s.wait.kind === kind).length;
  const close = () => startClosing(wait, [wait.id]);
  const Icon = kindIcon(wait);
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
        <span className="alert-pos">{t('composer.alertTotal', { total: target.stream.length, ask: target.stream.length - done.length, done: kinds('done'), reply: kinds('reply') })}</span>
        <button type="button" className="btn ghost sm fold" aria-label={t('composer.fold')} title={t('composer.fold')} onClick={props.onFold}>
          <ChevronsDownUp aria-hidden="true" />
        </button>
      </div>
      {wait.instruction && <Instruction text={wait.instruction} />}
      <div className="wait-body" data-tone={wait.tone}>
        <WaitContent node={target.view.id} wait={wait} hints onClose={close} />
        {wait.answerable && wait.tone !== 'done' && <Choices list={wait.choices ?? []} onPick={props.onChoose} hints />}
        <NoticeActions node={target.view.id} wait={wait} onClose={close} />
      </div>
      {wait.tone === 'done' && (
        <div className="alert-acts">
          <span className="confirm-split">
            <ConfirmButton wait={wait.id} onClose={close} />
            {done.length > 1 && <ConfirmAll key={wait.id} place={wait} waits={done} />}
          </span>
        </div>
      )}
    </div>
  );
}
