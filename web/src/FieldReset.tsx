import { createContext, useContext } from 'react';
import { RotateCcw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { SettingsView } from './gen/SettingsView';
import type { SettingsPatch } from './settingsPatch';

export type FieldDefaults = { settings: SettingsView; defaults: SettingsView | null; reset: (patch: SettingsPatch) => void };
export const FieldDefaults = createContext<FieldDefaults | null>(null);

const at = (value: unknown, path: readonly string[]): unknown => path.reduce<unknown>((v, key) => (v !== null && typeof v === 'object' ? (v as Record<string, unknown>)[key] : undefined), value);
const nulled = (path: readonly string[]): unknown => path.reduceRight<unknown>((inner, key) => ({ [key]: inner }), null);

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
