import { describe, expect, it } from 'vitest';
import css from './styles.css?raw';
import { COLOR_ROWS, contrast, PRESETS, tonedColor } from './uiColors';

/** styles.css :root의 `token: light-dark(oklch(L% …), oklch(L% …))`에서 두 밝기. */
function tone(token: string) {
  const found = css.match(new RegExp(`\\n\\s*${token}: light-dark\\(oklch\\(([\\d.]+)% [^)]*\\), oklch\\(([\\d.]+)% `));
  if (!found) throw new Error(`${token} is missing from styles.css`);
  return { light: Number(found[1]), dark: Number(found[2]) };
}

describe('COLOR_ROWS', () => {
  it('lightness table matches the styles.css defaults', () => {
    for (const row of COLOR_ROWS) {
      expect(tone(row.token), row.token).toEqual(row.tone);
      expect(tone(row.against.token), row.against.token).toEqual(row.against.tone);
    }
  });

  it('every preset keeps text contrast at 4.5:1 or more', () => {
    for (const row of COLOR_ROWS.filter((r) => r.against.min))
      for (const hex of PRESETS)
        for (const scheme of ['light', 'dark'] as const) expect(contrast(hex, row, scheme), `${row.key} ${hex} ${scheme}`).toBeGreaterThanOrEqual(4.5);
  });

  it('keeps the default lightness when contrast is enough, moves only lightness when not', () => {
    const warn = COLOR_ROWS.find((r) => r.key === 'warn')!;
    expect(tonedColor('#8b5cf6', warn)).toBe('light-dark(oklch(from #8b5cf6 54% c h), oklch(from #8b5cf6 80% c h))');
    // 초록은 54%에서 흰 글자 대비가 4.48:1이라 조금 어둡게 쓴다.
    expect(tonedColor('#22c55e', warn)).toMatch(/^light-dark\(oklch\(from #22c55e 5[0-3](\.5)?% c h\), oklch\(from #22c55e 80% c h\)\)$/);
  });

  it('edges and borders keep the default lightness', () => {
    const edge = COLOR_ROWS.find((r) => r.key === 'edge')!;
    expect(tonedColor('#14b8a6', edge)).toBe('light-dark(oklch(from #14b8a6 80% c h), oklch(from #14b8a6 43.9% c h))');
  });
});
