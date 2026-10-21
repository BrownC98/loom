import { ChevronDown, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { dismissWait } from './api';
import type { HumanWait } from './gen/HumanWait';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import { useShowNodes } from './Graph';
import { useOutsideClose } from './popover';

type Waiting = NodeView & { wait: HumanWait };

/** 사람 응답을 기다리는 노드, 최근 것부터. 받은편지함·시스템 알림은 `NodeView.wait`만 읽는다(docs/design.md 「사람 응답 대기」). */
export function waitingNodes(nodes: ReadonlyMap<NodeId, NodeView>): Waiting[] {
  return [...nodes.values()].filter((v): v is Waiting => v.wait !== null).sort((a, b) => b.wait.since - a.wait.since);
}

/**
 * 그래프 칸 위 가운데에 뜨는 [질문 N] 알약과 그 목록(목업 docs/mockups/question-inbox-place.html ②). 기다리는 노드가 없으면 알약도 없다.
 * 줄이나 시스템 알림을 누르면 사이드바 클릭처럼 그래프를 그 노드로 옮기고 고른다. `notify`는 설정 「시스템 알림」이다.
 */
export function Inbox(props: { waiting: Waiting[]; notify: boolean; onSelect: (id: NodeId) => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const show = useShowNodes();
  const go = (id: NodeId) => {
    show([id]);
    props.onSelect(id);
  };
  useOsNotify(props.waiting, props.notify, go);
  const home = useOutsideClose<HTMLDivElement>(() => setOpen(false));
  const n = props.waiting.length;
  const latest = props.waiting[0];

  // 마지막 대기가 사라지면 목록을 닫는다. 다음 질문이 생길 때 목록이 저절로 열리지 않는다.
  useEffect(() => {
    if (n === 0) setOpen(false);
  }, [n]);

  useEffect(() => {
    if (!open) return;
    const close = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('keydown', close);
    return () => document.removeEventListener('keydown', close);
  }, [open]);

  return (
    <div className="inbox" ref={home}>
      {/* 알약이 나타나거나 수가 바뀌면 화면 읽기 프로그램이 읽는다. 목록은 이 영역 밖이라 열어도 읽지 않는다. */}
      <div className="inbox-live" aria-live="polite">
        {latest && (
          <button type="button" className="inbox-pill" aria-expanded={open} aria-label={t('inbox.buttonLabel', { count: n })} onClick={() => setOpen(!open)}>
            <span className="dot waiting" aria-hidden="true" />
            <b>
              {t('inbox.button')} {n}
            </b>
            <span className="preview">
              {latest.name}: {latest.wait.text || t(`wait.${latest.wait.kind}`)}
            </span>
            <ChevronDown className={open ? 'chev open' : 'chev'} aria-hidden="true" />
          </button>
        )}
      </div>
      {open && n > 0 && (
        <div className="pop inbox-pop" role="dialog" aria-label={t('inbox.title', { count: n })}>
          <h2>{t('inbox.title', { count: n })}</h2>
          <ul>
            {props.waiting.map((v) => (
              <li key={v.id}>
                <button
                  className="go"
                  onClick={() => {
                    setOpen(false);
                    go(v.id);
                  }}
                >
                  <span className="dot waiting" aria-hidden="true" />
                  <span className="who">
                    <b>{v.name}</b> · {t(`wait.${v.wait.kind}`)}
                  </span>
                  {v.wait.text && <span className="line">{v.wait.text}</span>}
                </button>
                <DismissButton id={v.id} />
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

/** 줄의 [×]. 사람이 그 노드 터미널에 키를 누른 것처럼 대기를 지운다. 카드의 「응답 대기」도 같은 값이라 함께 사라진다. 거절되면 툴팁이 이유를 보인다. */
function DismissButton(props: { id: NodeId }) {
  const { t } = useTranslation();
  const [refusal, setRefusal] = useState<string | null>(null);
  return (
    <button
      type="button"
      className="dismiss"
      aria-label={t('inbox.dismiss')}
      title={refusal ?? t('inbox.dismiss')}
      data-refused={refusal !== null || undefined}
      onClick={() => {
        setRefusal(null);
        dismissWait(props.id).catch((err: Error) => setRefusal(err.message));
      }}
    >
      <X aria-hidden="true" />
    </button>
  );
}

/**
 * 설정이 켜져 있고 창이 보이지 않을 때, 새로 생긴 대기마다 시스템 알림을 띄운다. 누르면 창을 앞으로 가져오고 그 노드로 간다.
 * 허용은 설정을 켤 때 묻는다(`requestNotifyPermission`). 화면을 열 때는 묻지 않는다.
 */
function useOsNotify(waiting: Waiting[], enabled: boolean, onSelect: (id: NodeId) => void) {
  const { t } = useTranslation();
  // 이미 알린 대기(노드와 시작 시각). 화면을 연 뒤 처음 본 대기는 알리지 않는다.
  const seen = useRef<Set<string> | null>(null);
  const select = useRef(onSelect);
  select.current = onSelect;
  useEffect(() => {
    const keys = new Set(waiting.map((v) => `${v.id}:${v.wait.since}`));
    const before = seen.current;
    seen.current = keys;
    if (before === null || !enabled || !document.hidden || typeof Notification === 'undefined' || Notification.permission !== 'granted') return;
    for (const v of waiting) {
      if (before.has(`${v.id}:${v.wait.since}`)) continue;
      const note = new Notification(`${v.name} · ${t(`wait.${v.wait.kind}`)}`, { body: v.wait.text, tag: `loom-wait-${v.id}` });
      note.onclick = () => {
        window.focus();
        select.current(v.id);
        note.close();
      };
    }
  }, [waiting, enabled]);
}

/** 설정을 켤 때 부른다(사람의 클릭). 허용되면 true. 브라우저가 알림을 지원하지 않거나 막으면 false. */
export async function requestNotifyPermission(): Promise<boolean> {
  if (typeof Notification === 'undefined') return false;
  if (Notification.permission === 'granted') return true;
  return (await Notification.requestPermission()) === 'granted';
}
