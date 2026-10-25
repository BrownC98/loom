import './testSetup';
import { ReactFlowProvider } from '@xyflow/react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as api from './api';
import { Graph } from './Graph';
import { deferred, nodeView, Providers, repo, settings } from './testSupport';

vi.mock('./api', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  renameNode: vi.fn(),
  getSpots: vi.fn(),
  getSizes: vi.fn(),
}));
vi.mock('./Terminal', async (importOriginal) => ({ ...(await importOriginal<Record<string, unknown>>()), SizingTerminal: () => null }));
const renameNode = vi.mocked(api.renameNode);

const views = new Map([[1, nodeView(1, 'alpha')]]);

const mount = (onSelect: (id: number | null) => void = () => {}) =>
  render(
    <Providers>
      <ReactFlowProvider>
        <Graph
          repos={[repo]}
          onRepo={() => {}}
          views={views}
          pulses={new Map()}
          hops={new Map()}
          agents={[]}
          onRescanAgents={() => {}}
          settings={settings}
          viewVersion={0}
          onMinimapFold={() => {}}
          accounts={null}
          selected={null}
          focusAsk={0}
          onSelect={onSelect}
          onClear={() => {}}
          chosenBox={null}
          onChooseBox={() => {}}
          highlight={null}
          onClearHighlight={() => {}}
          direction="down"
        />
      </ReactFlowProvider>
    </Providers>,
  );

const open = () => fireEvent.doubleClick(screen.getByText('alpha'));
const field = () => screen.getByLabelText('Node name');
const settle = () => act(async () => {});

beforeEach(() => {
  renameNode.mockReset();
  vi.mocked(api.getSpots).mockResolvedValue({ placed: [], arranged: [] });
  vi.mocked(api.getSizes).mockResolvedValue([]);
});

describe('Graph card name edit', () => {
  it('saves a changed name on Enter and closes the field', async () => {
    renameNode.mockResolvedValue(null);
    mount();
    await settle();
    open();
    fireEvent.change(field(), { target: { value: 'gamma' } });
    fireEvent.keyDown(field(), { key: 'Enter' });
    await settle();
    expect(renameNode).toHaveBeenCalledExactlyOnceWith(1, 'gamma');
    expect(screen.queryByLabelText('Node name')).toBeNull();
  });

  it('saves once on blur and keeps the field read-only until the save ends', async () => {
    const pending = deferred();
    renameNode.mockReturnValue(pending.promise);
    mount();
    await settle();
    open();
    fireEvent.change(field(), { target: { value: 'gamma' } });
    fireEvent.blur(field());
    fireEvent.blur(field());
    expect(renameNode).toHaveBeenCalledTimes(1);
    expect(field().hasAttribute('readonly')).toBe(true);
    pending.resolve(null);
    await settle();
    expect(screen.queryByLabelText('Node name')).toBeNull();
  });

  it('closes without a save when the name is unchanged', async () => {
    mount();
    await settle();
    open();
    fireEvent.blur(field());
    await settle();
    expect(renameNode).not.toHaveBeenCalled();
    expect(screen.queryByLabelText('Node name')).toBeNull();
  });

  it('keeps the field open on a refusal and shows it as an alert', async () => {
    renameNode.mockRejectedValue(new Error('name is empty'));
    mount();
    await settle();
    open();
    fireEvent.change(field(), { target: { value: ' ' } });
    fireEvent.keyDown(field(), { key: 'Enter' });
    await settle();
    expect(screen.getByRole('alert', { hidden: true }).textContent).toBe('name is empty');
    expect(field().hasAttribute('readonly')).toBe(false);
    fireEvent.change(field(), { target: { value: 'gamma' } });
    expect(screen.queryByRole('alert', { hidden: true })).toBeNull();
  });
});

describe('Graph touch on a card', () => {
  const touch = (x: number, y: number) => ({ touches: [{ clientX: x, clientY: y }] });
  const camera = () => document.querySelector<HTMLElement>('.react-flow__viewport')!.style.transform;

  it('moves the camera on a drag and stops the click after it', async () => {
    mount();
    await settle();
    const before = camera();
    const card = screen.getByText('alpha');
    fireEvent.touchStart(card, touch(100, 100));
    fireEvent.touchMove(card, touch(104, 100));
    expect(camera()).toBe(before);
    fireEvent.touchMove(card, touch(140, 130));
    await settle();
    expect(camera()).toBe('translate(40px,30px) scale(1)');
    expect(fireEvent.touchEnd(card, { touches: [] })).toBe(false);
  });

  it('keeps a tap as a selection', async () => {
    const onSelect = vi.fn();
    mount(onSelect);
    await settle();
    const before = camera();
    const card = screen.getByText('alpha');
    fireEvent.touchStart(card, touch(100, 100));
    expect(fireEvent.touchEnd(card, { touches: [] })).toBe(true);
    fireEvent.mouseDown(card);
    fireEvent.click(card);
    expect(onSelect).toHaveBeenCalledWith(1);
    expect(camera()).toBe(before);
  });
});
