// @vitest-environment jsdom
import './testSetup';
import { ReactFlowProvider } from '@xyflow/react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as api from './api';
import { Tree } from './Tree';
import { deferred, nodeView, Providers, repo, settings } from './testSupport';

vi.mock('./api', async (importOriginal) => ({ ...(await importOriginal<typeof import('./api')>()), renameNode: vi.fn() }));
const renameNode = vi.mocked(api.renameNode);

const views = new Map([1, 2].map((id) => [id, nodeView(id, id === 1 ? 'alpha' : 'beta')]));

const mount = () =>
  render(
    <Providers>
      <ReactFlowProvider>
        <Tree
          repos={[repo]}
          current={1}
          onRepo={() => {}}
          onClear={() => {}}
          onAdded={() => {}}
          views={views}
          agents={[]}
          onRescanAgents={() => {}}
          selected={null}
          onSelect={() => {}}
          chosenBox={null}
          onChooseBox={() => {}}
          settings={settings}
          accounts={null}
          onRepoChanged={() => {}}
          onRepoRemoved={() => {}}
          find={null}
        />
      </ReactFlowProvider>
    </Providers>,
  );

const open = (name: string) => fireEvent.doubleClick(screen.getByText(name));
const field = () => screen.getByLabelText('Node name');
const settle = () => act(async () => {});

beforeEach(() => {
  renameNode.mockReset();
});

describe('Tree name edit', () => {
  it('saves a changed name on Enter and closes the field', async () => {
    renameNode.mockResolvedValue(null);
    mount();
    open('alpha');
    fireEvent.change(field(), { target: { value: 'gamma' } });
    fireEvent.keyDown(field(), { key: 'Enter' });
    await settle();
    expect(renameNode).toHaveBeenCalledExactlyOnceWith(1, 'gamma');
    expect(screen.queryByLabelText('Node name')).toBeNull();
  });

  it('saves on blur once, even when Enter and blur both fire', async () => {
    renameNode.mockResolvedValue(null);
    mount();
    open('alpha');
    fireEvent.change(field(), { target: { value: 'gamma' } });
    fireEvent.keyDown(field(), { key: 'Enter' });
    fireEvent.blur(field());
    await settle();
    expect(renameNode).toHaveBeenCalledTimes(1);
  });

  it('closes without a save when the name is unchanged or Escape is pressed', async () => {
    mount();
    open('alpha');
    fireEvent.blur(field());
    await settle();
    expect(screen.queryByLabelText('Node name')).toBeNull();
    open('alpha');
    const input = field();
    fireEvent.change(input, { target: { value: 'gamma' } });
    fireEvent.keyDown(input, { key: 'Escape' });
    fireEvent.blur(input);
    await settle();
    expect(screen.queryByLabelText('Node name')).toBeNull();
    expect(renameNode).not.toHaveBeenCalled();
  });

  it('keeps the field open with the refusal as an alert, then retries', async () => {
    renameNode.mockRejectedValueOnce(new Error('name is empty')).mockResolvedValueOnce(null);
    mount();
    open('alpha');
    fireEvent.change(field(), { target: { value: ' ' } });
    fireEvent.keyDown(field(), { key: 'Enter' });
    await settle();
    expect(screen.getByRole('alert', { hidden: true }).textContent).toBe('name is empty');
    expect(field().getAttribute('aria-invalid')).toBe('true');
    fireEvent.change(field(), { target: { value: 'gamma' } });
    expect(screen.queryByRole('alert', { hidden: true })).toBeNull();
    fireEvent.keyDown(field(), { key: 'Enter' });
    await settle();
    expect(renameNode).toHaveBeenLastCalledWith(1, 'gamma');
    expect(screen.queryByLabelText('Node name')).toBeNull();
  });

  it('makes the field read-only while the save is pending', () => {
    renameNode.mockReturnValue(deferred().promise);
    mount();
    open('alpha');
    fireEvent.change(field(), { target: { value: 'gamma' } });
    fireEvent.keyDown(field(), { key: 'Enter' });
    expect(field().hasAttribute('readonly')).toBe(true);
  });

  // 129750a7: the blur save of row A ends after row B opened its field.
  it('a late save of one row does not close the field of another row', async () => {
    const late = deferred();
    renameNode.mockReturnValue(late.promise);
    mount();
    open('alpha');
    fireEvent.change(field(), { target: { value: 'gamma' } });
    fireEvent.blur(field());
    open('beta');
    expect(screen.getByLabelText('Node name')).toHaveProperty('value', 'beta');
    late.resolve(null);
    await settle();
    expect(field()).toHaveProperty('value', 'beta');
  });

  // 67c48888: the blur save of row A is refused after row B opened its field.
  it('a late refusal of a closed field shows as that row error and keeps the open field', async () => {
    const late = deferred();
    renameNode.mockReturnValue(late.promise);
    mount();
    open('alpha');
    fireEvent.change(field(), { target: { value: ' ' } });
    fireEvent.blur(field());
    open('beta');
    late.reject(new Error('name is empty'));
    await settle();
    expect(field()).toHaveProperty('value', 'beta');
    const alert = screen.getByRole('alert', { hidden: true });
    expect(alert.textContent).toBe('name is empty');
    expect(alert.closest('li')?.textContent).toContain('alpha');
  });
});
