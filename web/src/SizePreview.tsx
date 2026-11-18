import type { CSSProperties, ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { DeskFace } from './DeskComposer';
import { FLOAT_SIZES, floatZoom } from './floatPlace';
import type { FloatSize } from './gen/FloatSize';

export type SizePreview = (props: { size: FloatSize }) => ReactNode;

const LARGEST = FLOAT_SIZES[FLOAT_SIZES.length - 1];

const IDLE = () => {};

const framed =
  (Sample: (props: { size: FloatSize }) => ReactNode): SizePreview =>
  ({ size }) => {
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

function ComposerSample(props: { size: FloatSize }) {
  const { t } = useTranslation();
  return (
    <div style={{ zoom: floatZoom(props.size) }}>
      <DeskFace route="prompt" name={t('floatSize.sampleNode')} dragging={false} drop={{}} field={{ value: '', readOnly: true }} sendable onFiles={IDLE} onSubmit={IDLE} target={{ open: false, onToggle: IDLE }} />
    </div>
  );
}

function UiSample(props: { size: FloatSize }) {
  const { t } = useTranslation();
  return (
    <div className="ui-sample" style={{ '--step': floatZoom(props.size) } as CSSProperties}>
      <ul className="tree">
        {(['sampleNode', 'sampleWorker'] as const).map((name) => (
          <li key={name}>
            <div className="tree-row">
              <span className="dot running" />
              <span className="name">{t(`floatSize.${name}`)}</span>
            </div>
          </li>
        ))}
      </ul>
      <div className="entry ui-sample-card">
        <div className="top">
          <span className="qid">Q1</span>
          <b>{t('floatSize.sampleNode')}</b>
          <span className="published">12:00</span>
        </div>
        <p className="instruction">{t('floatSize.sampleAsk')}</p>
        <div className="ui-sample-acts">
          <span className="btn sm">A {t('floatSize.sampleYes')}</span>
          <span className="btn sm">B {t('floatSize.sampleNo')}</span>
        </div>
      </div>
    </div>
  );
}

export const ComposerPreview = framed(ComposerSample);
export const UiPreview = framed(UiSample);
