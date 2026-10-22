import { beforeEach, describe, expect, it, vi } from 'vitest';

const plugin = vi.hoisted(() => ({ sent: [] as { id: number; title: string; body: string }[], action: null as ((e: { notification?: { id: number } }) => void) | null }));
const app = vi.hoisted(() => ({ tauri: true }));

vi.mock('@tauri-apps/api/core', () => ({ isTauri: () => app.tauri }));
vi.mock('@tauri-apps/plugin-notification', () => ({
  sendNotification: (o: { id: number; title: string; body: string }) => plugin.sent.push(o),
  onAction: async (cb: (e: { notification?: { id: number } }) => void) => {
    plugin.action = cb;
  },
}));

const { notify } = await import('./osNotify');

describe('notify', () => {
  beforeEach(() => {
    plugin.sent.length = 0;
  });

  it('in the app, sends through the plugin and runs the click of the clicked notification only once', () => {
    const a = vi.fn();
    const b = vi.fn();
    notify('A', 'a', 'tag-a', a);
    notify('B', 'b', 'tag-b', b);
    expect(plugin.sent.map((n) => n.title)).toEqual(['A', 'B']);
    plugin.action!({ notification: { id: plugin.sent[1].id } });
    plugin.action!({ notification: { id: plugin.sent[1].id } });
    plugin.action!({});
    expect(a).not.toHaveBeenCalled();
    expect(b).toHaveBeenCalledTimes(1);
  });

  it('in a browser, uses Web Notification and its click', () => {
    app.tauri = false;
    const notes: { title: string; options: NotificationOptions; onclick?: () => void; close: () => void }[] = [];
    vi.stubGlobal('Notification', function (this: (typeof notes)[0], title: string, options: NotificationOptions) {
      Object.assign(this, { title, options, close: vi.fn() });
      notes.push(this);
    });
    vi.stubGlobal('window', { focus: vi.fn() });
    const click = vi.fn();
    notify('A', 'a', 'tag-a', click);
    expect(plugin.sent).toEqual([]);
    expect(notes[0].options).toEqual({ body: 'a', tag: 'tag-a' });
    notes[0].onclick!();
    expect(click).toHaveBeenCalledTimes(1);
    expect(notes[0].close).toHaveBeenCalled();
  });
});
