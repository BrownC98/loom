import { ArrowRight, ArrowUp, ChevronDown, ExternalLink, Image as ImageIcon, ImagePlus, X } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState, type DOMAttributes, type ReactNode, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import type { QuestionPill } from './gen/QuestionPill';
import { isTauri } from '@tauri-apps/api/core';
import { answerWait, dismissWait, doneImageUrl, openWaitDoc, PASTE_LIMIT_BYTES, savePastedFile, waitDocPass, waitDocUrl } from './api';
import { choices } from './choices';
import { dateAndTime } from './i18n';
import { notify, notifyAllowed, windowHidden } from './osNotify';
import type { HumanWait } from './gen/HumanWait';
import type { NodeId } from './gen/NodeId';
import type { WaitId } from './gen/WaitId';
import type { NodeView } from './gen/NodeView';
import { useShowNodes } from './Graph';
import { stateClass } from './NodeCard';
import { bySince, ICON, NodeText, NoticeBadges, NoticeLead, oldestWait, showsId, summaryOf, toneOf } from './notice';
import { Modal } from './Modal';
import { useDismiss } from './popover';
import { linkParts } from './links';
import { openLink, openPass, phoneBrowser } from './openLink';

/**
 * 사람 응답을 기다리는 노드, 가장 오래된 대기 순으로. 알약은 첫 노드의 가장 오래된 대기를 보인다. 새 질문이 와도 알약이 가리키는 대기가 바뀌지 않는다.
 * 받은편지함·시스템 알림은 `NodeView.waits`만 읽는다(docs/design.md 「사람 응답 대기」).
 */
export function waitingNodes(nodes: ReadonlyMap<NodeId, NodeView>): NodeView[] {
  return [...nodes.values()].filter((v) => v.waits.length > 0).sort((a, b) => bySince(oldestWait(a), oldestWait(b)));
}

/** 「알림」 탭 배지의 수: 모든 노드의 대기 수(목업 M28: 노드 수가 아니라 알림 수). */
export const waitCount = (waiting: NodeView[]) => waiting.reduce((n, v) => n + v.waits.length, 0);

/**
 * 그래프 칸 위 가운데에 뜨는 [질문 N] 알약과 그 목록(목업 docs/mockups/question-inbox-place.html ②, question-pill-tone.html M31 ②). 기다리는 노드가 없으면 알약도 없다.
 * 알약은 펼치지 않아도 가장 오래된 대기 하나의 전문과 답 칸을 보인다. 목록 줄의 노드 이름과 시스템 알림을 누르면 사이드바 클릭처럼 그래프를 그 노드로 옮기고 고른다.
 * 알약 머리를 누르면 목록이 열린다. `notify`는 설정 「시스템 알림」이다.
 */
