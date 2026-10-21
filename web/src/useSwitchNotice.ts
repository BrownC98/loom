import { useEffect, useState } from 'react';
import { getEventLog } from './api';
import { isAutoSwitch, isSwitchEntry, latestSwitch, type SwitchEntry } from './accountSwitch';
import type { EventRecord } from './gen/EventRecord';

/** 닫은 알림의 기록 id를 이 키에 적는다. 다시 열면 닫은 알림은 보이지 않는다. */
const CLOSED_KEY = 'loom.switchNoticeClosed';

function closedId(): number | null {
  try {
    const n = Number(localStorage.getItem(CLOSED_KEY));
    return Number.isFinite(n) && n > 0 ? n : null;
  } catch {
    return null;
  }
}

function rememberClosed(id: number) {
  try {
    localStorage.setItem(CLOSED_KEY, String(id));
  } catch {
    // 저장하지 못하면 다시 열 때 그 알림이 한 번 더 보인다.
  }
}

export type SwitchRecord = EventRecord & { entry: SwitchEntry };

/** 상태 줄 알림 하나. 처음에 이벤트 기록의 가장 새 전환을 읽고(닫은 것은 뺀다), 그 뒤는 `onRecord`로 받은 새 기록이 덮는다.
 *  닫기 전에는 시간이 지나도 남는다. `onAuto`는 데몬이 스스로 바꾼 기록이 올 때 부른다. */
export function useSwitchNotice(onAuto: () => void) {
  const [record, setRecord] = useState<SwitchRecord | null>(null);
  useEffect(() => {
    getEventLog()
      .then((log) => {
        const latest = latestSwitch(log);
        if (latest && latest.id !== closedId()) setRecord(latest);
      })
      .catch(() => {});
  }, []);
  const onRecord = (next: EventRecord) => {
    if (!isSwitchEntry(next.entry)) return;
    setRecord(next as SwitchRecord);
    if (isAutoSwitch(next.entry)) onAuto();
  };
  const close = () => {
    if (record) rememberClosed(record.id);
    setRecord(null);
  };
  return { record, onRecord, close };
}
