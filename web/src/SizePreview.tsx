import { ArrowUp, Image as ImageIcon, Paperclip, SquareTerminal, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { FLOAT_SIZES, floatZoom } from './floatPlace';
import type { FloatSize } from './gen/FloatSize';
import { ICON } from './notice';

export type SizeField = 'pillSize' | 'composerSize';

const LARGEST = FLOAT_SIZES[FLOAT_SIZES.length - 1];

export function SizePreview(props: { field: SizeField; size: FloatSize }) {
  const { t } = useTranslation();
  const Sample = props.field === 'pillSize' ? PillSample : ComposerSample;
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
  const Icon = ICON.ask;
  return (
    <div className="inbox-pill" data-view="line" style={{ zoom: floatZoom(props.size) }}>
      <div className="head">
        <span className="pill-close">
          <X />
        </span>
        <Icon className="qicon" />
        <b>{t('wait.question')}</b>
        <span className="qid">Q1</span>
        <span className="go">
          <b>{t('floatSize.sampleNode')}</b>: {t('floatSize.sampleText')}
        </span>
      </div>
    </div>
  );
}

function ComposerSample(props: { size: FloatSize }) {
  const { t } = useTranslation();
  return (
    <div className="composer" style={{ zoom: floatZoom(props.size) }}>
      <div className="composer-to">
        <span className="target">
          <SquareTerminal />
          <b>{t('floatSize.sampleNode')}</b>
          {t('composer.to')}
        </span>
        <span className="keys">{t('composer.keys')}</span>
      </div>
      <div className="composer-line">
        <textarea rows={1} readOnly placeholder={t('composer.placeholder')} />
        <span className="attach">
          <ImageIcon />
        </span>
        <span className="attach">
          <Paperclip />
        </span>
        <span className="send">
          <ArrowUp />
        </span>
      </div>
    </div>
  );
}
