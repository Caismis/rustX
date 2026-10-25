import { act, cleanup, fireEvent, render, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { RuntimeClientTranscriptEntry } from '../../protocol/app-server/v57';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { CompactionCheckpoint } from '../src/app/agent/CompactionCheckpoint';
import { useCompactionHistory } from '../src/app/agent/useCompactionHistory';
import { installTranscriptWindow } from '../src/client/transcript';
import { projectionHead, readingWindow, Server, snapshot } from './fixture';

afterEach(cleanup);
const entry = (cursor: string, text: string): RuntimeClientTranscriptEntry => ({ cursor, item: { type: 'message', message: { role: 'assistant', id: `message-${cursor}`, content: [{ type: 'text', text }] } } });
const marker = (id: string, anchor: string, server: Server) => {
  const head = { ...projectionHead(`Summary ${id}`), checkpoint_id: id, activation_id: id, transcript_anchor: anchor };
  return { head, content: <CompactionCheckpoint head={head} client={server.client} sessionId="A"/> };
};
const before = (a: Element, b: Element) => expect(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

it('keeps each checkpoint after its original response tail through later replies and a second compaction', () => {
  const server = new Server(), first = entry('10', 'First answer'), second = entry('20', 'Next answer');
  first.completed_response = { closing_locator: { type: 'by_message_id', message_id: 'message-10' }, retry_locator: { type: 'by_message_id', message_id: 'user' }, deferred_fields: [], closing_message_id: 'message-10', origin: { conversation_id: 'original', attempt_id: 'first' }, surface_revision: '2', retry_message_id: 'user', completed_at: '2026-10-20T00:00:00Z' };
  const marks = [marker('1', '10', server)];
  const ui = render(<AgentTranscript snapshot={{ ...snapshot(), messages: [], transcript: { entries: [first] } }} compactions={marks}/>);
  const checkpoint = ui.container.querySelector('[data-compaction-checkpoint="1"]')!;
  before(ui.container.querySelector('[data-turn-tail]')!, checkpoint);
  fireEvent.click(ui.getByRole('button', { name: 'Context compacted · View compaction summary' }));
  ui.rerender(<AgentTranscript snapshot={{ ...snapshot(), messages: [], transcript: { entries: [first, second, entry('30', 'Latest answer')] } }} compactions={[...marks, marker('2', '20', server)]}/>);
  expect(ui.container.querySelector('[data-compaction-checkpoint="1"]')).toBe(checkpoint);
  expect(ui.getByText('Summary 1')).toBeTruthy(); // No remount/collapse on new output.
  before(checkpoint, ui.getByText('Next answer'));
  before(ui.getByText('Next answer'), ui.container.querySelector('[data-compaction-checkpoint="2"]')!);
  before(ui.container.querySelector('[data-compaction-checkpoint="2"]')!, ui.getByText('Latest answer'));
});

it('does not append out-of-window checkpoints or put gap markers beside unrelated messages', () => {
  const server = new Server(), old = entry('10', 'Old answer'), live = entry('90', 'Live answer');
  const history = installTranscriptWindow({ ...readingWindow({ entries: [old], next_cursor: '10' }), newer_cursor: '10' });
  const ui = render(<AgentTranscript snapshot={{ ...snapshot(), messages: [], transcript: { entries: [live], next_cursor: '90' } }} history={history} loadLater={vi.fn()} compactions={[marker('1', '5', server), marker('2', '10', server), marker('3', '50', server), marker('4', '90', server), marker('5', '95', server)]}/>);
  expect([...ui.container.querySelectorAll('[data-compaction-checkpoint]')].map(el => el.getAttribute('data-compaction-checkpoint'))).toEqual(['2', '4']);
  before(ui.container.querySelector('[data-compaction-checkpoint="2"]')!, ui.container.querySelector('[data-chat-history-boundary]')!);
  before(ui.getByText('Live answer'), ui.container.querySelector('[data-compaction-checkpoint="4"]')!);
});

it('keeps inherited checkpoints before the fork boundary and orders checkpoints sharing a position', () => {
  const server = new Server();
  const ui = render(<AgentTranscript snapshot={{ ...snapshot(), messages: [], transcript: { inherited_through: '10', entries: [entry('10', 'Inherited answer'), entry('20', 'Branch answer')] } }} forkPoint={{ through: '10', origin: { type: 'fork', side: 'after', source_session: 'B', source_node: 'node-B', source_surface_revision: '1', source_message: 'source' } }} compactions={[marker('2', '10', server), marker('1', '10', server)]}/>);
  expect([...ui.container.querySelectorAll('[data-compaction-checkpoint]')].map(el => el.getAttribute('data-compaction-checkpoint'))).toEqual(['1', '2']);
  before(ui.container.querySelector('[data-compaction-checkpoint="2"]')!, ui.container.querySelector('[data-chat-anchor-key="fork-point"]')!);
  before(ui.container.querySelector('[data-compaction-checkpoint="2"]')!, ui.getByText('Branch answer'));
});

it('splits process steps at an automatic checkpoint rather than collecting later reasoning above it', () => {
  const server = new Server(), first = entry('10', ''), later = entry('20', '');
  const process = { conversation_id: 'original', attempt_id: 'attempt', final_message_id_deferred: false, outcome: 'completed' as const, control_cursor: '10', tool_call_count: 0, message_count: 2 };
  for (const [row, text] of [[first, 'Before compression'], [later, 'After compression']] as const) {
    row.turn_process = process;
    if (row.item.type === 'message' && row.item.message.role === 'assistant') row.item.message.content = [{ type: 'reasoning', text }];
  }
  const ui = render(<AgentTranscript snapshot={{ ...snapshot(), messages: [], transcript: { entries: [first, later] } }} compactions={[marker('1', '10', server)]}/>);
  const checkpoint = ui.container.querySelector('[data-compaction-checkpoint]')!;
  // Hidden process content still has the correct chronological DOM seat.
  before(ui.getByText('Before compression'), checkpoint);
  before(checkpoint, ui.getByText('After compression'));
});

it('reads all native checkpoint pages and rejects late history from a replaced attachment', async () => {
  const server = new Server(); await server.attached('A');
  try {
    const first = marker('1', '10', server).head, latest = marker('2', '20', server).head;
    server.snapshots.set('A', { ...snapshot(), context: { compaction_count: 2, compaction_in_progress: false, projection_head: latest } });
    await server.client.refresh('A');
    const read = vi.spyOn(server.client, 'projections').mockImplementation(async (_id, after) => after === '0' ? { records: [first], next_activation: '1' } : { records: [latest], next_activation: null });
    const ui = renderHook(() => useCompactionHistory(server.client, 'A', true));
    await waitFor(() => expect(ui.result.current.map(item => item.head.activation_id)).toEqual(['1', '2']));
    expect(read.mock.calls).toEqual([['A', '0'], ['A', '1']]);
    const finishes: ((page: { records: typeof first[]; next_activation: null }) => void)[] = [];
    read.mockImplementation(() => new Promise(resolve => { finishes.push(resolve); }));
    await act(async () => { await server.client.release('A'); await server.client.attach('A'); });
    await waitFor(() => expect(finishes.length).toBeGreaterThan(0));
    await act(async () => { await server.client.release('A'); });
    await act(async () => { for (const finish of finishes) finish({ records: [first, latest], next_activation: null }); });
    expect(ui.result.current).toEqual([]);
  } finally { server.client.disconnect(); }
});
