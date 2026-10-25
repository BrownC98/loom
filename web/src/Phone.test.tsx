import './testSetup';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { Theme } from './gen/Theme';
import { readRoute, SettingsSheet } from './Phone';
import { NOTIFY_EVENT, type NotifyState, type PhoneNotify } from './phoneNotify';
import { Providers } from './testSupport';

vi.mock('./Terminal', () => ({ borrowSize: () => {}, SizingTerminal: () => null }));

function bridge(state: NotifyState | string) {
  let now = typeof state === 'string' ? state : JSON.stringify(state);
  const fake = { state: vi.fn(() => now), ask: vi.fn(), done: vi.fn(), link: vi.fn() } satisfies PhoneNotify;
  return { fake, change: (next: NotifyState) => (now = JSON.stringify(next)) };
}

const mount = (notify: PhoneNotify | undefined, onTheme: (next: Theme) => void = () => {}) =>
  render(
    <Providers>
      <SettingsSheet theme="dark" onTheme={onTheme} notify={notify} onClose={() => {}} />
    </Providers>,
  );

const toggle = (name: string) => screen.getByRole<HTMLInputElement>('switch', { name: new RegExp(`^${name}`) });

describe('phone settings sheet', () => {
  it('keeps the theme rows and hides phone notifications in a plain browser', () => {
    const onTheme = vi.fn();
    mount(undefined, onTheme);
    expect(screen.getByRole('dialog', { name: 'Settings' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Dark' }).getAttribute('aria-pressed')).toBe('true');
    fireEvent.click(screen.getByRole('button', { name: 'Light' }));
    expect(onTheme).toHaveBeenCalledWith('light');
    expect(screen.queryByText('Phone notifications')).toBeNull();
    expect(screen.queryAllByRole('switch')).toHaveLength(0);
  });

  it('shows the two switches from the app and sends each change to it', () => {
    const { fake } = bridge({ ask: true, done: false, allowed: true });
    mount(fake);
    expect(screen.getByText('Phone notifications')).toBeTruthy();
    expect(toggle('Questions').checked).toBe(true);
    expect(toggle('Done').checked).toBe(false);
    expect(screen.getByText('Things that wait for your answer')).toBeTruthy();
    expect(screen.getByText('Work you gave is finished')).toBeTruthy();
    fireEvent.click(toggle('Done'));
    expect(fake.done).toHaveBeenCalledWith(true);
    fireEvent.click(toggle('Questions'));
    expect(fake.ask).toHaveBeenCalledWith(false);
  });

  it('opens the connection notice settings from the app', () => {
    const { fake } = bridge({ ask: true, done: true, allowed: true });
    mount(fake);
    fireEvent.click(screen.getByRole('button', { name: 'Hide the PC connection notice' }));
    expect(fake.link).toHaveBeenCalledOnce();
    expect(screen.getByText(/^The 'Connected to the PC' notice keeps the PC link open/)).toBeTruthy();
  });

  it('hides the connection notice button for an app without it', () => {
    const { fake } = bridge({ ask: true, done: true, allowed: true });
    mount({ state: fake.state, ask: fake.ask, done: fake.done });
    expect(toggle('Questions').checked).toBe(true);
    expect(screen.queryByRole('button', { name: 'Hide the PC connection notice' })).toBeNull();
    expect(screen.queryByText(/Connected to the PC/)).toBeNull();
  });

  it('reads the state again when the app says it changed', () => {
    const { fake, change } = bridge({ ask: false, done: false, allowed: false });
    mount(fake);
    expect(screen.getAllByText('Android notifications for loom are off')).toHaveLength(2);
    expect(toggle('Questions').checked).toBe(false);
    fireEvent.click(toggle('Questions'));
    expect(fake.ask).toHaveBeenCalledWith(true);
    expect(toggle('Questions').checked).toBe(false);
    change({ ask: true, done: false, allowed: true });
    act(() => void window.dispatchEvent(new Event(NOTIFY_EVENT)));
    expect(toggle('Questions').checked).toBe(true);
    expect(screen.queryByText('Android notifications for loom are off')).toBeNull();
  });

  it('shows both switches off when the app state is not readable', () => {
    mount(bridge('not json').fake);
    expect(toggle('Questions').checked).toBe(false);
    expect(toggle('Done').checked).toBe(false);
    expect(screen.getAllByText('Android notifications for loom are off')).toHaveLength(2);
  });
});

describe('phone route', () => {
  it('opens the question panel for a notification link', () => {
    expect(readRoute('#/node/5/ask')).toEqual({ screen: 'node', id: 5, ask: true });
    expect(readRoute('#/node/5')).toEqual({ screen: 'node', id: 5 });
    expect(readRoute('#/node/5/other')).toEqual({ screen: 'nodes' });
    expect(readRoute('#/inbox')).toEqual({ screen: 'inbox' });
  });
});
