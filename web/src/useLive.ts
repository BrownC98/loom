import { useRef, useState } from 'react';
import type { NodeId } from './gen/NodeId';
import type { Toward } from './gen/Toward';
import { pickHue, type HopEndName } from './hopLanes';

export type Pulse = Readonly<{ hue: number; last: Toward; counts: Readonly<Record<Toward, number>> }>;

export type Hop = Readonly<{ source: NodeId; target: NodeId; hue: number; last: HopEndName; counts: Readonly<Record<HopEndName, number>> }>;

type Live = Readonly<{ pulses: ReadonlyMap<NodeId, Pulse>; hops: ReadonlyMap<string, Hop> }>;

const PULSE_MS = 3000;

export function useLive() {
  const [live, setLive] = useState<Live>({ pulses: new Map(), hops: new Map() });
  const timers = useRef(new Map<string, number>());
  const flash = (timer: string, bump: (now: Live) => Live, drop: (now: Live) => Live) => {
    setLive(bump);
    clearTimeout(timers.current.get(timer));
    timers.current.set(timer, window.setTimeout(() => setLive(drop), PULSE_MS));
  };
  const without = <K, V>(map: ReadonlyMap<K, V>, key: K) => {
    const next = new Map(map);
    next.delete(key);
    return next;
  };
  const hueOf = (now: Live, prev: { hue: number } | undefined) => prev?.hue ?? pickHue([...now.pulses.values(), ...now.hops.values()].map((edge) => edge.hue));
  const pulse = (child: NodeId, toward: Toward) =>
    flash(
      `pulse-${child}`,
      (now) => {
        const prev = now.pulses.get(child);
        const counts = prev?.counts ?? { parent: 0, child: 0 };
        return { ...now, pulses: new Map(now.pulses).set(child, { hue: hueOf(now, prev), last: toward, counts: { ...counts, [toward]: counts[toward] + 1 } }) };
      },
      (now) => ({ ...now, pulses: without(now.pulses, child) }),
    );
  const hop = (from: NodeId, to: NodeId) => {
    const [source, target] = from < to ? [from, to] : [to, from];
    const toward = to === target ? 'target' : 'source';
    const key = `${source}-${target}`;
    flash(
      `hop-${key}`,
      (now) => {
        const prev = now.hops.get(key);
        const counts = prev?.counts ?? { source: 0, target: 0 };
        return { ...now, hops: new Map(now.hops).set(key, { source, target, hue: hueOf(now, prev), last: toward, counts: { ...counts, [toward]: counts[toward] + 1 } }) };
      },
      (now) => ({ ...now, hops: without(now.hops, key) }),
    );
  };
  return { ...live, pulse, hop };
}
