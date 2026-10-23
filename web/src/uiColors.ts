import { useSyncExternalStore } from 'react';
import type { Theme } from './gen/Theme';
import type { ThemedColor } from './gen/ThemedColor';
import type { UiColors } from './gen/UiColors';

type ColorKey = keyof UiColors;
export type ColorScheme = 'light' | 'dark';
export const SCHEMES = ['light', 'dark'] as const satisfies readonly ColorScheme[];
const DARK_QUERY = '(prefers-color-scheme: dark)';

export function useColorMode(theme: Theme): ColorScheme {
  const systemDark = useSyncExternalStore(
    (onChange) => {
      const query = matchMedia(DARK_QUERY);
      query.addEventListener('change', onChange);
      return () => query.removeEventListener('change', onChange);
    },
    () => matchMedia(DARK_QUERY).matches,
  );
  return theme === 'system' ? (systemDark ? 'dark' : 'light') : theme;
}

type Tone = Record<ColorScheme, number>;
type Neutral = { token: string; tone: Tone; min?: number };
const TEXT_CONTRAST = 4.5;

export type ColorRow = {
  key: ColorKey;
  token: string;
  tone: Tone;
  against: Neutral;
};

const WARN_INK: Neutral = { token: '--warn-ink', tone: { light: 100, dark: 20.5 }, min: TEXT_CONTRAST };
const SURFACE: Neutral = { token: '--surface', tone: { light: 100, dark: 26.9 }, min: TEXT_CONTRAST };
const BG: Neutral = { token: '--bg', tone: { light: 97, dark: 20.5 } };

export const COLOR_ROWS: readonly ColorRow[] = [
  { key: 'warn', token: '--warn', tone: { light: 54, dark: 80 }, against: WARN_INK },
  { key: 'run', token: '--run', tone: { light: 54.6, dark: 70.7 }, against: SURFACE },
  { key: 'ok', token: '--ok', tone: { light: 50, dark: 76 }, against: SURFACE },
  { key: 'err', token: '--err', tone: { light: 55, dark: 70.4 }, against: { token: '--err-ink', tone: { light: 98.5, dark: 20.5 }, min: TEXT_CONTRAST } },
  { key: 'edge', token: '--edge', tone: { light: 80, dark: 43.9 }, against: BG },
  { key: 'repo', token: '--repo-line', tone: { light: 57, dark: 58 }, against: BG },
  { key: 'worktree', token: '--group-line', tone: { light: 57, dark: 58 }, against: BG },
  { key: 'notice', token: '--notice', tone: { light: 56, dark: 70 }, against: { token: '--notice-ink', tone: { light: 100, dark: 20.5 }, min: TEXT_CONTRAST } },
  { key: 'accent', token: '--accent', tone: { light: 20.5, dark: 92.2 }, against: { token: '--accent-ink', tone: { light: 98.5, dark: 20.5 }, min: TEXT_CONTRAST } },
];

export const PRESETS = ['#ef4444', '#f97316', '#eab308', '#22c55e', '#14b8a6', '#3b82f6', '#8b5cf6', '#ec4899'] as const;

export const NO_COLOR: Readonly<ThemedColor> = Object.freeze({ light: null, dark: null });

const HEX = /^#[0-9a-f]{6}$/i;
export const isHex = (text: string) => HEX.test(text);

export const toned = (hex: string, row: ColorRow, scheme: ColorScheme) => `oklch(from ${hex} ${lightness(hex, row, scheme)}% c h)`;
export const tonedColor = (hex: string, row: ColorRow) => `light-dark(${toned(hex, row, 'light')}, ${toned(hex, row, 'dark')})`;

const SHEET_ID = 'ui-colors';

export function applyColors(colors: UiColors) {
  const vars = COLOR_ROWS.flatMap((row) =>
    SCHEMES.flatMap((scheme) => {
      const hex = colors[row.key][scheme];
      return hex ? [`${row.token}-${scheme}: ${toned(hex, row, scheme)};`] : [];
    }),
  );
  let sheet = document.getElementById(SHEET_ID);
  if (!sheet) {
    sheet = document.createElement('style');
    sheet.id = SHEET_ID;
    document.head.append(sheet);
  }
  sheet.textContent = vars.length ? `:root, [data-scheme] { ${vars.join(' ')} }` : '';
}

const linear = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);

function chromaHue(hex: string): { c: number; h: number } {
  const [r, g, b] = [1, 3, 5].map((i) => linear(parseInt(hex.slice(i, i + 2), 16) / 255));
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const bb = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return { c: Math.hypot(a, bb), h: Math.atan2(bb, a) };
}

function luminance(lightness: number, c: number, h: number): number {
  const a = c * Math.cos(h);
  const b = c * Math.sin(h);
  const l = (lightness + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (lightness - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (lightness - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const clip = (v: number) => Math.min(1, Math.max(0, v));
  const r = clip(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s);
  const g = clip(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s);
  const bl = clip(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s);
  return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
}

const ratio = (a: number, b: number) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
const STEP = 0.5;
const MAX_STEPS = 200;

function lightness(hex: string, row: ColorRow, scheme: ColorScheme): number {
  const { c, h } = chromaHue(hex);
  const other = row.against.tone[scheme];
  const away = row.tone[scheme] >= other ? STEP : -STEP;
  let at = row.tone[scheme];
  for (let i = 0; row.against.min && i < MAX_STEPS && at > 0 && at < 100; i++) {
    if (ratio(luminance(at / 100, c, h), luminance(other / 100, 0, 0)) >= row.against.min) break;
    at += away;
  }
  return at;
}

export function contrast(hex: string, row: ColorRow, scheme: ColorScheme): number {
  const { c, h } = chromaHue(hex);
  return ratio(luminance(lightness(hex, row, scheme) / 100, c, h), luminance(row.against.tone[scheme] / 100, 0, 0));
}
