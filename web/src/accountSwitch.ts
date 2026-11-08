import type { Entry } from './gen/Entry';

export function isAutoSwitch(entry: Entry): boolean {
  return entry.kind === 'accountSwitched' && entry.why.kind !== 'manual';
}

export function rowStateKey(busy: boolean) {
  return busy ? ('switch.dialog.busy' as const) : ('switch.dialog.idle' as const);
}
