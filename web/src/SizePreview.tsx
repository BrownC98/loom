import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { DeskFace } from './DeskComposer';
import { FLOAT_SIZES, floatZoom } from './floatPlace';
import type { FloatSize } from './gen/FloatSize';

export type SizePreview = (props: { size: FloatSize }) => ReactNode;

const LARGEST = FLOAT_SIZES[FLOAT_SIZES.length - 1];

const IDLE = () => {};

export const ComposerPreview: SizePreview = ({ size }) => {
  const { t } = useTranslation();
  return (
    <div className="size-prev" inert>
      <span className="cap">{t('floatSize.preview', { size: t(`floatSize.${size}`) })}</span>
      <div className="room">
        <Sample size={LARGEST} />
      </div>
      <Sample size={size} />
    </div>
  );
};

function Sample(props: { size: FloatSize }) {
  const { t } = useTranslation();
  return (
    <div style={{ zoom: floatZoom(props.size) }}>
      <DeskFace route="prompt" name={t('floatSize.sampleNode')} dragging={false} drop={{}} field={{ value: '', readOnly: true }} sendable onFiles={IDLE} onSubmit={IDLE} target={{ open: false, onToggle: IDLE }} />
    </div>
  );
}
