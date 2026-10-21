import { createContext, useContext } from 'react';
import { RotateCcw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { SettingsView } from './gen/SettingsView';
import type { SettingsPatch } from './settingsPatch';

/** 설정 창이 칸마다 [기본값] 단추에 주는 값. `defaults`는 데몬의 기본 설정이다(`GET /api/settings/default`). 받기 전에는 단추를 보이지 않는다. */
export type FieldDefaults = { settings: SettingsView; defaults: SettingsView | null; reset: (patch: SettingsPatch) => void };
export const FieldDefaults = createContext<FieldDefaults | null>(null);

const at = (value: unknown, path: readonly string[]): unknown => path.reduce<unknown>((v, key) => (v !== null && typeof v === 'object' ? (v as Record<string, unknown>)[key] : undefined), value);
/** `['terminalTheme', 'dark']` → `{ terminalTheme: { dark: null } }`. 데몬은 `null` 칸을 지우고 기본값으로 읽는다. */
const nulled = (path: readonly string[]): unknown => path.reduceRight<unknown>((inner, key) => ({ [key]: inner }), null);

/**
 * 설정 칸 하나의 [기본값] 단추. `path`의 지금 값이 기본값과 다를 때만 보인다(2026-10-22 사람 지시 "모든 설정에 초기화 버튼을 추가해").
 * `path`는 `SettingsView` 안의 칸 경로다. 기본값은 Rust에만 둔다.
 */
export function FieldReset({ path }: { path: readonly [keyof SettingsView, ...string[]] }) {
  const { t } = useTranslation();
  const ctx = useContext(FieldDefaults);
  if (!ctx?.defaults || JSON.stringify(at(ctx.settings, path)) === JSON.stringify(at(ctx.defaults, path))) return null;
  return (
    <button type="button" className="btn ghost sm field-reset" title={t('settings.fieldResetHint')} onClick={() => ctx.reset(nulled(path) as SettingsPatch)}>
      <RotateCcw />
      {t('settings.fieldReset')}
    </button>
  );
}
