import { describe, expect, it, vi } from 'vitest';
import { editEnd } from './editEnd';

const settle = () => new Promise((r) => setTimeout(r));

describe('editEnd', () => {
  it('saves a changed name on blur and closes after the save', async () => {
    const end = editEnd();
    const save = vi.fn(() => Promise.resolve());
    const close = vi.fn();
    expect(end.save('new', 'old', save, close, () => {})).toBe(true);
    expect(save).toHaveBeenCalledWith('new');
    await settle();
    expect(close).toHaveBeenCalledOnce();
  });

  it('closes without a request when the name did not change', () => {
    const end = editEnd();
    const save = vi.fn(() => Promise.resolve());
    const close = vi.fn();
    expect(end.save(' old ', 'old', save, close, () => {})).toBe(false);
    expect(save).not.toHaveBeenCalled();
    expect(close).toHaveBeenCalledOnce();
  });

  it('does not save on the blur that follows Esc', () => {
    const end = editEnd();
    const save = vi.fn(() => Promise.resolve());
    const close = vi.fn();
    end.cancel(close);
    expect(end.save('new', 'old', save, close, () => {})).toBe(false);
    expect(save).not.toHaveBeenCalled();
    expect(close).toHaveBeenCalledOnce();
  });

  it('sends one request when blur follows Enter', async () => {
    const end = editEnd();
    const save = vi.fn(() => Promise.resolve());
    const close = vi.fn();
    end.save('new', 'old', save, close, () => {});
    end.save('new', 'old', save, close, () => {});
    await settle();
    expect(save).toHaveBeenCalledOnce();
    expect(close).toHaveBeenCalledOnce();
  });

  it('keeps the edit open after a refusal and lets the next blur try again', async () => {
    const end = editEnd();
    const save = vi.fn().mockRejectedValueOnce(new Error('empty name')).mockResolvedValueOnce(undefined);
    const close = vi.fn();
    const fail = vi.fn();
    end.save('', 'old', save, close, fail);
    await settle();
    expect(fail).toHaveBeenCalledWith('empty name');
    expect(close).not.toHaveBeenCalled();
    end.save('new', 'old', save, close, fail);
    await settle();
    expect(save).toHaveBeenCalledTimes(2);
    expect(close).toHaveBeenCalledOnce();
  });
});
