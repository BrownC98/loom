import { useEffect, useState } from 'react';
import { getEventLog } from './api';
import { isAutoSwitch, isSwitchEntry, latestSwitch, type SwitchEntry } from './accountSwitch';
import type { EventRecord } from './gen/EventRecord';

export type SwitchRecord = EventRecord & { entry: SwitchEntry };

/** 상태 줄 알림 하나. 처음에 이벤트 기록의 가장 새 전환을 읽고, 그 뒤는 `onRecord`로 받은 새 기록이 덮는다.
 *  닫은 알림의 기록 번호(`closed`)는 데몬 설정에 있다. 그 번호의 알림은 보이지 않는다. 닫기 전에는 시간이 지나도 남는다.
 *  `onAuto`는 데몬이 스스로 바꾼 기록이 올 때 부른다. */
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
