import { useEffect, useRef, useState } from 'react';
import { Pipette, RotateCcw, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { UiColors } from './gen/UiColors';
import { useOutsideClose } from './popover';
import { COLOR_ROWS, contrast, isHex, PRESETS, tonedColor, type ColorRow, type ColorScheme } from './uiColors';

type Props = { colors: UiColors; onColors: (next: UiColors) => void };

const SCHEMES = ['light', 'dark'] as const satisfies readonly ColorScheme[];

/** 왼쪽 반은 라이트, 오른쪽 반은 다크에서 쓰는 색. `color`는 `light-dark()` 값이나 토큰이다. 반마다 `data-scheme`이 토큰을 그 테마로 다시 정한다(styles.css). */
function Halves({ color }: { color: string }) {
  return (
    <span className="halves">
      {SCHEMES.map((scheme) => (
        <i key={scheme} data-scheme={scheme} style={{ background: color }} />
      ))}
    </span>
  );
}

/** 설정 「UI」의 「색」(목업 docs/mockups/color-settings.html v3 ①): 뜻마다 한 줄, 색 칸을 누르면 색 고르기 창이 뜬다. */
export function ColorsSection({ colors, onColors }: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState<ColorRow['key'] | null>(null);
  const set = (row: ColorRow, hex: string | null) => onColors({ ...colors, [row.key]: hex });
  return (
    <section>
      <h4>{t('colors.heading')}</h4>
      <div className="colors">
        {COLOR_ROWS.map((row) => {
          const hex = colors[row.key];
          const name = t(`colors.name.${row.key}`);
          return (
            <div key={row.key} className="crow">
              <button className="sw" aria-label={t('colors.pick', { name })} aria-expanded={open === row.key} onClick={() => setOpen(open === row.key ? null : row.key)}>
                <Halves color={`var(${row.token})`} />
              </button>
              <b>{name}</b>
              <span className="hex">{hex ?? t('colors.default')}</span>
              <span className="use">{t(`colors.use.${row.key}`)}</span>
              <button className="btn sm" disabled={!hex} aria-label={t('colors.resetRow', { name })} onClick={() => set(row, null)}>
                <RotateCcw />
                {t('colors.reset')}
              </button>
              {open === row.key && (
                <ColorPopover
                  row={row}
                  name={name}
                  hex={hex}
                  onPick={(next) => set(row, next)}
                  onReset={() => {
                    set(row, null);
                    setOpen(null);
                  }}
                  onClose={() => setOpen(null)}
                />
              )}
            </div>
          );
        })}
      </div>
      <div className="color-preview" aria-label={t('colors.preview')}>
        <span className="hint">{t('colors.preview')}</span>
        <span className="pill">
          <b>{t('colors.previewPill')}</b>
          <span className="count">3</span>
        </span>
        <span className="badge warn">{t('colors.previewBadge')}</span>
        <span className="dot waiting" />
        <span className="dot running" />
        <span className="dot done" />
        <span className="dot exited" />
        <span className="mini" aria-hidden="true">
          <span className="g">
            <span className="c" />
            <span className="e" />
            <span className="c" />
          </span>
        </span>
      </div>
      <span className="hint">{t('colors.hint')}</span>
    </section>
  );
}

/** 색 고르기 창(목업 ②): 추천 색, 색 값, 브라우저 색 창, 테마마다 쓰는 색과 대비, 기본값. 고르면 바로 저장한다. */
function ColorPopover(props: { row: ColorRow; name: string; hex: string | null; onPick: (hex: string) => void; onReset: () => void; onClose: () => void }) {
  const { t } = useTranslation();
  const { row, hex, onPick } = props;
  const box = useOutsideClose<HTMLDivElement>(props.onClose);
  const native = useRef<HTMLInputElement>(null);
  const [text, setText] = useState(hex ?? '');
  useEffect(() => setText(hex ?? ''), [hex]);
  // 브라우저 색 창은 끄는 동안 input을 잇달아 보낸다. 저장은 고르기를 마칠 때(change) 한 번만 한다.
  useEffect(() => {
    const input = native.current!;
    const commit = () => onPick(input.value);
    input.addEventListener('change', commit);
    return () => input.removeEventListener('change', commit);
  }, [onPick]);
  const invalid = text !== '' && !isHex(text);
  return (
    <div
      ref={box}
      className="pop color-pop"
      role="dialog"
      aria-label={props.name}
      onKeyDown={(e) => {
        // 설정 창(<dialog>)까지 닫히지 않게 Esc를 여기서 끝낸다.
        if (e.key !== 'Escape') return;
        e.preventDefault();
        e.stopPropagation();
        props.onClose();
      }}
    >
      <h2>
        {props.name}
        <button className="btn ghost icon" aria-label={t('common.close')} onClick={props.onClose}>
          <X />
        </button>
      </h2>
      <span className="hint">{t('colors.presets')}</span>
      <div className="presets">
        {PRESETS.map((p) => (
          <button key={p} aria-label={p} aria-pressed={p === hex} onClick={() => onPick(p)}>
            <Halves color={tonedColor(p, row)} />
          </button>
        ))}
      </div>
      <span className="hint">{t('colors.value')}</span>
      <div className="hexrow">
        <input
          className="mono"
          aria-label={t('colors.valueLabel')}
          aria-invalid={invalid}
          placeholder="#rrggbb"
          value={text}
          spellCheck={false}
          onChange={(e) => {
            setText(e.target.value);
            if (isHex(e.target.value)) onPick(e.target.value.toLowerCase());
          }}
        />
        <button className="btn sm" onClick={() => native.current?.click()}>
          <Pipette />
          {t('colors.native')}
        </button>
        <input ref={native} type="color" className="native" tabIndex={-1} aria-hidden="true" defaultValue={hex ?? PRESETS[0]} onChange={(e) => setText(e.target.value)} />
      </div>
      <span className="hint">{t('colors.schemes')}</span>
      <div className="schemes">
        {SCHEMES.map((scheme) => (
          <div key={scheme} data-scheme={scheme}>
            <span className="sample" style={{ background: `var(${row.token})`, color: `var(${row.against.token})` }}>
              3
            </span>
            {t(`theme.${scheme}`)}
            {hex && <span className="ratio">{contrast(hex, row, scheme).toFixed(1)}:1</span>}
          </div>
        ))}
      </div>
      <div className="pop-foot">
        <button className="btn sm" disabled={!hex} onClick={props.onReset}>
          <RotateCcw />
          {t('colors.reset')}
        </button>
        <span className="hint">{t('colors.saved')}</span>
      </div>
    </div>
  );
}
