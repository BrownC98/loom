import { CircleCheck, CircleQuestionMark, type LucideIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { ScreenWait } from './gen/ScreenWait';
import type { NodeView } from './gen/NodeView';
import type { WaitId } from './gen/WaitId';
import type { WaitKind } from './gen/WaitKind';
import type { WaitTone } from './gen/WaitTone';

export const QuestionIcon: LucideIcon = CircleQuestionMark;

export const ICON: Record<WaitTone, LucideIcon> = { ask: QuestionIcon, done: CircleCheck };

export const showsId = (kind: WaitKind) => kind === 'question';

export const bySince = (a: ScreenWait, b: ScreenWait) => a.since - b.since;

const oldest = (waits: readonly ScreenWait[]): ScreenWait => waits.reduce((a, b) => (bySince(b, a) < 0 ? b : a));

export const oldestWait = (v: NodeView): ScreenWait => oldest(v.waits);

export const newestId = (waits: readonly ScreenWait[]): WaitId => Math.max(...waits.map((w) => w.id));

export const pillClosed = (closed: WaitId | null, waits: readonly ScreenWait[]) => closed !== null && waits.every((w) => w.id <= closed);

export const asking = (view: NodeView) => view.waits.filter((w) => w.tone === 'ask');

export const minimapClass = (v: NodeView): string => {
  if (v.waits.length > 0) return oldestWait(v).tone;
  return v.state.kind === 'exited' || v.state.kind === 'setupFailed' ? v.state.kind : '';
};

const TONES: readonly WaitTone[] = ['ask', 'done'];

export const countsByTone = (waits: readonly ScreenWait[]) => TONES.map((tone) => ({ tone, n: waits.filter((w) => w.tone === tone).length })).filter((c) => c.n > 0);

export function NoticeBadges(props: { waits: readonly ScreenWait[] }) {
  return (
    <>
      {countsByTone(props.waits).map(({ tone, n }) => {
        const Icon = ICON[tone];
        return (
          <span key={tone} className="notice-count" data-tone={tone}>
            <Icon aria-hidden="true" />
            {n}
          </span>
        );
      })}
    </>
  );
}

export function NoticeChips(props: { view: NodeView }) {
  const { t } = useTranslation();
  const asks = asking(props.view);
  const dones = props.view.waits.length - asks.length;
  if (asks.length + dones === 0) return null;
  return (
    <span className="chips">
      {asks.length > 0 && (
        <span className="state-tag" data-tone="ask">
          {t(`wait.${oldest(asks).kind}`)} {asks.length}
        </span>
      )}
      {dones > 0 && (
        <span className="state-tag" data-tone="done">
          {t('tree.done', { n: dones })}
        </span>
      )}
    </span>
  );
}

export function NoticeLead(props: { wait: ScreenWait; waits: readonly ScreenWait[] }) {
  const { t } = useTranslation();
  const { wait } = props;
  const Icon = ICON[wait.tone];
  return (
    <>
      <Icon className="qicon" aria-hidden="true" />
      <b>{t(`wait.${wait.kind}`)}</b>
      <NoticeBadges waits={props.waits} />
      {showsId(wait.kind) && <span className="qid">Q{wait.id}</span>}
    </>
  );
}

export function NodeText(props: { name: string; wait: ScreenWait }) {
  const text = props.wait.summary;
  return (
    <>
      <b>{props.name}</b>
      {text && `: ${text}`}
    </>
  );
}
