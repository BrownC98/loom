// 설정 「색」(목업 docs/mockups/color-settings.html v3): 사람이 뜻마다 고른 한 색으로 그 CSS 토큰을 바꾼다.
// 고른 색의 색상·채도를 쓰고, 밝기는 테마마다 그 토큰의 기본 밝기를 쓴다. 그래서 바탕·글자와의 대비가 기본 색과 거의 같다.
// 글자 대비가 모자라는 색상(예: 초록)은 밝기를 대비가 맞을 때까지 상대 무채색에서 멀리 옮긴다.
import type { UiColors } from './gen/UiColors';

type ColorKey = keyof UiColors;
export type ColorScheme = 'light' | 'dark';
/** OKLCH 밝기(%)를 테마마다. */
type Tone = Record<ColorScheme, number>;
/** 무채색 토큰과 그 밝기. 대비를 잴 상대다. `min`이 있으면 대비를 그 값 이상으로 맞춘다. */
type Neutral = { token: string; tone: Tone; min?: number };
/** 글자 대비 하한(WCAG 1.4.3). */
const TEXT_CONTRAST = 4.5;

export type ColorRow = {
  key: ColorKey;
  token: string;
  /** `token` 기본값의 밝기. styles.css와 같아야 한다(uiColors.test.ts). */
  tone: Tone;
  /** 이 색 위의 글자나 이 색 둘레의 바탕. 미리 보기의 대비는 이 색과 잰다. */
  against: Neutral;
};

const WARN_INK: Neutral = { token: '--warn-ink', tone: { light: 100, dark: 20.5 }, min: TEXT_CONTRAST };
const SURFACE: Neutral = { token: '--surface', tone: { light: 100, dark: 26.9 }, min: TEXT_CONTRAST };
const BG: Neutral = { token: '--bg', tone: { light: 97, dark: 20.5 } };

/** 설정 화면의 줄 순서다. */
export const COLOR_ROWS: readonly ColorRow[] = [
  { key: 'warn', token: '--warn', tone: { light: 54, dark: 80 }, against: WARN_INK },
  { key: 'run', token: '--run', tone: { light: 54.6, dark: 70.7 }, against: SURFACE },
  { key: 'ok', token: '--ok', tone: { light: 50, dark: 76 }, against: SURFACE },
  { key: 'err', token: '--err', tone: { light: 55, dark: 70.4 }, against: { token: '--err-ink', tone: { light: 98.5, dark: 20.5 }, min: TEXT_CONTRAST } },
  { key: 'edge', token: '--edge', tone: { light: 80, dark: 43.9 }, against: BG },
  { key: 'repo', token: '--repo-line', tone: { light: 57, dark: 58 }, against: BG },
  { key: 'worktree', token: '--group-line', tone: { light: 57, dark: 58 }, against: BG },
  { key: 'accent', token: '--accent', tone: { light: 20.5, dark: 92.2 }, against: { token: '--accent-ink', tone: { light: 98.5, dark: 20.5 }, min: TEXT_CONTRAST } },
];

/** 강조 색을 바꾸면 고른 줄·선택 바탕(--accent-wash)도 같은 색상으로 바꾼다. 투명도는 styles.css 기본값과 같다. */
const ACCENT_WASH = { token: '--accent-wash', alpha: { light: 0.07, dark: 0.12 } } as const;

/** 색 고르기 창의 추천 색. 빨강·주황·노랑·초록·청록·파랑·보라·분홍. */
export const PRESETS = ['#ef4444', '#f97316', '#eab308', '#22c55e', '#14b8a6', '#3b82f6', '#8b5cf6', '#ec4899'] as const;

const HEX = /^#[0-9a-f]{6}$/i;
export const isHex = (text: string) => HEX.test(text);

const toned = (hex: string, row: ColorRow, scheme: ColorScheme, alpha = '') => `oklch(from ${hex} ${lightness(hex, row, scheme)}% c h${alpha})`;
/** `hex`의 색상·채도에 `row`의 테마별 밝기를 얹은 CSS 값. 그 값을 쓰는(사용자 정의 속성이면 선언한) 요소의 `color-scheme`이 라이트·다크를 고른다. */
export const tonedColor = (hex: string, row: ColorRow) => `light-dark(${toned(hex, row, 'light')}, ${toned(hex, row, 'dark')})`;

/** 고른 색을 얹는 <style>. styles.css 토큰과 같은 선택자라 뒤에 오는 이 규칙이 이긴다. */
const SHEET_ID = 'ui-colors';

/** 고른 색을 토큰으로 얹는다. 고르지 않은 색은 styles.css 기본값 그대로다. */
export function applyColors(colors: UiColors) {
  const vars = COLOR_ROWS.flatMap((row) => {
    const hex = colors[row.key];
    return hex ? [`${row.token}: ${tonedColor(hex, row)};`] : [];
  });
  const accent = colors.accent;
  if (accent) {
    const row = COLOR_ROWS.find((r) => r.key === 'accent')!;
    const wash = (scheme: ColorScheme) => toned(accent, row, scheme, ` / ${ACCENT_WASH.alpha[scheme]}`);
    vars.push(`${ACCENT_WASH.token}: light-dark(${wash('light')}, ${wash('dark')});`);
  }
  let sheet = document.getElementById(SHEET_ID);
  if (!sheet) {
    sheet = document.createElement('style');
    sheet.id = SHEET_ID;
    document.head.append(sheet);
  }
  sheet.textContent = vars.length ? `:root, [data-scheme] { ${vars.join(' ')} }` : '';
}

// 대비 계산: sRGB ↔ OKLab(Björn Ottosson, https://bottosson.github.io/posts/oklab/). 화면 밖 색은 채널마다 sRGB 안으로 자른다.
// 브라우저는 채도를 줄여 맞추므로 값이 조금 다를 수 있다. 미리 보기의 대비는 어림값이다.
const linear = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);

/** `#rrggbb`의 OKLCH 채도와 색상(도). */
function chromaHue(hex: string): { c: number; h: number } {
  const [r, g, b] = [1, 3, 5].map((i) => linear(parseInt(hex.slice(i, i + 2), 16) / 255));
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const bb = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return { c: Math.hypot(a, bb), h: Math.atan2(bb, a) };
}

/** OKLCH(밝기 0~1, 채도, 색상 라디안)의 상대 휘도(WCAG). */
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
/** 밝기를 옮기는 한 걸음(%)과 걸음 수 상한. 200걸음이면 어느 기본 밝기에서도 0% 또는 100%에 닿는다. */
const STEP = 0.5;
const MAX_STEPS = 200;

/** 테마 `scheme`에서 `hex`로 만든 `row` 색의 밝기(%). 기본 밝기에서 시작해 `against.min` 대비가 될 때까지 상대에서 멀어진다. */
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

/** 테마 `scheme`에서 `hex`로 만든 `row`의 색과 그 상대 무채색의 대비(WCAG, 1~21). */
export function contrast(hex: string, row: ColorRow, scheme: ColorScheme): number {
  const { c, h } = chromaHue(hex);
  return ratio(luminance(lightness(hex, row, scheme) / 100, c, h), luminance(row.against.tone[scheme] / 100, 0, 0));
}
