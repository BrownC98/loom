import { useEffect, useRef, useState } from 'react';
import { Pipette, RotateCcw, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { FieldReset } from './FieldReset';
import type { Theme } from './gen/Theme';
import type { ThemedColor } from './gen/ThemedColor';
import type { UiColors } from './gen/UiColors';
import { useDismiss } from './popover';
import { COLOR_ROWS, contrast, isHex, NO_COLOR, PRESETS, SCHEMES, toned, tonedColor, useColorMode, type ColorRow, type ColorScheme } from './uiColors';

type Props = { colors: UiColors; theme: Theme; onColors: (next: UiColors) => void };

type Target = ColorScheme | 'both';

function Halves({ color }: { color: string }) {
  return (
    <span className="halves">
      {SCHEMES.map((scheme) => (
        <i key={scheme} data-scheme={scheme} style={{ background: color }} />
      ))}
    </span>
  );
}

export function ColorsSection({ colors, theme, onColors }: Props) {
  const { t } = useTranslation();
  const mode = useColorMode(theme);
  const [open, setOpen] = useState<ColorRow['key'] | null>(null);
  const set = (row: ColorRow, next: ThemedColor) => onColors({ ...colors, [row.key]: next });
  return (
    <section>
      <h4>
        {t('colors.heading')}
        <FieldReset path={['colors']} />
      </h4>
      <div className="colors">
        {COLOR_ROWS.map((row) => {
          const color = colors[row.key];
          const changed = color.light !== null || color.dark !== null;
          const name = t(`colors.name.${row.key}`);
          return (
            <div key={row.key} className="crow">
              <button className="sw" aria-label={t('colors.pick', { name })} aria-expanded={open === row.key} onClick={() => setOpen(open === row.key ? null : row.key)}>
                <Halves color={`var(${row.token})`} />
              </button>
              <b>{name}</b>
              <span className="hex">
                {color.light === color.dark ? (
                  (color.light ?? t('colors.default'))
                ) : (
                  SCHEMES.map((scheme) => (
                    <span key={scheme}>
                      <small>{t(`theme.${scheme}`)}</small>
                      {color[scheme] ?? t('colors.default')}
                    </span>
                  ))
                )}
              </span>
              <span className="use">{t(`colors.use.${row.key}`)}</span>
              <button className="btn sm" disabled={!changed} aria-label={t('colors.resetRow', { name })} onClick={() => set(row, NO_COLOR)}>
                <RotateCcw />
                {t('colors.reset')}
              </button>
              {open === row.key && (
                <ColorPopover
                  row={row}
                  name={name}
                  color={color}
                  mode={mode}
                  onColor={(next) => set(row, next)}
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

function ColorPopover(props: { row: ColorRow; name: string; color: ThemedColor; mode: ColorScheme; onColor: (next: ThemedColor) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const { row, color, onColor } = props;
  const box = useDismiss<HTMLDivElement>(props.onClose);
  const native = useRef<HTMLInputElement>(null);
  const [target, setTarget] = useState<Target>(color.light === color.dark ? 'both' : props.mode);
  const hex = target === 'both' ? (color.light === color.dark ? color.light : null) : color[target];
  const pick = (next: string) => onColor(target === 'both' ? { light: next, dark: next } : { ...color, [target]: next });
  const [text, setText] = useState(hex ?? '');
  useEffect(() => setText(hex ?? ''), [hex]);
  useEffect(() => {
    const input = native.current!;
    const commit = () => pick(input.value);
    input.addEventListener('change', commit);
    return () => input.removeEventListener('change', commit);
  });
  const invalid = text !== '' && !isHex(text);
  const changed = target === 'both' ? color.light !== null || color.dark !== null : color[target] !== null;
  return (
    <div
      ref={box}
      className="pop color-pop"
      role="dialog"
      aria-label={props.name}
    >
      <h2>
        {props.name}
        <button className="btn ghost icon" aria-label={t('common.close')} onClick={props.onClose}>
          <X />
        </button>
      </h2>
      <div className="schemes" role="group" aria-label={t('colors.schemes')}>
        {SCHEMES.map((scheme) => {
          const side = color[scheme];
          return (
            <button
              key={scheme}
              data-scheme={scheme}
              data-target={target === scheme ? 'only' : target === 'both' ? undefined : 'other'}
              aria-pressed={target === 'both' || target === scheme}
              title={t(target === scheme ? 'colors.bothThemes' : 'colors.onlyTheme', { theme: t(`theme.${scheme}`) })}
              onClick={() => setTarget(target === scheme ? 'both' : scheme)}
            >
              <span className="sample" style={{ background: `var(${row.token})`, color: `var(${row.against.token})` }}>
                3
              </span>
              {t(`theme.${scheme}`)}
              {side && <span className="ratio">{contrast(side, row, scheme).toFixed(1)}:1</span>}
            </button>
          );
        })}
      </div>
      <span className="hint">{t('colors.presets')}</span>
      <div className="presets">
        {PRESETS.map((p) => (
          <button key={p} aria-label={p} aria-pressed={p === hex} onClick={() => pick(p)}>
            <Halves color={target === 'both' ? tonedColor(p, row) : toned(p, row, target)} />
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
            if (isHex(e.target.value)) pick(e.target.value.toLowerCase());
          }}
        />
        <button className="btn sm" onClick={() => native.current?.click()}>
          <Pipette />
          {t('colors.native')}
        </button>
        <input ref={native} type="color" className="native" tabIndex={-1} aria-hidden="true" defaultValue={hex ?? PRESETS[0]} onChange={(e) => setText(e.target.value)} />
      </div>
      <div className="pop-foot">
        <button
          className="btn sm"
          disabled={!changed}
          onClick={() => {
            onColor(target === 'both' ? NO_COLOR : { ...color, [target]: null });
            props.onClose();
          }}
        >
          <RotateCcw />
          {t('colors.reset')}
        </button>
        <span className="hint">{t('colors.saved')}</span>
      </div>
    </div>
  );
}
