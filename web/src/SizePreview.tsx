import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { ComposerFace } from './Composer';
import { FLOAT_SIZES, floatZoom } from './floatPlace';
import type { FloatSize } from './gen/FloatSize';
import type { ScreenWait } from './gen/ScreenWait';
import { PillFace } from './Inbox';

export type SizePreview = (props: { size: FloatSize }) => ReactNode;

const LARGEST = FLOAT_SIZES[FLOAT_SIZES.length - 1];

const IDLE = () => {};

export const PillPreview: SizePreview = ({ size }) => <PreviewBox size={size} Sample={PillSample} />;
export const ComposerPreview: SizePreview = ({ size }) => <PreviewBox size={size} Sample={ComposerSample} />;

function PreviewBox(props: { size: FloatSize; Sample: SizePreview }) {
  const { t } = useTranslation();
  const { Sample } = props;
  return (
    <div className="size-prev" inert>
      <span className="cap">{t('floatSize.preview', { size: t(`floatSize.${props.size}`) })}</span>
      <div className="room">
        <Sample size={LARGEST} />
      </div>
      <Sample size={props.size} />
    </div>
  );
}

function PillSample(props: { size: FloatSize }) {
  const { t } = useTranslation();
  const wait: ScreenWait = { tone: 'ask', summary: t('floatSize.sampleText'), id: 1, kind: 'question', text: '', since: 0, answerable: false, doc: null, images: [] };
  return <PillFace size={props.size} view="line" name={t('floatSize.sampleNode')} wait={wait} waits={[]} open={false} dragging={false} onClickCapture={IDLE} onToggle={IDLE} onDragStart={IDLE} onReset={null} onClose={IDLE} />;
}

function ComposerSample(props: { size: FloatSize }) {
  const { t } = useTranslation();
  return (
    <div style={{ zoom: floatZoom(props.size) }}>
      <ComposerFace name={t('floatSize.sampleNode')} dragging={false} drop={{}} field={{ value: '', readOnly: true }} sendable onFiles={IDLE} onSubmit={IDLE} />
    </div>
  );
}
