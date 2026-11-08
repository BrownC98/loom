import type { RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { AlertLine } from './AlertLine';
import type { StreamWait } from './alertTarget';
import type { WaitId } from './gen/WaitId';

export function StreamList(props: { box: RefObject<HTMLDivElement | null>; room: number | null; stream: readonly StreamWait[]; current: WaitId | null; onPick: (entry: StreamWait) => void }) {
  const { t } = useTranslation();
  const rest = props.stream.filter((entry) => entry.wait.id !== props.current);
  if (rest.length === 0) return null;
  return (
    <div ref={props.box} className="alert-stream" style={props.room === null ? undefined : { maxHeight: props.room }} role="list" aria-label={t('composer.sectionAlerts')}>
      {rest.map((entry) => (
        <button key={entry.wait.id} type="button" role="listitem" tabIndex={-1} className="al-row" data-tone={entry.wait.tone} onClick={() => props.onPick(entry)}>
          <AlertLine {...entry} />
        </button>
      ))}
    </div>
  );
}
