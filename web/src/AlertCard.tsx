import { useTranslation } from 'react-i18next';
import { PROMPT, type Target } from './alertTarget';
import { Choices, Instruction, ToTerminal, WaitContent, WaitHead } from './Inbox';

function Position(props: { target: Target }) {
  const { t } = useTranslation();
  const { target } = props;
  return <span className="alert-pos">{t('composer.alertCount', { index: target.index, count: target.waits.length, total: target.total })}</span>;
}

export function AlertCard(props: { target: Target; onChoose: (key: string) => void; onGo: () => void }) {
  const { t } = useTranslation();
  const { target } = props;
  const wait = target.card;
  if (!wait) return null;
  return (
    <div className="entry alert-card" data-tone={wait.tone}>
      <WaitHead wait={wait}>
        {target.key === PROMPT && <span className="kind">{t('composer.notAnswer')}</span>}
        <Position target={target} />
      </WaitHead>
      {wait.instruction && <Instruction text={wait.instruction} />}
      <div className="wait-body" data-tone={wait.tone}>
        <WaitContent node={target.view.id} wait={wait} />
        {wait.answerable ? wait.tone !== 'done' && <Choices list={wait.choices ?? []} onPick={props.onChoose} /> : <ToTerminal onGo={props.onGo} />}
      </div>
    </div>
  );
}
