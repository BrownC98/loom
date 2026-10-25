import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { watchComposition } from './composition';

const fire = (target: EventTarget, type: string) => target.dispatchEvent(new Event(type));

describe('watchComposition', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('runs nothing late when no composition is open', () => {
    const watched = watchComposition(new EventTarget());
    expect(watched.afterText(() => {})).toBe(false);
  });

  it('holds an Enter that comes in the task of the last syllable until the editor has read that syllable', () => {
    const target = new EventTarget();
    const sent: string[] = [];
    target.addEventListener('compositionupdate', () => setTimeout(() => sent.push('end read')));
    target.addEventListener('compositionend', () => setTimeout(() => sent.push('last syllable')));
    const watched = watchComposition(target);
    fire(target, 'compositionstart');
    fire(target, 'compositionupdate');
    fire(target, 'compositionend');
    expect(watched.afterText(() => sent.push('\r'))).toBe(true);
    expect(sent).toEqual([]);
    vi.runAllTimers();
    expect(sent).toEqual(['end read', 'last syllable', '\r']);
  });

  it('holds an Enter that comes while the composition stays open for one task only', () => {
    const target = new EventTarget();
    const sent: string[] = [];
    target.addEventListener('compositionupdate', () => setTimeout(() => sent.push('end read')));
    const watched = watchComposition(target);
    fire(target, 'compositionstart');
    fire(target, 'compositionupdate');
    expect(watched.afterText(() => sent.push('\r'))).toBe(true);
    vi.advanceTimersToNextTimer();
    vi.advanceTimersToNextTimer();
    expect(sent).toEqual(['end read', '\r']);
    expect(watched.afterText(() => sent.push('\r'))).toBe(true);
    vi.runAllTimers();
    expect(sent).toEqual(['end read', '\r', '\r']);
  });

  it('drops held work and stops listening when disposed', () => {
    const target = new EventTarget();
    const sent: string[] = [];
    const watched = watchComposition(target);
    fire(target, 'compositionstart');
    watched.afterText(() => sent.push('\r'));
    watched.dispose();
    fire(target, 'compositionend');
    vi.runAllTimers();
    expect(sent).toEqual([]);
    expect(watched.afterText(() => {})).toBe(false);
  });
});
