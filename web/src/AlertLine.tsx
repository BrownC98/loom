import { useTranslation } from 'react-i18next';
import type { StreamWait } from './alertTarget';
import type { ScreenWait } from './gen/ScreenWait';
import { clockTime } from './i18n';
import { ICON, showsId } from './notice';

export function KindChip(props: { wait: ScreenWait }) {
  const { t } = useTranslation();
  const { wait } = props;
  return showsId(wait.kind) ? <span className="qid">Q{wait.id}</span> : <span className="al-kind" data-tone={wait.tone}>{t(`wait.${wait.kind}`)}</span>;
}

export function AlertLine(props: StreamWait) {
  const { view, wait } = props;
  const Icon = ICON[wait.tone];
  return (
    <>
      <Icon className="kind-mark" aria-hidden="true" />
      <KindChip wait={wait} />
      <b className="al-node">{view.name}</b>
      <span className="al-text">{wait.summary}</span>
      <time className="al-time" dateTime={new Date(wait.since * 1000).toISOString()}>
        {clockTime(wait.since * 1000)}
      </time>
    </>
  );
}
