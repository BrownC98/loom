import { CircleCheck, CircleQuestionMark, type LucideIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { HumanWait } from './gen/HumanWait';
import type { NodeView } from './gen/NodeView';
import type { WaitId } from './gen/WaitId';
import type { WaitKind } from './gen/WaitKind';
import type { WaitTone } from './gen/WaitTone';

export const QuestionIcon: LucideIcon = CircleQuestionMark;

export const ICON: Record<WaitTone, LucideIcon> = { ask: QuestionIcon, done: CircleCheck };

export const showsId = (kind: WaitKind) => kind === 'question';

export const bySince = (a: HumanWait, b: HumanWait) => a.since - b.since;

const oldest = (waits: readonly HumanWait[]): HumanWait => waits.reduce((a, b) => (bySince(b, a) < 0 ? b : a));

export const oldestWait = (v: NodeView): HumanWait => oldest(v.waits);

export const newestId = (waits: readonly HumanWait[]): WaitId => Math.max(...waits.map((w) => w.id));

export const pillClosed = (closed: WaitId | null, waits: readonly HumanWait[]) => closed !== null && waits.every((w) => w.id <= closed);

export const asking = (view: NodeView) => view.waits.filter((w) => w.tone === 'ask');

export const minimapClass = (v: NodeView): string => {
  if (v.waits.length > 0) return oldestWait(v).tone;
  return v.state.kind === 'exited' || v.state.kind === 'setupFailed' ? v.state.kind : '';
};

const TONES: readonly WaitTone[] = ['ask', 'done'];

export const countsByTone = (waits: readonly HumanWait[]) => TONES.map((tone) => ({ tone, n: waits.filter((w) => w.tone === tone).length })).filter((c) => c.n > 0);

export function NoticeBadges(props: { waits: readonly HumanWait[] }) {
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

export function NoticeLead(props: { wait: HumanWait; waits: readonly HumanWait[] }) {
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

export function NodeText(props: { name: string; wait: HumanWait }) {
  const text = props.wait.summary;
  return (
    <>
      <b>{props.name}</b>
      {text && `: ${text}`}
    </>
  );
}