export function Inbox(props: { waiting: NodeView[]; notify: boolean; pill: QuestionPill; onSelect: (id: NodeId) => void; onFocus: (id: NodeId) => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const show = useShowNodes();
  const go = (id: NodeId) => {
    show([id]);
    props.onSelect(id);
  };
  useOsNotify(props.waiting, props.notify, go);
  const pill = useRef<HTMLDivElement>(null);
  const waits = props.waiting.flatMap((v) => v.waits);
  const n = waits.length;
  const first = props.waiting[0];
  const firstWait = first && oldestWait(first);
  const line = props.pill === 'line';
  const toList = () => setOpen(!open);

  // 마지막 대기가 사라지면 목록을 닫는다. 다음 질문이 생길 때 목록이 저절로 열리지 않는다.
  useEffect(() => {
    if (n === 0) setOpen(false);
  }, [n]);

  return (
    <div className="inbox">
      {/* 알약 꼴은 설정 「질문 알약」이다. `line`: 한 줄. `answer`: 가장 오래된 대기 하나와 답 칸(목업 docs/mockups/question-pill-tone.html M31 ②). 답하면 다음으로 오래된 대기가 그 자리에 온다. */}
      {first && (
        <div ref={pill} className="inbox-pill" data-view={props.pill} data-tone={toneOf(firstWait.kind)}>
          <PillHead node={first} wait={firstWait} waits={waits} line={line} open={open} onToggle={toList} />
          {!line && (
            <div className="pill-row">
              <WaitEntry key={firstWait.id} node={first.id} wait={firstWait} onGo={() => go(first.id)} />
            </div>
          )}
        </div>
      )}
      {open && n > 0 && (
        <InboxPop label={t('inbox.title', { count: n })} pill={pill} onClose={() => setOpen(false)}>
          {/* 노드로 가도 목록은 열린 채로 둔다. 이어서 다른 질문을 보고 답한다(2026-10-22 사람 지시 "총괄클릭해서 이동할때 팝오버 유지되도록"). */}
          <WaitList
            waiting={props.waiting}
            onGo={go}
            onFocus={(id) => {
              show([id]);
              props.onFocus(id);
            }}
          />
        </InboxPop>
      )}
    </div>
  );
}

/** 알약이 연 목록. 열린 동안만 밖 누르기와 Esc가 닫는다. 알약(답 칸 포함)을 누르는 것은 밖이 아니다. */
function InboxPop(props: { label: string; pill: RefObject<HTMLDivElement | null>; onClose: () => void; children: ReactNode }) {
  const box = useDismiss<HTMLDivElement>(props.onClose, { anchor: props.pill });
  return (
    <div ref={box} className="pop inbox-pop" role="dialog" aria-label={props.label}>
      <h2>{props.label}</h2>
      {props.children}
    </div>
  );
}

/** 알약 머리 줄. `node`·`wait`는 가장 오래된 대기(`waitingNodes`의 첫 노드와 `oldestWait`), `waits`는 모든 대기다. */
function PillHead(props: { node: NodeView; wait: HumanWait; waits: readonly HumanWait[]; line: boolean; open: boolean; onToggle: () => void }) {
  const { t } = useTranslation();
  const { wait, open } = props;
  return (
    // 머리 줄이 바뀌면(새 알약, 수, 다음 알림) 화면 읽기 프로그램이 읽는다. 답 칸은 이 영역 밖이다.
    <div className="head" aria-live="polite">
      <NoticeLead wait={wait} waits={props.waits} />
      {/* 알약 머리는 목록만 편다. 노드로 가는 길은 목록 줄의 노드 이름이다(2026-10-22 사람 질문 "알약에 터미널 바로가기가 필요한가?"). */}
      <button type="button" className="go" aria-expanded={open} aria-label={t('inbox.listLabel', { count: props.waits.length })} onClick={props.onToggle}>
        {props.line ? <NodeText name={props.node.name} wait={wait} /> : <b>{props.node.name}</b>}
        {!props.line && <ChevronDown className={open ? 'chev open' : 'chev'} aria-hidden="true" />}
      </button>
    </div>
  );
}

/** 오른쪽 사이드바 「질문」 탭(2026-10-21 사람 지시 "예비용으로 질문함은 … 우측 사이드 챗에 유지해줘"). 알약 목록과 같은 줄을 보인다. 대기가 없으면 한 줄만 보인다. */
export function InboxPanel(props: { waiting: NodeView[]; onSelect: (id: NodeId) => void; onFocus: (id: NodeId) => void }) {
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
      onFocus={(id) => {
        show([id]);
        props.onFocus(id);
      }}
    />
  );
}

/**
 * 대기 목록. 알약 목록과 「질문」 탭이 함께 쓴다(목업 docs/mockups/multi-question.html M28, question-id.html M25 v3).
 * 노드로 묶는다: 노드 이름은 한 번만 보이고(누르면 `onGo`), 그 아래에 대기가 물은 차례대로 있다. 질문이 여럿이면 이름 옆에 수를 보인다.
 */
function WaitList(props: { waiting: NodeView[]; onGo: (id: NodeId) => void; onFocus: (id: NodeId) => void }) {
  const { t } = useTranslation();
  return (
    <ul className="wait-list">
      {props.waiting.map((v) => (
        <li key={v.id} className="wait-node" data-tone={toneOf(oldestWait(v).kind)}>
          <button type="button" className="go" aria-label={t('inbox.jumpLabel', { name: v.name })} onClick={() => props.onGo(v.id)}>
            <span className={`dot ${stateClass(v)}`} aria-hidden="true" />
            <b>{v.name}</b>
            <NoticeBadges waits={v.waits} />
          </button>
          <ul className="wait-rows">
            {[...v.waits].sort(bySince).map((w) => (
              <WaitRow key={w.id} node={v.id} wait={w} onGo={() => props.onGo(v.id)} onFocus={() => props.onFocus(v.id)} />
            ))}
          </ul>
        </li>
      ))}
    </ul>
  );
}

