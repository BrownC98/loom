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

/** 색 고르기 창이 바꾸는 테마. `'both'`는 두 테마에 같은 색을 쓴다(목업 docs/mockups/color-themes.html v3). */
type Target = ColorScheme | 'both';

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

/** 설정 「UI」의 「색」(목업 docs/mockups/color-settings.html v3 ①, color-themes.html v3 ①): 뜻마다 한 줄, 색 칸을 누르면 색 고르기 창이 뜬다. */
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

/** 색 고르기 창(목업 color-settings.html ②, color-themes.html v3 ②): 맨 위 테마 칸, 추천 색, 색 값, 브라우저 색 창, 기본값. 고르면 바로 저장한다.
 *  테마 칸을 누르면 그 테마만 바꾼다. 같은 칸을 다시 누르면 두 테마를 함께 바꾼다. */
function ColorPopover(props: { row: ColorRow; name: string; color: ThemedColor; mode: ColorScheme; onColor: (next: ThemedColor) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const { row, color, onColor } = props;
  const box = useDismiss<HTMLDivElement>(props.onClose);
  const native = useRef<HTMLInputElement>(null);
  const [target, setTarget] = useState<Target>(color.light === color.dark ? 'both' : props.mode);
  // 두 테마 색이 다를 때 「두 테마」에는 보일 한 값이 없다.
  const hex = target === 'both' ? (color.light === color.dark ? color.light : null) : color[target];
  const pick = (next: string) => onColor(target === 'both' ? { light: next, dark: next } : { ...color, [target]: next });
  const [text, setText] = useState(hex ?? '');
  useEffect(() => setText(hex ?? ''), [hex]);
  // 브라우저 색 창은 끄는 동안 input을 잇달아 보낸다. 저장은 고르기를 마칠 때(change) 한 번만 한다.
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
