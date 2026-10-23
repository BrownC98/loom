import type { SettingsView } from './gen/SettingsView';

export type SettingsPatch = { [K in keyof SettingsView]?: unknown };

type Json = unknown;
const isObject = (v: Json): v is Record<string, Json> => typeof v === 'object' && v !== null && !Array.isArray(v);

function diff(before: Json, after: Json): Json | undefined {
  if (isObject(before) && isObject(after)) {
    const out: Record<string, Json> = {};
    for (const key of new Set([...Object.keys(before), ...Object.keys(after)])) {
      const d = key in after ? diff(before[key], after[key]) : null;
      if (d !== undefined) out[key] = d;
    }
    return Object.keys(out).length ? out : undefined;
  }
  return JSON.stringify(before) === JSON.stringify(after) ? undefined : (after ?? null);
}

export function settingsPatch(before: SettingsView, after: SettingsView): SettingsPatch | null {
  return (diff(before, after) as SettingsPatch | undefined) ?? null;
}

function apply(target: Json, patch: Json): Json {
  if (!isObject(patch)) return patch;
  const out: Record<string, Json> = isObject(target) ? { ...target } : {};
  for (const [key, value] of Object.entries(patch)) {
    if (value === null) delete out[key];
    else out[key] = apply(out[key], value);
  }
  return out;
}

export function applySettingsPatch(view: SettingsView, patch: SettingsPatch): SettingsView {
  return apply(view, patch) as SettingsView;
}
