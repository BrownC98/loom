import { AlarmClock } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { wakeNode } from './api';
import type { NodeView } from './gen/NodeView';
import { asleep } from './notice';

export function WakeButton(props: { view: Pick<NodeView, 'id' | 'process'> }) {
  const { t } = useTranslation();
  const [refusal, setRefusal] = useState<string | null>(null);
  if (!asleep(props.view)) return null;
  return (
    <button
      type="button"
      className="btn ghost sm icon nodrag wake"
      aria-label={t('node.wake')}
      title={refusal ?? t('node.wake')}
      data-refused={refusal !== null || undefined}
      onClick={() => {
        setRefusal(null);
        wakeNode(props.view.id).catch((err: Error) => setRefusal(err.message));
      }}
    >
      <AlarmClock />
    </button>
  );
}
