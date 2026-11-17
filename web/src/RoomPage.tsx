import { Check, Link, MessageSquare } from 'lucide-react';
import { useEffect, useState, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { getRoomTalk, postRoomTalk } from './api';
import type { NodeId } from './gen/NodeId';
import type { RoomTalk } from './gen/RoomTalk';
import { TalkThread } from './TalkThread';

const POLL_MS = 600;
const COPIED_SHOWN_MS = 2000;

export function roomPageNode(): NodeId | null {
  const match = /^\/rooms\/(\d+)$/.exec(location.pathname);
  return match ? Number(match[1]) : null;
}

export function RoomPage({ node }: { node: NodeId }) {
  const { t } = useTranslation();
  const [talk, setTalk] = useState<RoomTalk | null>(null);
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  const to = talk?.to ?? null;

  useEffect(() => {
    document.title = to ? t('room.tab', { name: to.name }) : t('room.title');
  }, [to?.name, t]);

  useEffect(() => {
    let stopped = false;
    let timer = 0;
    const next = () => {
      if (!stopped) timer = window.setTimeout(poll, POLL_MS);
    };
    const poll = () => {
      if (document.visibilityState === 'hidden') return next();
      getRoomTalk(node)
        .then((now) => stopped || setTalk(now), (err: Error) => stopped || setRefusal(err.message))
        .finally(next);
    };
    poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [node]);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), COPIED_SHOWN_MS);
    return () => clearTimeout(timer);
  }, [copied]);

  const ready = !busy && to !== null && message.trim() !== '';
  const send = () => {
    if (!ready) return;
    setBusy(true);
    setRefusal(null);
    postRoomTalk(node, message)
      .then(() => setMessage(''))
      .catch((err: Error) => setRefusal(err.message))
      .finally(() => setBusy(false));
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key !== 'Enter' || e.shiftKey || e.nativeEvent.isComposing) return;
    e.preventDefault();
    send();
  };

  const copy = () => navigator.clipboard.writeText(location.href).then(() => setCopied(true), (err: Error) => setRefusal(err.message));
  const placeholder = to ? t('docPage.messageTo', { name: to.name }) : t('room.gone');

  return (
    <div className="doc-page">
      {refusal && (
        <p className="refusal" role="alert">
          {refusal}
        </p>
      )}
      <div className="doc-main room">
        <section className="doc-talk room" aria-label={t('room.title')}>
          <div className="talk-head">
            <MessageSquare aria-hidden="true" />
            <b>{t('room.title')}</b>
            {to && <span className="to">{to.name}</span>}
            {to && <span className="state" data-process={to.process.kind}>{t(`docPage.process.${to.process.kind}`)}</span>}
            <span className="spacer" />
            <button type="button" className="btn ghost" onClick={copy}>
              {copied ? <Check aria-hidden="true" /> : <Link aria-hidden="true" />}
              {copied ? t('room.copied') : t('room.copyLink')}
            </button>
          </div>
          <TalkThread lines={talk?.lines ?? []} empty={to ? t('room.empty', { name: to.name }) : t('room.gone')} />
          <div className="talk-compose">
            <textarea rows={3} value={message} disabled={busy || to === null} onChange={(e) => setMessage(e.target.value)} onKeyDown={onKey} aria-label={placeholder} placeholder={placeholder} />
            <span className="talk-row">
              <span className="hint">{t('docPage.keys')}</span>
              <button type="button" className="btn send" disabled={!ready} onClick={send}>
                {t('docPage.send')}
              </button>
            </span>
          </div>
        </section>
      </div>
    </div>
  );
}