/**
 * 목록의 대기 한 줄(`WaitEntry`). 줄의 빈 곳이나 글을 누르면 그 노드로 가서 터미널에 입력 초점을 준다(2026-10-22 사람 지시 "질문 펼쳤을때 질문영역 클릭커블하게 만들고 클릭하면 해당 터미널 포커싱").
 * 단추·입력창·링크는 제 일을 한다. 키보드로는 노드 이름 단추가 같은 곳으로 간다.
 */
function WaitRow(props: { node: NodeId; wait: HumanWait; onGo: () => void; onFocus: () => void }) {
  return (
    <li
      className="wait-row"
      onClick={(e) => {
        if (!(e.target instanceof Element) || e.target.closest('button, input, textarea, a, label')) return;
        // 글을 끌어 고르는 중이면 옮기지 않는다.
        if (window.getSelection()?.toString()) return;
        props.onFocus();
      }}
    >
      <WaitEntry node={props.node} wait={props.wait} onGo={props.onGo} />
    </li>
  );
}

/**
 * 대기 한 건: 머리 줄(아이콘, 종류, 번호, 시각, 오른쪽 위 [×]·[확인])과 본문. 목록 줄과 펼친 알약이 함께 쓴다.
 * 완료는 [확인]만 지운다. 입력·다음 턴·프로세스 끝으로는 지우지 않는다(2026-10-22 사람 지시 "완료 알림은 사용자가 확인 버튼 클릭하면 제거되는 걸로").
 */
function WaitEntry(props: { node: NodeId; wait: HumanWait; onGo: () => void }) {
  const { t } = useTranslation();
  const { wait } = props;
  const tone = toneOf(wait.kind);
  const Icon = ICON[tone];
  return (
    <div className="entry" data-tone={tone}>
      <div className="top">
        <Icon className="kind-mark" aria-hidden="true" />
        <span className="kind">{t(`wait.${wait.kind}`)}</span>
        {showsId(wait.kind) && <span className="qid">Q{wait.id}</span>}
        <Published wait={wait} />
        {tone === 'done' ? <ConfirmButton wait={wait.id} /> : <DismissButton wait={wait.id} />}
      </div>
      {wait.instruction && <Instruction text={wait.instruction} />}
      <WaitBody node={props.node} wait={wait} onGo={props.onGo} />
    </div>
  );
}

/** 알림이 올라온 날짜·시각(2026-10-22 사람 지시 "알림은 알림이 언제 publish 되었는지 날짜,시간을 표시해"). 「알림」 이름표를 붙인다. 옆의 「지시:」 줄 때문에 지시한 시각으로 읽혔다(같은 날 "저기 나온 시간이 지시를 한 시간인지, 알림이 발생한 시간인지 혼동됨"). */
function Published(props: { wait: HumanWait }) {
  const { t } = useTranslation();
  const at = props.wait.since * 1000;
  return <time className="published" dateTime={new Date(at).toISOString()}>{t('inbox.published', { time: dateAndTime(at) })}</time>;
}

/** 완료를 맡긴 사람의 지시(`loom done --asked`)(2026-10-22 사람 지시 "사람이 어떤 지시를 내렸었는지 확인하기 어려움"). 접지 않고 다 보인다(같은 날 "완료 알림도 질문 알림처럼 더보기로 접지말고 항상 원문 다 보여줘"). */
function Instruction(props: { text: string }) {
  const { t } = useTranslation();
  return <p className="instruction">{t('inbox.instruction', { text: props.text })}</p>;
}

/** 그림 썸네일(완료 알림의 그림, 사람이 붙인 그림). 누르면 화면 위에 원래 크기로 띄운다. Esc나 바깥 누르기로 닫는다(2026-10-22 사람 지시 "썸네일 크게보기 기능 추가해줘", "사용자가 입력한 이미지 썸네일도 클릭하면 크게 볼 수 있게 해줘"). */
function ZoomImage(props: { src: string; name: string; title: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" className="zoom" title={props.title} aria-label={t('inbox.zoomImage', { name: props.name })} onClick={() => setOpen(true)}>
        <img src={props.src} alt={props.name} />
      </button>
      {open && (
        <Modal className="image-view" label={props.name} onClose={() => setOpen(false)} outsideClose>
          <img src={props.src} alt={props.name} onClick={() => setOpen(false)} />
        </Modal>
      )}
    </>
  );
}

