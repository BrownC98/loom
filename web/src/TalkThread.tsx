import { LoaderCircle } from 'lucide-react';
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import type { ProcessState } from './gen/ProcessState';
import type { TalkLine } from './gen/TalkLine';
import { hourMinute } from './i18n';

export function TalkThread({ lines, empty }: { lines: readonly TalkLine[]; empty: string }) {
  const { t } = useTranslation();
  const thread = useRef<HTMLOListElement>(null);
  useEffect(() => {
    thread.current?.scrollTo({ top: thread.current.scrollHeight });
  }, [lines.length]);
  const last = lines.at(-1);
  return (
    <ol className="talk-thread" ref={thread}>
      {lines.length === 0 && <li className="talk-empty">{empty}</li>}
      {lines.map((line) => (
        <li key={line.id} data-by={line.said.by}>
          <p className="talk-text">{line.text}</p>
          <span className="talk-meta">
            {line.said.by === 'agent'
              ? `${line.said.name} · ${hourMinute(line.at * 1000)}`
              : [
                  line.said.by === 'outside' ? t('docPage.outsideTo', { from: line.said.from, to: line.said.to }) : line.said.to,
                  hourMinute(line.at * 1000),
                  line.said.by === 'human' && line.said.answer !== null && t('docPage.asAnswerOf', { id: `Q${line.said.answer}` }),
                  line.said.received ? t('docPage.received') : t('docPage.sentOnly'),
                ]
                  .filter(Boolean)
                  .join(' · ')}
          </span>
        </li>
      ))}
      {last && last.said.by !== 'agent' && last.said.process && <Waiting name={last.said.to} received={last.said.received} process={last.said.process} />}
    </ol>
  );
}

function Waiting({ name, received, process }: { name: string; received: boolean; process: ProcessState }) {
  const { t } = useTranslation();
  if (received && process.kind === 'working')
    return (
      <li data-by="agent" className="talk-typing" role="status">
        <p className="talk-text">
          <LoaderCircle className="spin" aria-hidden="true" />
          {t('docPage.responding', { name })}
        </p>
      </li>
    );
  const blocked = process.kind === 'blocked';
  const state = received ? (blocked ? 'receivedBlocked' : 'restingNoReply') : blocked ? 'queuedBlocked' : process.kind === 'working' ? 'queuedBusy' : 'queuedWaking';
  return (
    <li data-by="human" className="talk-state" role="status">
      <span className="talk-meta">{t(`docPage.waiting.${state}`)}</span>
    </li>
  );
}
