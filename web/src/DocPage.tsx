import { Check, ChevronRight, Download, MessageSquare } from 'lucide-react';
import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { answerWait, docUrl, getDocQuestion, getDocTalk, postDocTalk } from './api';
import type { DocQuestion } from './gen/DocQuestion';
import type { DocTalk } from './gen/DocTalk';
import { dateAndTime, hourMinute } from './i18n';

type Decision = { id: string; keys: string[]; recommended: string | null };
type Pick = { key?: string; text?: string };
type Picks = Record<string, Pick>;
type FromDoc = { loom: 'doc' } & ({ kind: 'decisions'; list: Decision[] } | { kind: 'pick'; id: string; key: string } | { kind: 'write'; id: string; text: string } | { kind: 'scroll'; y: number });

const ANSWER_JOIN = ', ';
const MESSAGE_JOIN = '\n\n';
const NOTE_JOIN = ' — ';
const POLL_MS = 600;
const CHANGED_SHOWN_MS = 3000;
const FOLDED_KEY = 'loom.docTalkFolded';

export function docPageId(): string | null {
  const match = /^\/docs\/([^/]+)$/.exec(location.pathname);
  return match ? decodeURIComponent(match[1]) : null;
}

function answerText(choice: string | null, decisions: readonly Decision[], picks: Picks, unpicked: string, message: string): string {
  const boxes = decisions
    .map((d) => {
      const value = pickText(picks[d.id]);
      if (value) return `${d.id}: ${value.replace(/\s*\n\s*/g, ' ')}`;
      return d.recommended && `${d.id}: ${d.recommended}(${unpicked})`;
    });
  const picked = [choice, ...boxes].filter(Boolean).join(ANSWER_JOIN);
  return [picked, message.trim()].filter(Boolean).join(MESSAGE_JOIN);
}

function pickText(pick: Pick | undefined): string {
  return [pick?.key, pick?.text?.trim()].filter(Boolean).join(NOTE_JOIN);
}

function picksOf(text: string, decisions: readonly Decision[]): Picks {
  const picks: Picks = {};
  for (const part of text.split(MESSAGE_JOIN)[0].split(/, (?=D\d+: )/)) {
    const [id, value] = [part.slice(0, part.indexOf(': ')), part.slice(part.indexOf(': ') + 2)];
    const decision = decisions.find((d) => d.id === id);
    if (!decision) continue;
    const key = decision.keys.find((k) => value === k || value.startsWith(`${k}(`) || value.startsWith(`${k}${NOTE_JOIN}`));
    if (!key) picks[id] = { text: value };
    else if (value.startsWith(`${key}${NOTE_JOIN}`)) picks[id] = { key, text: value.slice(key.length + NOTE_JOIN.length) };
    else picks[id] = { key };
  }
  return picks;
}

