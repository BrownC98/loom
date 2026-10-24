import './testSetup';
import { ReactFlowProvider } from '@xyflow/react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as api from './api';
import type { HumanWait } from './gen/HumanWait';
import { InboxPanel } from './Inbox';
import { deferred, nodeView, Providers } from './testSupport';

vi.mock('./api', async (importOriginal) => ({ ...(await importOriginal<Record<string, unknown>>()), answerWait: vi.fn(), dismissWait: vi.fn() }));
const answerWait = vi.mocked(api.answerWait);
const dismissWait = vi.mocked(api.dismissWait);

const question: HumanWait = {
  id: 10,
  kind: 'question',
  text: 'Merge now?',
  since: 0,
  answerable: true,
  doc: null,
  images: [],
  choices: [{ key: 'A', label: 'Yes', recommended: true }],
};

const mount = () =>
  render(
    <Providers>
      <ReactFlowProvider>
        <InboxPanel waiting={[nodeView(1, 'alpha', { waits: [question] })]} onSelect={() => {}} onFocus={() => {}} />
      </ReactFlowProvider>
    </Providers>,
  );

const answerBox = () => screen.getByLabelText('Answer to Q10');
const settle = () => act(async () => {});

beforeEach(() => {
  answerWait.mockReset();
  dismissWait.mockReset();
});

describe('Inbox answers', () => {
  it('sends the typed answer on Enter and not on Shift+Enter or when empty', async () => {
    answerWait.mockResolvedValue(null);
    mount();
    fireEvent.keyDown(answerBox(), { key: 'Enter' });
    fireEvent.change(answerBox(), { target: { value: 'go' } });
    fireEvent.keyDown(answerBox(), { key: 'Enter', shiftKey: true });
    expect(answerWait).not.toHaveBeenCalled();
    fireEvent.keyDown(answerBox(), { key: 'Enter' });
    await settle();
    expect(answerWait).toHaveBeenCalledExactlyOnceWith(10, 'go', []);
  });

  it('sends the key of a picked choice', () => {
    answerWait.mockResolvedValue(null);
    mount();
    fireEvent.click(screen.getByText('Yes'));
    expect(answerWait).toHaveBeenCalledExactlyOnceWith(10, 'A', []);
  });

  it('shows a refused answer as an alert and clears it on the next try', async () => {
    const retry = deferred();
    answerWait.mockRejectedValueOnce(new Error('node is gone')).mockReturnValueOnce(retry.promise);
    mount();
    fireEvent.change(answerBox(), { target: { value: 'go' } });
    fireEvent.keyDown(answerBox(), { key: 'Enter' });
    await settle();
    expect(screen.getByRole('alert', { hidden: true }).textContent).toBe('node is gone');
    expect(answerBox()).toHaveProperty('value', 'go');
    fireEvent.keyDown(answerBox(), { key: 'Enter' });
    expect(screen.queryByRole('alert', { hidden: true })).toBeNull();
    expect(answerWait).toHaveBeenCalledTimes(2);
  });
});

describe('Inbox dismiss', () => {
  it('marks the button with the refusal and clears the mark on the next try', async () => {
    const retry = deferred();
    dismissWait.mockRejectedValueOnce(new Error('already closed')).mockReturnValueOnce(retry.promise);
    mount();
    const button = screen.getByLabelText('Dismiss');
    fireEvent.click(button);
    await settle();
    expect(button.getAttribute('data-refused')).toBe('true');
    expect(button.getAttribute('title')).toBe('already closed');
    fireEvent.click(button);
    expect(button.hasAttribute('data-refused')).toBe(false);
    expect(dismissWait).toHaveBeenCalledWith(10);
  });
});