/** 완료 알림의 [확인]. 그 알림을 지운다. 거절되면 툴팁이 이유를 보인다. */
function ConfirmButton(props: { wait: WaitId }) {
  const { t } = useTranslation();
  const [refusal, setRefusal] = useState<string | null>(null);
  return (
    <button
      type="button"
      className="choice confirm"
      title={refusal ?? undefined}
      data-refused={refusal !== null || undefined}
      onClick={() => {
        setRefusal(null);
        dismissWait(props.wait).catch((err: Error) => setRefusal(err.message));
      }}
    >
      {t('inbox.confirm')}
    </button>
  );
}

/**
 * 대기 본문. 목록 줄과 펼치지 않은 알약이 함께 쓴다. 질문은 전문을 보인다. 글이 칸보다 길면(알약은 4줄) [더 보기]가 생긴다.
 * `loom ask` 질문은 여기서 답한다: 선택지 버튼은 바로 보내고, 입력창은 Enter로 보낸다. 입력창에 그림을 붙여넣거나 본문 위에 끌어 놓으면 답에 붙는다(목업 docs/mockups/answer-image.html M30).
 * 다른 대기(승인 창·폴더 신뢰·로그인)는 그 노드 터미널에서만 답하므로 [터미널에서 답하기]만 둔다.
 */