export function DocPage({ id }: { id: string }) {
  const { t } = useTranslation();
  const frame = useRef<HTMLIFrameElement>(null);
  const thread = useRef<HTMLOListElement>(null);
  const scrollY = useRef(0);
  const lastVersion = useRef<string | null>(null);
  const shownVersion = useRef<string | null>(null);
  const [question, setQuestion] = useState<DocQuestion | null>(null);
  const [talk, setTalk] = useState<DocTalk | null>(null);
  const [version, setVersion] = useState<string | null>(null);
  const [changedAt, setChangedAt] = useState<number | null>(null);
  const [decisions, setDecisions] = useState<Decision[]>([]);
  const [picks, setPicks] = useState<Picks>({});
  const [choice, setChoice] = useState<string | null>(null);
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [folded, setFolded] = useState(() => localStorage.getItem(FOLDED_KEY) === '1');

  useEffect(() => {
    document.title = id;
    getDocQuestion(id).then(setQuestion, (err: Error) => setRefusal(err.message));
  }, [id]);

  useEffect(() => {
    let stopped = false;
    const poll = () => {
      if (document.visibilityState === 'hidden') return;
      getDocQuestion(id).then((q) => stopped || setQuestion(q), () => {});
      getDocTalk(id).then(
        (now) => {
          if (stopped) return;
          setTalk(now);
          const stable = now.version === lastVersion.current;
          lastVersion.current = now.version;
          if (shownVersion.current === null) shownVersion.current = now.version;
          if (!stable || shownVersion.current === now.version) return;
          shownVersion.current = now.version;
          setVersion(now.version);
          setChangedAt(Date.now());
        },
        () => {},
      );
    };
    poll();
    const timer = setInterval(poll, POLL_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [id]);

  useEffect(() => {
    if (changedAt === null) return;
    const timer = setTimeout(() => setChangedAt(null), CHANGED_SHOWN_MS);
    return () => clearTimeout(timer);
  }, [changedAt]);

  const lineCount = talk?.lines.length ?? 0;
  useEffect(() => {
    thread.current?.scrollTo({ top: thread.current.scrollHeight });
  }, [lineCount, folded]);

  useEffect(() => {
    const heard = (e: MessageEvent<FromDoc>) => {
      if (e.source !== frame.current?.contentWindow || e.data?.loom !== 'doc') return;
      const data = e.data;
      if (data.kind === 'scroll') scrollY.current = data.y;
      if (data.kind === 'decisions') setDecisions(data.list);
      if (data.kind === 'pick') setPicks((now) => ({ ...now, [data.id]: { ...now[data.id], key: now[data.id]?.key === data.key ? undefined : data.key } }));
      if (data.kind === 'write') setPicks((now) => ({ ...now, [data.id]: { ...now[data.id], text: data.text } }));
    };
    window.addEventListener('message', heard);
    return () => window.removeEventListener('message', heard);
  }, []);

  const openWait = question?.kind === 'open' ? question.wait : null;
  useEffect(() => {
    setChoice(null);
    setPicks({});
  }, [openWait]);

  const shown = question?.kind === 'answered' ? picksOf(question.text, decisions) : picks;
  const mode = question?.kind === 'open' ? 'open' : question?.kind === 'answered' ? 'locked' : 'off';
  useEffect(() => {
    frame.current?.contentWindow?.postMessage({ loom: 'page', kind: 'show', mode, picks: shown, labels: { picked: t('docPage.picked'), write: t('docPage.write') } }, '*');
  });

  const toDoc = (data: object) => frame.current?.contentWindow?.postMessage({ loom: 'page', ...data }, '*');

  const choices = question?.kind === 'open' && decisions.length === 0 ? question.choices : [];
  const picked = choices.some((c) => c.key === choice) ? choice : null;
  const anyPick = picked !== null || decisions.some((d) => pickText(picks[d.id]) !== '');
  const nothingToPick = choices.length === 0 && decisions.length === 0;
  const answering = question?.kind === 'open' && (anyPick || nothingToPick) ? question.wait : null;
  const unpickedWord = t('docPage.unpickedAnswer');
  const preview = answering === null ? '' : answerText(picked, decisions, picks, unpickedWord, '');
  const to = talk?.to ?? null;
  const ready = !busy && (answering !== null ? anyPick || message.trim() !== '' : to !== null && message.trim() !== '');

  const refresh = () => {
    getDocQuestion(id).then(setQuestion, () => {});
    getDocTalk(id).then(setTalk, () => {});
  };

  const send = () => {
    if (!ready) return;
    setBusy(true);
    setRefusal(null);
    const sent = answering !== null ? answerWait(answering, answerText(picked, decisions, picks, unpickedWord, message), []) : postDocTalk(id, message);
    sent
      .then(() => setMessage(''), (err: Error) => setRefusal(err.message))
      .finally(() => {
        setBusy(false);
        refresh();
      });
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key !== 'Enter' || e.shiftKey || e.nativeEvent.isComposing) return;
    e.preventDefault();
    send();
  };

  const fold = (next: boolean) => {
    setFolded(next);
    localStorage.setItem(FOLDED_KEY, next ? '1' : '0');
  };

  const download = (
    <a className="btn ghost" href={`${docUrl(id)}download`} download>
      <Download aria-hidden="true" />
      {t('docPage.download')}
    </a>
  );

  return (
    <div className="doc-page">
      <div className="doc-bar" data-state={question?.kind ?? 'none'}>
        {question?.kind === 'open' && (
          <>
            <span className="qid">Q{question.wait}</span>
            <span className="bar-title">{t('docPage.answering')}</span>
            {download}
            <span className="spacer" />
            {choices.map((c) => (
              <button key={c.key} type="button" className="pick" aria-pressed={choice === c.key} data-empty={choice !== c.key || undefined} title={c.label} onClick={() => setChoice((now) => (now === c.key ? null : c.key))}>
                {c.key}: {c.label}
                {c.recommended && ` · ${t('inbox.recommended')}`}
              </button>
            ))}
            {decisions.map((d) => {
              const value = pickText(picks[d.id]) || null;
              return (
                <button key={d.id} type="button" className="pick" data-empty={!value || undefined} title={value ?? undefined} onClick={() => toDoc({ kind: 'focus', id: d.id })}>
                  {d.id}: {value ?? '—'}
                </button>
              );
            })}
          </>
        )}
        {question?.kind === 'answered' && (
          <>
            <Check className="ok" aria-hidden="true" />
            <span className="bar-title">{t('docPage.answered', { id: `Q${question.wait}`, time: dateAndTime(question.at * 1000) })}</span>
            <span className="sent" title={question.text}>
              {question.text}
            </span>
            <span className="spacer" />
            {download}
          </>
        )}
        {(question === null || question.kind === 'none' || question.kind === 'closed') && (
          <>
            <span className="qid plain">{id}</span>
            {question && <span className="bar-title muted">{question.kind === 'closed' ? t('docPage.closed', { id: `Q${question.wait}` }) : t('docPage.noQuestion')}</span>}
            <span className="spacer" />
            {download}
          </>
        )}
      </div>
      {refusal && (
        <p className="refusal" role="alert">
          {refusal}
        </p>
      )}
      <div className="doc-main">
        <div className="doc-frame">
          <p className="doc-changed" role="status" aria-live="polite">
            {changedAt !== null && (
              <span>
                <Check aria-hidden="true" />
                {t('docPage.changed', { time: hourMinute(changedAt) })}
              </span>
            )}
          </p>
          <iframe
            ref={frame}
            title={t('docPage.frame', { id })}
            src={version === null ? docUrl(id) : `${docUrl(id)}?v=${encodeURIComponent(version)}`}
            sandbox="allow-scripts allow-popups allow-popups-to-escape-sandbox allow-downloads"
            onLoad={() => toDoc({ kind: 'hello', scroll: scrollY.current })}
          />
        </div>
        {folded ? (
          <aside className="doc-talk folded">
            <button type="button" className="btn" onClick={() => fold(false)} aria-label={t('docPage.unfold')}>
              <MessageSquare aria-hidden="true" />
              {t('docPage.talkCount', { n: lineCount })}
            </button>
          </aside>
        ) : (
          <aside className="doc-talk" aria-label={t('docPage.talk')}>
            <div className="talk-head">
              <MessageSquare aria-hidden="true" />
              <b>{t('docPage.talk')}</b>
              {to && <span className="to">{to.name}</span>}
              {to && <span className="state" data-process={to.process.kind}>{t(`docPage.process.${to.process.kind}`)}</span>}
              <span className="spacer" />
              <button type="button" className="btn ghost" onClick={() => fold(true)}>
                <ChevronRight aria-hidden="true" />
                {t('docPage.fold')}
              </button>
            </div>
            <ol className="talk-thread" ref={thread}>
              {lineCount === 0 && <li className="talk-empty">{to ? t('docPage.talkEmpty', { name: to.name }) : t('docPage.noAgent')}</li>}
              {talk?.lines.map((line) => (
                <li key={line.id} data-by={line.said.by}>
                  <p className="talk-text">{line.text}</p>
                  <span className="talk-meta">
                    {line.said.by === 'agent'
                      ? `${line.said.name} · ${hourMinute(line.at * 1000)}`
                      : [hourMinute(line.at * 1000), line.said.answer !== null && t('docPage.asAnswerOf', { id: `Q${line.said.answer}` }), line.said.received ? t('docPage.received') : t('docPage.sentOnly')].filter(Boolean).join(' · ')}
                  </span>
                </li>
              ))}
            </ol>
            <div className="talk-compose">
              {answering !== null && <p className="talk-with">{preview ? t('docPage.asAnswer', { id: `Q${answering}`, text: preview }) : t('docPage.asAnswerOnly', { id: `Q${answering}` })}</p>}
              <textarea rows={3} value={message} disabled={busy || (answering === null && to === null)} onChange={(e) => setMessage(e.target.value)} onKeyDown={onKey} aria-label={t('docPage.message')} placeholder={t('docPage.message')} />
              <span className="talk-row">
                <span className="hint">{t('docPage.keys')}</span>
                <button type="button" className="btn send" disabled={!ready} onClick={send}>
                  {t('docPage.send')}
                </button>
              </span>
            </div>
          </aside>
        )}
      </div>
    </div>
  );
}
