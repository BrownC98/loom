import { describe, expect, it } from 'vitest';
import type { SettingsView } from './gen/SettingsView';
import { applySettingsPatch, settingsPatch } from './settingsPatch';

const base = {
  minimapFolded: false,
  arrangeDirection: 'down',
  paneWidths: { side: null, dock: 320 },
  bypassOff: ['claude'],
} as unknown as SettingsView;

describe('settingsPatch', () => {
  it('sends only changed fields, and inner objects field by field', () => {
    const next = { ...base, minimapFolded: true, paneWidths: { side: 300, dock: 320 } } as SettingsView;
    expect(settingsPatch(base, next)).toEqual({ minimapFolded: true, paneWidths: { side: 300 } });
  });

  it('sends a cleared value as null and a changed array whole', () => {
    const next = { ...base, paneWidths: { side: null, dock: null }, bypassOff: [] } as unknown as SettingsView;
    expect(settingsPatch(base, next)).toEqual({ paneWidths: { dock: null }, bypassOff: [] });
  });

  it('is null when nothing changed', () => {
    expect(settingsPatch(base, { ...base })).toBeNull();
  });

  it('two viewers: patches of different fields both stay', () => {
    const a = settingsPatch(base, { ...base, minimapFolded: true })!;
    const b = settingsPatch(base, { ...base, arrangeDirection: 'left' } as SettingsView)!;
    const merged = applySettingsPatch(applySettingsPatch(base, a), b);
    expect(merged).toMatchObject({ minimapFolded: true, arrangeDirection: 'left' });
  });
});