function WaitBody(props: { node: NodeId; wait: HumanWait; onGo: () => void }) {
  const { t } = useTranslation();
  const { wait } = props;
  const done = toneOf(wait.kind) === 'done';
  const qid = showsId(wait.kind) ? `Q${wait.id}` : t(`wait.${wait.kind}`);
  const [refusal, setRefusal] = useState<string | null>(null);
  const images = useImages(props.node, setRefusal);
  const [dragging, setDragging] = useState(false);
  const send = (text: string) => {
    setRefusal(null);
    answerWait(
      wait.id,
      text,
      images.list.map((i) => i.path),
    ).catch((err: Error) => setRefusal(err.message));
  };
  // 그림 끌어 놓기는 화면에서 답하는 질문만 받는다. 본문 안의 자식으로 옮겨 가도 덮개를 끄지 않는다.
  const drop: DOMAttributes<HTMLDivElement> = wait.answerable
    ? {
        onDragOver: (e) => {
          if (!e.dataTransfer.types.includes('Files')) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = 'copy';
          setDragging(true);
        },
        onDragLeave: (e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
        },
        onDrop: (e) => {
          if (!e.dataTransfer.types.includes('Files')) return;
          e.preventDefault();
          setDragging(false);
          images.attach([...e.dataTransfer.files]);
        },
      }
    : {};
  return (
    <div className="wait-body" data-tone={toneOf(wait.kind)} data-dragging={dragging || undefined} {...drop}>
      {/* 완료: 사람이 그 지시에 준 그림. 답에 붙인 그림과 같은 꼴이다(목업 docs/mockups/answer-image.html M30). */}
      {wait.images.length > 0 && (
        <ul className="answer-images">
          {wait.images.map((path, index) => (
            <li key={path}>
              <ZoomImage src={doneImageUrl(wait.id, index)} name={path.split(/[\\/]/).pop() ?? path} title={path} />
            </li>
          ))}
        </ul>
      )}
      {(wait.text || wait.doc) && (
        <p className="full">
          {linkParts(wait.text).map((part, index) =>
            'href' in part ? (
              <button key={index} type="button" className="doc-link" title={part.href} onClick={() => openLink(part.href, props.node)}>
                {part.label}
              </button>
            ) : (
              part.text
            ),
          )}
          {wait.doc && (
            // 문서는 따로 단추가 아니라 질문 글 끝의 링크다(2026-10-21 사람 지시 "문서 링크는 별도 버튼이 아니라 질문텍스트의 하이퍼링크표시+아이콘 으로 표시한다").
            <>
              {' '}
              {/* 앱 창은 새 탭을 열지 못해 데몬이 기본 브라우저를 연다. 폰 앱은 통행증 주소를 폰 브라우저로 넘긴다. 브라우저와 폰 브라우저는 데몬이 내는 문서를 새 탭에서 연다(목업 docs/mockups/mobile.html ②). */}
              {isTauri() || phoneBrowser() ? (
                <button
                  type="button"
                  className="doc-link"
                  title={t('inbox.openDocLabel', { id: wait.doc.id, title: wait.doc.title })}
                  onClick={() => {
                    const browser = phoneBrowser();
                    (browser ? openPass(waitDocPass(wait.id), browser) : openWaitDoc(wait.id)).catch((err: Error) => setRefusal(err.message));
                  }}
                >
                  {wait.doc.id} {wait.doc.title}
                  <ExternalLink aria-hidden="true" />
                </button>
              ) : (
                <a className="doc-link" href={waitDocUrl(wait.id)} target="_blank" rel="noopener" title={t('inbox.openDocTabLabel', { id: wait.doc.id, title: wait.doc.title })}>
                  {wait.doc.id} {wait.doc.title}
                  <ExternalLink aria-hidden="true" />
                </a>
              )}
            </>
          )}
        </p>
      )}
      {wait.answerable ? (
        <>
          {!done && <Choices question={wait.text} onPick={send} />}
          {images.list.length > 0 && (
            <ul className="answer-images">
              {images.list.map((image) => (
                <li key={image.path}>
                  <ZoomImage src={image.url} name={image.name} title={image.name} />
                  <button type="button" aria-label={t('inbox.removeImage')} title={t('inbox.removeImage')} onClick={() => images.remove(image)}>
                    <X aria-hidden="true" />
                  </button>
                </li>
              ))}
            </ul>
          )}
          <AnswerForm
            qid={qid}
            placeholder={t(done ? 'inbox.followUpPlaceholder' : 'inbox.answerPlaceholder')}
            hasImages={images.list.length > 0}
            onSend={send}
            onPasteImages={images.attach}
          />
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
      {dragging && (
        <div className="drop-cover" aria-hidden="true">
          <ImagePlus />
          {t('inbox.dropImage', { id: qid })}
          <span className="sub">{t('inbox.dropLimit', { limit: PASTE_LIMIT_BYTES / MB })}</span>
        </div>
      )}
    </div>
  );
}

const MB = 1024 * 1024;

/** 답에 붙인 그림 하나. `path`는 데몬 기계의 파일, `url`은 작은 그림을 그리는 브라우저 주소다. */
type AttachedImage = { path: string; url: string; name: string };

/**
 * 답에 붙일 그림들. 그림은 붙이는 대로 데몬에 올린다(`POST /api/pastes`, 터미널 붙여넣기와 같은 폴더와 상한). 그림이 아니거나 크면 받지 않고 이유를 알린다.
 * 줄이 사라지면 브라우저 주소를 놓는다. 데몬의 파일은 OS가 지운다.
 */
function useImages(node: NodeId, onError: (message: string) => void) {
  const { t } = useTranslation();
  const [list, setList] = useState<AttachedImage[]>([]);
  const shown = useRef<AttachedImage[]>([]);
  shown.current = list;
  useEffect(() => () => shown.current.forEach((i) => URL.revokeObjectURL(i.url)), []);
  const attach = (files: File[]) => {
    for (const file of files) {
      if (!file.type.startsWith('image/')) {
        onError(t('inbox.notImage', { name: file.name }));
        continue;
      }
      if (file.size > PASTE_LIMIT_BYTES) {
        onError(t('terminal.tooBig', { name: file.name, size: Math.ceil(file.size / MB), limit: PASTE_LIMIT_BYTES / MB }));
        continue;
      }
      savePastedFile(file, node).then(
        (path) => setList((now) => [...now, { path, url: URL.createObjectURL(file), name: file.name }]),
        (err: Error) => onError(err.message),
      );
    }
  };
  const remove = (image: AttachedImage) => {
    URL.revokeObjectURL(image.url);
    setList((now) => now.filter((i) => i !== image));
  };
  return { list, attach, remove };
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
          {/* 「권장」은 글 바로 뒤에 붙는다. 글이 여러 줄이 되어도 단추 끝으로 밀려나지 않는다(2026-10-22 사람 지시 "권장이 삐져나와 삐뚤빼뚤해짐"). */}
          <span className="label">
            {c.label}
            {c.recommended && <span className="mark">{t('inbox.recommended')}</span>}
          </span>
        </button>
      ))}
    </div>
  );
}

