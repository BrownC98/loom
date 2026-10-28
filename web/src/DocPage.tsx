import { Check, Download } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { answerWait, docUrl, getDocQuestion } from './api';
import type { DocQuestion } from './gen/DocQuestion';
import type { WaitId } from './gen/WaitId';
import { dateAndTime } from './i18n';

type Decision = { id: string; keys: string[]; recommended: string | null };
type Pick = { key?: string; text?: string };
type Picks = Record<string, Pick>;
type FromDoc = { loom: 'doc' } & ({ kind: 'decisions'; list: Decision[] } | { kind: 'pick'; id: string; key: string } | { kind: 'write'; id: string; text: string });

const ANSWER_JOIN = ', ';

export function docPageId(): string | null {
  const match = /^\/docs\/([^/]+)$/.exec(location.pathname);
  return match ? decodeURIComponent(match[1]) : null;
}

function answerText(decisions: readonly Decision[], picks: Picks, unpicked: string): string {
  return decisions
    .map((d) => {
      const pick = picks[d.id];
      if (pick?.text?.trim()) return `${d.id}: ${pick.text.trim()}`;
      if (pick?.key) return `${d.id}: ${pick.key}`;
      return d.recommended && `${d.id}: ${d.recommended}(${unpicked})`;
    })
    .filter(Boolean)
    .join(ANSWER_JOIN);
}

function picksOf(text: string, decisions: readonly Decision[]): Picks {
  const picks: Picks = {};
  for (const part of text.split(/, (?=D\d+: )/)) {
    const [id, value] = [part.slice(0, part.indexOf(': ')), part.slice(part.indexOf(': ') + 2)];
    const decision = decisions.find((d) => d.id === id);
    if (!decision) continue;
    const key = decision.keys.find((k) => value === k || value.startsWith(`${k}(`));
    picks[id] = key ? { key } : { text: value };
  }
  return picks;
}

export function DocPage({ id }: { id: string }) {
  const { t } = useTranslation();
  const frame = useRef<HTMLIFrameElement>(null);
  const [question, setQuestion] = useState<DocQuestion | null>(null);
  const [decisions, setDecisions] = useState<Decision[]>([]);
  const [picks, setPicks] = useState<Picks>({});
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);

  useEffect(() => {
    document.title = id;
    getDocQuestion(id).then(setQuestion, (err: Error) => setRefusal(err.message));
  }, [id]);

  useEffect(() => {
    const heard = (e: MessageEvent<FromDoc>) => {
      if (e.source !== frame.current?.contentWindow || e.data?.loom !== 'doc') return;
      const data = e.data;
      if (data.kind === 'decisions') return setDecisions(data.list);
      if (data.kind === 'pick') setPicks((now) => ({ ...now, [data.id]: now[data.id]?.key === data.key ? {} : { key: data.key } }));
      if (data.kind === 'write') setPicks((now) => ({ ...now, [data.id]: data.text ? { text: data.text } : {} }));
      setConfirming(false);
    };
    window.addEventListener('message', heard);
    return () => window.removeEventListener('message', heard);
  }, []);

  const shown = question?.kind === 'answered' ? picksOf(question.text, decisions) : picks;
  const mode = question?.kind === 'open' ? 'open' : question?.kind === 'answered' ? 'locked' : 'off';
  useEffect(() => {
    frame.current?.contentWindow?.postMessage({ loom: 'page', kind: 'show', mode, picks: shown, labels: { picked: t('docPage.picked'), write: t('docPage.write') } }, '*');
  });

  const toDoc = (message: object) => frame.current?.contentWindow?.postMessage({ loom: 'page', ...message }, '*');
  const unpicked = decisions.filter((d) => !picks[d.id]?.key && !picks[d.id]?.text?.trim()).map((d) => d.id);

  const send = (wait: WaitId) => {
    const text = answerText(decisions, picks, t('docPage.unpickedAnswer'));
    setBusy(true);
    answerWait(wait, text, []).then(
      () => setQuestion({ kind: 'answered', wait, text, at: Math.floor(Date.now() / 1000) }),
      (err: Error) => {
        setRefusal(err.message);
        getDocQuestion(id).then(setQuestion, () => {});
      },
    ).finally(() => {
      setBusy(false);
      setConfirming(false);
    });
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
            <span className="bar-title">{decisions.length > 0 ? t('docPage.answering') : t('docPage.noDecisions')}</span>
            {download}
            <span className="spacer" />
            {confirming ? (
              <>
                <span className="unpicked">{t('docPage.unpicked', { ids: unpicked.join(', ') })}</span>
                <button type="button" className="btn" onClick={() => toDoc({ kind: 'focus', id: unpicked[0] })}>
                  {t('docPage.see', { id: unpicked[0] })}
                </button>
                <button type="button" className="btn send" disabled={busy} onClick={() => send(question.wait)}>
                  {t('docPage.sendAnyway')}
                </button>
              </>
            ) : (
              <>
                {decisions.map((d) => {
                  const pick = picks[d.id];
                  const value = pick?.text?.trim() || pick?.key;
                  return (
                    <span key={d.id} className="pick" data-empty={!value || undefined} title={value}>
                      {d.id}: {value ?? '—'}
                    </span>
                  );
                })}
                <button
                  type="button"
                  className="btn send"
                  disabled={busy || decisions.length === 0}
                  onClick={() => (unpicked.length > 0 ? setConfirming(true) : send(question.wait))}
                >
                  {t('docPage.send')}
                </button>
              </>
            )}
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
      <iframe
        ref={frame}
        title={t('docPage.frame', { id })}
        src={docUrl(id)}
        sandbox="allow-scripts allow-popups allow-popups-to-escape-sandbox allow-downloads"
        onLoad={() => toDoc({ kind: 'hello' })}
      />
    </div>
  );
}
