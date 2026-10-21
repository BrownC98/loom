import { ArrowRight, ArrowUp, Check, ChevronDown, Copy, ExternalLink, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { answerWait, dismissWait, openWaitDoc } from './api';
import { choices } from './choices';
import type { HumanWait } from './gen/HumanWait';
import type { NodeId } from './gen/NodeId';
import type { NodeView } from './gen/NodeView';
import { useShowNodes } from './Graph';
import { useOutsideClose } from './popover';

type Waiting = NodeView & { wait: HumanWait };

/** 사람 응답을 기다리는 노드, 오래 기다린 것부터. 알약은 첫 노드를 보인다. 새 질문이 와도 알약이 가리키는 노드가 바뀌지 않는다. 받은편지함·시스템 알림은 `NodeView.wait`만 읽는다(docs/design.md 「사람 응답 대기」). */
export function waitingNodes(nodes: ReadonlyMap<NodeId, NodeView>): Waiting[] {
  return [...nodes.values()].filter((v): v is Waiting => v.wait !== null).sort((a, b) => a.wait.since - b.wait.since);
}

/**
 * 그래프 칸 위 가운데에 뜨는 [질문 N] 알약과 그 목록(목업 docs/mockups/question-inbox-place.html ②, question-pill-many.html ①). 기다리는 노드가 없으면 알약도 없다.
 * 알약 몸통, 줄, 시스템 알림을 누르면 사이드바 클릭처럼 그래프를 그 노드로 옮기고 고른다. 목록은 알약 끝 [▾]만 연다. `notify`는 설정 「시스템 알림」이다.
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
  const first = props.waiting[0];

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
        {first && (
          <div className="inbox-pill">
            <button
              type="button"
              className="jump"
              aria-label={t('inbox.jumpLabel', { name: first.name })}
              onClick={() => {
                setOpen(false);
                go(first.id);
              }}
            >
              <b>{t('inbox.button')}</b>
              <span className="count">{n}</span>
              <span className="qid">Q{first.wait.id}</span>
              <span className="preview">
                {first.name}: {first.wait.text || t(`wait.${first.wait.kind}`)}
              </span>
            </button>
            <button type="button" className="more" aria-expanded={open} aria-label={t('inbox.listLabel', { count: n })} onClick={() => setOpen(!open)}>
              <ChevronDown className={open ? 'chev open' : 'chev'} aria-hidden="true" />
            </button>
          </div>
        )}
      </div>
      {open && n > 0 && (
        <div className="pop inbox-pop" role="dialog" aria-label={t('inbox.title', { count: n })}>
          <h2>{t('inbox.title', { count: n })}</h2>
          <WaitList
            waiting={props.waiting}
            onGo={(id) => {
              setOpen(false);
              go(id);
            }}
          />
        </div>
      )}
    </div>
  );
}

/** 오른쪽 사이드바 「질문」 탭(2026-10-21 사람 지시 "예비용으로 질문함은 … 우측 사이드 챗에 유지해줘"). 알약 목록과 같은 줄을 보인다. 대기가 없으면 한 줄만 보인다. */
export function InboxPanel(props: { waiting: Waiting[]; onSelect: (id: NodeId) => void }) {
  const { t } = useTranslation();
  const show = useShowNodes();
  if (props.waiting.length === 0) return <p className="inbox-empty">{t('inbox.empty')}</p>;
  return (
    <WaitList
      waiting={props.waiting}
      onGo={(id) => {
        show([id]);
        props.onSelect(id);
      }}
    />
  );
}

/**
 * 대기 줄 목록. 알약 목록과 「질문」 탭이 함께 쓴다(목업 docs/mockups/question-id.html v3). 이름을 누르면 `onGo`, [×]는 대기를 지운다.
 * 질문은 전문을 보인다. `loom ask` 질문은 여기서 답한다: 선택지 버튼은 바로 보내고, 입력창은 Enter로 보낸다. 번호 칩은 「Q12: 」를 복사한다.
 * 다른 대기(승인 창·폴더 신뢰·로그인)는 그 노드 터미널에서만 답하므로 [터미널에서 답하기]만 둔다.
 */
function WaitList(props: { waiting: Waiting[]; onGo: (id: NodeId) => void }) {
  return (
    <ul className="wait-list">
      {props.waiting.map((v) => (
        <WaitRow key={v.id} view={v} onGo={() => props.onGo(v.id)} />
      ))}
    </ul>
  );
}

function WaitRow(props: { view: Waiting; onGo: () => void }) {
  const { t } = useTranslation();
  const { wait, name } = props.view;
  const qid = `Q${wait.id}`;
  const [refusal, setRefusal] = useState<string | null>(null);
  const send = (text: string) => {
    setRefusal(null);
    answerWait(wait.id, text).catch((err: Error) => setRefusal(err.message));
  };
  return (
    <li>
      <div className="top">
        <span className="dot waiting" aria-hidden="true" />
        {wait.answerable ? <CopyId qid={qid} /> : <span className="qid">{qid}</span>}
        <button type="button" className="go" aria-label={t('inbox.jumpLabel', { name })} onClick={props.onGo}>
          <b>{name}</b> · {t(`wait.${wait.kind}`)}
        </button>
        <DismissButton id={props.view.id} />
      </div>
      {(wait.text || wait.doc) && (
        <p className="full">
          {wait.text}
          {wait.doc && (
            // 문서는 따로 단추가 아니라 질문 글 끝의 링크다(2026-10-21 사람 지시 "문서 링크는 별도 버튼이 아니라 질문텍스트의 하이퍼링크표시+아이콘 으로 표시한다").
            <>
              {' '}
              <button
                type="button"
                className="doc-link"
                title={t('inbox.openDocLabel', { id: wait.doc.id, title: wait.doc.title })}
                onClick={() => openWaitDoc(wait.id).catch((err: Error) => setRefusal(err.message))}
              >
                {wait.doc.id} {wait.doc.title}
                <ExternalLink aria-hidden="true" />
              </button>
            </>
          )}
        </p>
      )}
      {wait.answerable ? (
        <>
          <Choices question={wait.text} onPick={send} />
          <AnswerForm qid={qid} onSend={send} />
        </>
      ) : (
        <button type="button" className="to-terminal" onClick={props.onGo}>
          {t('inbox.toTerminal')}
          <ArrowRight aria-hidden="true" />
        </button>
      )}
      {refusal && (
        <p className="refusal" role="alert">
          {refusal}
        </p>
      )}
    </li>
  );
}

/** 표준 꼴 질문의 선택지 버튼. 누르면 그 글자(`A`)를 바로 답으로 보낸다. 권장안은 표시한다. */
function Choices(props: { question: string; onPick: (key: string) => void }) {
  const { t } = useTranslation();
  const list = choices(props.question);
  if (list.length === 0) return null;
  return (
    <div className="choices">
      {list.map((c) => (
        <button key={c.key} type="button" className="choice" data-recommended={c.recommended || undefined} onClick={() => props.onPick(c.key)}>
          <span className="key">{c.key}</span>
          {c.label}
          {c.recommended && <span className="mark">{t('inbox.recommended')}</span>}
        </button>
      ))}
    </div>
  );
}

/** 답 입력창. Enter나 [↑]로 보낸다. 보내면 질문이 사라지므로 비우지 않는다. 거절되면 글이 남아 다시 보낼 수 있다. */
function AnswerForm(props: { qid: string; onSend: (text: string) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState('');
  return (
    <form
      className="answer"
      onSubmit={(e) => {
        e.preventDefault();
        if (text.trim()) props.onSend(text);
      }}
    >
      <input value={text} onChange={(e) => setText(e.target.value)} aria-label={t('inbox.answerLabel', { id: props.qid })} placeholder={t('inbox.answerPlaceholder')} />
      <button type="submit" className="send" aria-label={t('inbox.send', { id: props.qid })} disabled={!text.trim()}>
        <ArrowUp aria-hidden="true" />
      </button>
    </form>
  );
}

/** 질문 번호 칩. 누르면 「Q12: 」를 복사하고 잠깐 「Q12: 복사됨」으로 바뀐다. 사람은 노드 터미널에 붙여 「Q12: A」처럼 답한다. */
function CopyId(props: { qid: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), COPIED_SHOWN_MS);
    return () => clearTimeout(timer);
  }, [copied]);
  const label = t('inbox.copyId', { id: props.qid });
  return (
    <button
      type="button"
      className="qid copy"
      data-copied={copied || undefined}
      aria-label={copied ? t('inbox.copied', { id: props.qid }) : label}
      title={label}
      onClick={() => {
        navigator.clipboard.writeText(`${props.qid}: `).then(() => setCopied(true), console.error);
      }}
    >
      {copied ? (
        <>
          <Check aria-hidden="true" />
          {t('inbox.copied', { id: props.qid })}
        </>
      ) : (
        <>
          {props.qid}
          <Copy aria-hidden="true" />
        </>
      )}
    </button>
  );
}

/** 「복사됨」을 보이는 시간. 눌렀음을 알아볼 만큼만 보인다. */
const COPIED_SHOWN_MS = 1500;

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