/**
 * 답 입력창. Enter나 [↑]로 보내고 Shift+Enter는 줄을 바꾼다. 창 높이는 줄 수를 따라 늘어난다(2026-10-23 사람 "알림 텍스트 입력에서 쉬프트 엔터로 개행 가능하겍 해줘, 입력창 높이도 행 개수에 맞춰 늘어나게하고").
 * 보내면 질문이 사라지므로 비우지 않는다. 거절되면 글이 남아 다시 보낼 수 있다.
 */
function AnswerForm(props: { qid: string; placeholder: string; hasImages: boolean; onSend: (text: string) => void; onPasteImages: (files: File[]) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState('');
  const box = useRef<HTMLTextAreaElement>(null);
  // 그림만 붙여도 보낼 수 있다(목업 M30).
  const ready = text.trim() !== '' || props.hasImages;
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const fit = () => {
      // 폰 질문 띠의 접힌 패널(`hidden`)에서는 scrollHeight가 0이다. 펼쳐질 때 다시 잰다.
      if (el.offsetParent === null) return;
      el.style.height = 'auto';
      el.style.height = `${el.scrollHeight + el.offsetHeight - el.clientHeight}px`;
    };
    fit();
    const watch = new ResizeObserver(fit);
    watch.observe(el);
    return () => watch.disconnect();
  }, [text]);
  return (
    <form
      className="answer"
      onSubmit={(e) => {
        e.preventDefault();
        if (ready) props.onSend(text);
      }}
    >
      <textarea
        ref={box}
        rows={1}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          // 한글 조합 중의 Enter는 글자를 끝내는 키다. 보내지 않는다.
          if (e.key !== 'Enter' || e.shiftKey || e.nativeEvent.isComposing) return;
          e.preventDefault();
          if (ready) props.onSend(text);
        }}
        onPaste={(e) => {
          const files = [...e.clipboardData.files].filter((f) => f.type.startsWith('image/'));
          if (files.length === 0) return;
          e.preventDefault();
          props.onPasteImages(files);
        }}
        aria-label={t('inbox.answerLabel', { id: props.qid })}
        placeholder={props.placeholder}
      />
      {/* 폰에는 붙여넣기·끌어 놓기가 없어 사진첩을 연다(목업 docs/mockups/mobile.html ②). 데스크톱 화면은 숨긴다(styles.css). */}
      <label className="attach" title={t('inbox.attachImage')}>
        <ImageIcon aria-hidden="true" />
        <input
          type="file"
          accept="image/*"
          multiple
          aria-label={t('inbox.attachImage')}
          onChange={(e) => {
            props.onPasteImages([...(e.target.files ?? [])]);
            e.target.value = '';
          }}
        />
      </label>
      <button type="submit" className="send" aria-label={t('inbox.send', { id: props.qid })} disabled={!ready}>
        <ArrowUp aria-hidden="true" />
      </button>
    </form>
  );
}

/** 줄의 [×]. 그 대기 하나를 지운다. 카드의 「응답 대기」도 같은 값이라 마지막 대기면 함께 사라진다. 거절되면 툴팁이 이유를 보인다. */
function DismissButton(props: { wait: WaitId }) {
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
        dismissWait(props.wait).catch((err: Error) => setRefusal(err.message));
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
function useOsNotify(waiting: NodeView[], enabled: boolean, onSelect: (id: NodeId) => void) {
  const { t } = useTranslation();
  // 이미 알린 대기 번호. 화면을 연 뒤 처음 본 대기는 알리지 않는다.
  const seen = useRef<Set<WaitId> | null>(null);
  const select = useRef(onSelect);
  select.current = onSelect;
  useEffect(() => {
    const keys = new Set(waiting.flatMap((v) => v.waits.map((w) => w.id)));
    const before = seen.current;
    seen.current = keys;
    if (before === null || !enabled || !notifyAllowed()) return;
    const fresh = waiting.flatMap((v) => v.waits.filter((w) => !before.has(w.id)).map((w) => ({ v, w })));
    if (fresh.length === 0) return;
    windowHidden().then((hidden) => {
      if (!hidden) return;
      for (const { v, w } of fresh) notify(`${v.name} · ${t(`wait.${w.kind}`)}`, summaryOf(w), `loom-wait-${w.id}`, () => select.current(v.id));
    });
  }, [waiting, enabled]);
}
