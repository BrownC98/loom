import type { SettingsView } from './gen/SettingsView';

/**
 * 설정에서 바꾼 칸만 담은 JSON Merge Patch(RFC 7386). 데몬이 지금 설정에 합친다(`src/settings.rs` `Settings::patch`).
 * 객체는 칸마다, 다른 값(배열 포함)은 통째로 바꾼다. `null`은 칸을 지워 기본값으로 돌린다.
 */
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

/** `before`에서 `after`로 바꾼 칸. 바뀐 칸이 없으면 `null`이다. */
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

/** 데몬과 같은 규칙으로 합친다. 저장이 끝나기 전에 다시 읽은 설정 위에 아직 보내는 중인 칸을 다시 얹는 데 쓴다. */
export function applySettingsPatch(view: SettingsView, patch: SettingsPatch): SettingsView {
  return apply(view, patch) as SettingsView;
}
