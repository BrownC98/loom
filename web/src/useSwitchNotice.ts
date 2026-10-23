import { useEffect, useState } from 'react';
import { getEventLog } from './api';
import { isAutoSwitch, isSwitchEntry, latestSwitch, type SwitchEntry } from './accountSwitch';
import type { EventRecord } from './gen/EventRecord';

export type SwitchRecord = EventRecord & { entry: SwitchEntry };

export function useSwitchNotice(closed: number | null, onClose: (id: number) => void, onAuto: () => void) {
  const [latest, setLatest] = useState<SwitchRecord | null>(null);
  useEffect(() => {
    getEventLog()
      .then((log) => setLatest(latestSwitch(log) ?? null))
      .catch(() => {});
  }, []);
  const onRecord = (next: EventRecord) => {
    if (!isSwitchEntry(next.entry)) return;
    setLatest(next as SwitchRecord);
    if (isAutoSwitch(next.entry)) onAuto();
  };
  const record = latest && latest.id !== closed ? latest : null;
  const close = () => {
    if (record) onClose(record.id);
  };
  return { record, onRecord, close };
}
