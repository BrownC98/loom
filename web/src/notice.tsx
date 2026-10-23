import { CircleCheck, CircleQuestionMark, type LucideIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { HumanWait } from './gen/HumanWait';
import type { NodeView } from './gen/NodeView';
import type { WaitKind } from './gen/WaitKind';
import { plainText } from './links';

// 모든 화면이 알림을 이 파일 하나로 읽는다(목업 docs/mockups/notice-look.html M9 v2, 승인 Q500·Q502 A).

/** `ask`: 사람이 답할 일(질문·승인·신뢰·로그인·입력). `done`: 완료 알림. */
export type Tone = 'ask' | 'done';

export const toneOf = (kind: WaitKind): Tone => (kind === 'done' ? 'done' : 'ask');

/** 사람 질문을 뜻하는 아이콘은 이것 하나다: 목록 줄, 알약, 폰 띠, 배지(2026-10-22 사람 지시 "물음표 아이콘 모양 통일"). 질문과 완료를 합친 「알림」 탭은 종(`Bell`)이다. */
export const QuestionIcon: LucideIcon = CircleQuestionMark;

export const ICON: Record<Tone, LucideIcon> = { ask: QuestionIcon, done: CircleCheck };

/** 번호(`Q12`)는 사람이 터미널에 「Q12: A」로 답할 수 있는 질문에만 붙는다. */
export const showsId = (kind: WaitKind) => kind === 'question';

/** 한 줄 미리보기 글. 글이 없는 알림(승인 창 등)은 빈 글이다. */
export const summaryOf = (wait: HumanWait) => wait.prompt?.summary ?? plainText(wait.parts ?? []);

/** 오래된 순(2026-10-23 사람 "알림은 오래된 순으로 표시해줘"). */
export const bySince = (a: HumanWait, b: HumanWait) => a.since - b.since;

/** 가장 오래된 대기. 빈 목록은 받지 않는다. */
const oldest = (waits: readonly HumanWait[]): HumanWait => waits.reduce((a, b) => (bySince(b, a) < 0 ? b : a));

/** 노드에서 가장 오래된 대기. 대기가 없는 노드는 받지 않는다(`waitingNodes`가 거른다). */
export const oldestWait = (v: NodeView): HumanWait => oldest(v.waits);

/** 사람의 답을 기다리는 대기. 완료 알림은 답할 일이 없어 뺀다. */
export const asking = (view: NodeView) => view.waits.filter((w) => toneOf(w.kind) === 'ask');

/** 미니맵 칸의 클래스: 가장 오래된 알림의 색. 알림이 없으면 종료·설정 실패만 색을 가진다(사이드바 점과 같은 규칙). */
export const minimapClass = (v: NodeView): string => {
  if (v.waits.length > 0) return toneOf(oldestWait(v).kind);
  return v.state.kind === 'exited' || v.state.kind === 'setupFailed' ? v.state.kind : '';
};

const TONES: readonly Tone[] = ['ask', 'done'];

/** 종류마다 하나씩 센 수. 없는 종류는 뺀다. */
export const countsByTone = (waits: readonly HumanWait[]) => TONES.map((tone) => ({ tone, n: waits.filter((w) => toneOf(w.kind) === tone).length })).filter((c) => c.n > 0);

/** 알약·목록 머리·폰 띠의 수 배지: 종류마다 아이콘과 수 하나, 나란히. */
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

/** 노드 줄·카드 머리·폰 노드 머리의 이름 칩 묶음: 「<가장 오래된 질문의 이름> N」과 「완료 N」. */
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

/** 알약 머리와 폰 띠의 앞 칸: 아이콘, 종류 이름, 수 배지(`waits`는 모든 대기), 번호. */
export function NoticeLead(props: { wait: HumanWait; waits: readonly HumanWait[] }) {
  const { t } = useTranslation();
  const { wait } = props;
  const Icon = ICON[toneOf(wait.kind)];
  return (
    <>
      <Icon className="qicon" aria-hidden="true" />
      <b>{t(`wait.${wait.kind}`)}</b>
      <NoticeBadges waits={props.waits} />
      {showsId(wait.kind) && <span className="qid">Q{wait.id}</span>}
    </>
  );
}

/** 「노드: 글」. 글이 없으면 노드 이름만 보인다. */
export function NodeText(props: { name: string; wait: HumanWait }) {
  const text = summaryOf(props.wait);
  return (
    <>
      <b>{props.name}</b>
      {text && `: ${text}`}
    </>
  );
}
