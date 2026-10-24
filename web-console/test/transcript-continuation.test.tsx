import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { installTranscriptWindow } from '../src/client/transcript';
import { snapshot, readingWindow } from './fixture';
import type { RuntimeClientTranscriptEntry } from '../../protocol/app-server/v57';

const entry = (cursor: string, text: string): RuntimeClientTranscriptEntry => ({ cursor, item: { type: 'message', message: { id: `m-${cursor}`, role: 'assistant', content: [{ type: 'text', text }] } } });
afterEach(cleanup);
const paint = () => new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
it('uses the Harness older control and seats silent continuation after history, before the live tail', () => {
  const old = entry('10', 'Historical answer'), current = entry('90', 'Current answer');
  const history = installTranscriptWindow({ ...readingWindow({ entries: [old], next_cursor: '10' }), newer_cursor: '10' });
  const older = vi.fn(), later = vi.fn();
  const ui = render(<div data-conversation-scroll><AgentTranscript snapshot={{ ...snapshot(), transcript: { entries: [current] } }} history={history} loadEarlier={older} loadLater={later}/></div>);
  fireEvent.click(screen.getByRole('button', { name: 'Load earlier' }));
  expect(older).toHaveBeenCalledOnce();
  expect(screen.queryByRole('button', { name: 'Load later' })).toBeNull();
  const marker = ui.container.querySelector('[data-chat-history-boundary]')!;
  expect(screen.getByText('Historical answer').compareDocumentPosition(marker) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(marker.compareDocumentPosition(screen.getByText('Current answer')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(later).not.toHaveBeenCalled();
});

it('reads only on downward intent at the window edge, coalesces pending reads, and cleans up on latest', async () => {
  const source = { ...snapshot(), transcript: { entries: [entry('90', 'Live answer')] } };
  const history = installTranscriptWindow({ ...readingWindow({ entries: [entry('10', 'Old answer')] }), newer_cursor: '10' });
  let release!: () => void;
  const later = vi.fn(() => new Promise<void>(resolve => { release = resolve; }));
  const ui = render(<div data-conversation-scroll><AgentTranscript snapshot={source} history={history} loadLater={later}/></div>);
  const viewport = ui.container.firstElementChild!, marker = ui.container.querySelector('[data-chat-history-boundary]')!;
  vi.spyOn(viewport, 'getBoundingClientRect').mockReturnValue({ top: 0, bottom: 500 } as DOMRect);
  const bounds = vi.spyOn(marker, 'getBoundingClientRect').mockReturnValue({ top: 900, bottom: 901 } as DOMRect);
  await act(async () => { fireEvent.wheel(viewport, { deltaY: 100 }); await paint(); });
  expect(later).not.toHaveBeenCalled();
  bounds.mockReturnValue({ top: 480, bottom: 481 } as DOMRect);
  fireEvent.wheel(viewport, { deltaY: -100 });
  expect(later).not.toHaveBeenCalled();
  viewport.scrollTop = 200; fireEvent.scroll(viewport);
  await act(async () => { await paint(); });
  expect(later).not.toHaveBeenCalled();
  await act(async () => { fireEvent.wheel(viewport, { deltaY: 100 }); fireEvent.wheel(viewport, { deltaY: 100 }); await paint(); });
  expect(later).toHaveBeenCalledOnce();
  await act(async () => { fireEvent.wheel(viewport, { deltaY: 100 }); await paint(); });
  expect(later).toHaveBeenCalledOnce();
  await act(async () => release());
  fireEvent.touchStart(viewport, { touches: [{ clientY: 300 }] });
  await act(async () => { fireEvent.touchMove(viewport, { touches: [{ clientY: 200 }] }); await paint(); });
  expect(later).toHaveBeenCalledTimes(2);
  await act(async () => release());
  ui.rerender(<div data-conversation-scroll><AgentTranscript snapshot={source}/></div>);
  fireEvent.wheel(viewport, { deltaY: 100 });
  expect(later).toHaveBeenCalledTimes(2);
  expect(ui.container.querySelector('[data-chat-history-boundary]')).toBeNull();
});

it('allows PageDown from message controls while editing and button activation do not paginate', async () => {
  const history = installTranscriptWindow({ ...readingWindow({ entries: [entry('10', 'Old answer')] }), newer_cursor: '10' });
  const later = vi.fn(async () => {});
  const ui = render(<div data-conversation-scroll><button type="button">Copy message</button><input aria-label="Draft"/><AgentTranscript snapshot={snapshot()} history={history} loadLater={later}/></div>);
  vi.spyOn(ui.container.firstElementChild!, 'getBoundingClientRect').mockReturnValue({ top: 0, bottom: 500 } as DOMRect);
  vi.spyOn(ui.container.querySelector('[data-chat-history-boundary]')!, 'getBoundingClientRect').mockReturnValue({ top: 480, bottom: 481 } as DOMRect);
  await act(async () => {
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'PageDown' });
    fireEvent.keyDown(screen.getByRole('button', { name: 'Copy message' }), { key: ' ' });
    await paint();
  });
  expect(later).not.toHaveBeenCalled();
  await act(async () => { fireEvent.keyDown(screen.getByRole('button', { name: 'Copy message' }), { key: 'PageDown' }); await paint(); });
  expect(later).toHaveBeenCalledOnce();
});
