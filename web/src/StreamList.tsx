import { useLayoutEffect, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { AlertLine } from './AlertLine';
import type { StreamWait } from './alertTarget';
import type { WaitId } from './gen/WaitId';

export function StreamList(props: { box: RefObject<HTMLDivElement | null>; room: number | null; stream: readonly StreamWait[]; current: WaitId | null; onPick: (entry: StreamWait) => void }) {
  const { t } = useTranslation();
  useLayoutEffect(() => {
    props.box.current?.querySelector('[aria-current]')?.scrollIntoView({ block: 'nearest' });
  }, [props.current, props.stream.length]);
  if (props.stream.length === 0) return null;
  return (
    <div ref={props.box} className="alert-stream" style={props.room === null ? undefined : { maxHeight: props.room }} role="list" aria-label={t('composer.sectionAlerts')}>
      {props.stream.map((entry) => (
        <button key={entry.wait.id} type="button" role="listitem" tabIndex={-1} className="al-row" data-tone={entry.wait.tone} aria-current={entry.wait.id === props.current || undefined} onClick={() => props.onPick(entry)}>
          <AlertLine {...entry} />
        </button>
      ))}
    </div>
  );
}
