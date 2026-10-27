import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { CheckpointPage, CheckpointReadCut, RuntimeClientMessageDescriptor, RuntimeClientTranscriptEntry, TraceRecord, TraceToolDefinition, TurnProcessView } from '../../protocol/app-server/v61';
import { traceAttemptKey } from '../../protocol/app-server/trace-identity';
import { completeTools, readRequestPredecessor, readSystemRequests, toolChanges } from '../src/client/request-system';
import type { TraceFieldReader } from '../src/client/trace';
import { systemChangePlacements, useSystemChanges } from '../src/app/agent/SystemChanges';
import { JournalSourceProvider } from '../src/app/agent/ObservationSource';
import type { AppServerClient } from '../src/client/app-server';
import { traceRecord, requestDetail } from './trace-fixture';
import { nativeTraceFixture } from './native-trace-fixture';
import { checkpoint, snapshot, readCut } from './fixture';
import { translator } from '../src/locale/translation';
import { localeController } from '../src/locale/controller';

afterEach(() => { cleanup(); localeController.setLocale('en'); });
const cut: CheckpointReadCut = { ...checkpoint(snapshot()).read_cut, source_cut: { read: readCut(), ledger_through: '0', transcript_total: '0', surface_revision: '0', inherited_through: '0' } };
const turn: TurnProcessView = { conversation_id: cut.conversation_id, attempt_id: 'attempt-a', final_message_id: 'message-4', final_message_id_deferred: false, outcome: 'completed', control_cursor: '1', message_count: 2, tool_call_count: 0 };
const entries: RuntimeClientTranscriptEntry[] = [
  { cursor: '1', turn_process: turn, item: { type: 'message', message: { role: 'user', id: 'user-a', source: 'human', content: [{ type: 'text', text: 'Inspect' }] } } },
  { cursor: '2', turn_process: turn, item: { type: 'message', message: { role: 'assistant', id: 'message-4', content: [{ type: 'text', text: 'Response' }] } } },
];
function tracePage(cut: CheckpointReadCut, records: TraceRecord[], next?: number): CheckpointPage {
  return { read_cut: cut, cursor: cut.revision, domain: 'trace.records', total: '3', next_offset: next == null ? null : String(next), collections: [],
    items: records.map(record => ({ offset: record.id.slice(6), identity: record.id, value: record, deferred: false, content: [] })) };
}
function fixture() {
  const before = traceRecord(2), after = traceRecord(4), boundary = traceRecord(1, { kind: 'attempt', request: null });
  after.request!.predecessor = { availability: 'available', request_id: before.request!.request_id };
  after.request!.retry_number = 0; after.request!.system_prompt.state = 'changed'; after.request!.tool_catalog = 'changed';
  const previous = requestDetail(2), current = requestDetail(4);
  previous.request!.effective_system_prompt.text = 'OLD SYSTEM'; current.request!.effective_system_prompt.text = 'NEW SYSTEM\n' + 'x'.repeat(70_000) + '\nPROMPT_END';
  const oldTool = previous.request!.tools[0]!;
  current.request!.tools = [{ ...oldTool, input_schema: { value: { type: 'object', required: ['command'] }, truncated: false } }, { ...oldTool, name: 'search', tool_id: 'tool-search' }];
  const oldSource = nativeTraceFixture(before, previous), newSource = nativeTraceFixture(after, current);
  const reader: TraceFieldReader = {
    page: vi.fn(async (cut, domain, offset, owned) => {
      if (!owned()) throw Error('Retired source');
      if (domain === 'trace.records') return offset === 0 ? tracePage(cut, [after], 3) : tracePage(cut, [boundary, before]);
      return (domain.startsWith('trace.records.2.') ? oldSource : newSource).original.page(cut, domain, offset, owned);
    }),
    value: vi.fn(async (cut, field, offset, owned) => (field.domain.startsWith('trace.records.2.') ? oldSource : newSource).original.value(cut, field, offset, owned)),
    content: vi.fn(async () => { throw Error('No payload content expected'); }),
  };
  return { before, after, boundary, current, reader };
}

it('reads the complete represented native Attempt across pages, then stops at its start', async () => {
  const f = fixture();
  expect((await readSystemRequests(f.reader, cut, new Set([traceAttemptKey(turn)!]), () => true)).map(row => row.id)).toEqual(['trace:2', 'trace:4']);
  expect(vi.mocked(f.reader.page).mock.calls.map(call => call[2])).toEqual([0, 3]);
});

it('resolves the explicitly recorded predecessor rather than the adjacent request', async () => {
  const f = fixture(), wrong = traceRecord(3);
  const read = f.reader.page;
  f.reader.page = async (...args) => args[1] === 'trace.records' && args[2] === 0 ? tracePage(cut, [wrong, f.after], 3) : read(...args);
  expect((await readRequestPredecessor(f.reader, cut, f.after, () => true))?.id).toBe(f.before.id);
  f.after.request!.predecessor = { availability: 'unavailable' };
  expect(await readRequestPredecessor(f.reader, cut, f.after, () => true)).toBeUndefined();
});

it('rejects a foreign frozen scope and a cursor that fails to advance', async () => {
  const f = fixture();
  f.reader.page = async () => tracePage({ ...cut, conversation_id: 'foreign' }, []);
  await expect(readSystemRequests(f.reader, cut, new Set(['a']), () => true)).rejects.toThrow('scope changed');
  f.reader.page = async () => tracePage(cut, [], 3);
  await expect(readSystemRequests(f.reader, cut, new Set(['a']), () => true)).rejects.toThrow('did not advance');
});

it('includes uncalled schema and description edits in Tool changes and rejects partial catalogs', () => {
  const original = requestDetail(2).request!.tools, updated: TraceToolDefinition[] = [{ ...original[0]!, description: { text: 'New instructions', truncated: false } }];
  expect(toolChanges(original, updated)).toEqual({ added: [], removed: [], updated: ['bash'] });
  expect(toolChanges(original, [])).toEqual({ added: [], removed: ['bash'], updated: [] });
  expect(completeTools([{ ...updated[0]!, input_schema: { value: {}, truncated: true } }])).toBe(false);
});

it('seats prompt updates before their own Turn input and Tool updates before their exact publication', () => {
  const f = fixture(), input = traceRecord(0, { kind: 'user', request: null, message_id: 'user-a' });
  const placements = systemChangePlacements([input, f.after], entries);
  expect(placements.map(row => [row.kind, row.beforeMessageId])).toEqual([['prompt', 'user-a'], ['tools', 'message-4']]);
  f.after.request!.retry_number = 1;
  expect(systemChangePlacements([f.after], entries)[0]!.beforeMessageId).toBe('message-4');
  f.after.request!.assistant_message_id = 'unloaded';
  expect(systemChangePlacements([f.after], entries).every(row => !row.beforeMessageId)).toBe(true);
  f.after.request!.accepted_message_locator = { type: 'by_message_id', message_id: 'unloaded' };
  expect(systemChangePlacements([f.after], entries)).toEqual([]);
  f.after.request!.system_prompt.state = 'unchanged'; f.after.request!.tool_catalog = 'unchanged';
  expect(systemChangePlacements([f.after], entries)).toEqual([]);
});

it('uses the native Trace input owner when the Chat input has no Turn-process membership', () => {
  const f = fixture(), input = traceRecord(0, { kind: 'user', request: null, message_id: 'user-a' });
  const transcript = entries.map(entry => ({ ...entry, turn_process: undefined }));
  expect(systemChangePlacements([input, f.after], transcript).map(row => [row.kind, row.beforeMessageId])).toEqual([['prompt', 'user-a'], ['tools', 'message-4']]);
  expect(systemChangePlacements([f.after, input], transcript)[0].beforeMessageId).toBe('message-4');
  input.location = { attempt_id: 'another-owner' };
  expect(systemChangePlacements([input, f.after], transcript)[0].beforeMessageId).toBe('message-4');
});

it('covers an input before the Attempt start even when the page ends exactly at that start', async () => {
  const f = fixture(), input = traceRecord(0, { kind: 'user', request: null, message_id: 'user-a' });
  const read = f.reader.page;
  f.reader.page = async (...args) => args[1] === 'trace.records' ? args[2] === 0 ? tracePage(cut, [f.boundary, f.after], 1) : tracePage(cut, [input]) : read(...args);
  expect((await readSystemRequests(f.reader, cut, new Set([traceAttemptKey(turn)!]), () => true, new Set(['user-a']))).map(row => row.id)).toEqual(['trace:0', 'trace:4']);
});

it('anchors notices to deferred native input and publication descriptors without fabricating transcript rows', () => {
  const f = fixture(), input = traceRecord(0, { kind: 'user', request: null, message_id: 'user-a' });
  const descriptors: RuntimeClientMessageDescriptor[] = [
    { identity: 'user-a', role: 'user', kind: 'message', locator: { type: 'by_message_id', message_id: 'user-a' }, block_count: '1', tool_call_count: '0', tool_calls: [], content: [], collections: [] },
    { identity: 'message-4', role: 'assistant', kind: 'message', locator: { type: 'by_message_id', message_id: 'message-4' }, block_count: '1', tool_call_count: '0', tool_calls: [], turn_process: turn, content: [], collections: [] },
  ];
  expect(systemChangePlacements([input, f.after], [], undefined, descriptors).map(row => [row.kind, row.beforeMessageId])).toEqual([['prompt', 'user-a'], ['tools', 'message-4']]);
});

function Notices() {
  const feedback = useSystemChanges({ conversation_id: cut.conversation_id, attempt: null }, entries, undefined);
  return <>{[...feedback.messages.values()]}{[...feedback.attempts.values()]}</>;
}

it.each(['root', 'child', 'history'] as const)('%s Chat reads frozen fields in its own source and exposes complete prompt and Tool changes', async kind => {
  const f = fixture(), calls: { method: string; params: Record<string, unknown> }[] = [];
  const client = { request: async (request: typeof calls[number]) => {
    calls.push(request);
    const p = request.params;
    return request.method.endsWith('Page') ? { page: await f.reader.page(p.cut as CheckpointReadCut, p.domain as string, p.offset as number, () => true) }
      : { chunk: await f.reader.value(p.cut as CheckpointReadCut, p as never, p.offset as number, () => true) };
  } } as unknown as AppServerClient;
  const source = kind === 'history' ? { client, sessionId: 'A', cut: () => cut, owned: () => true }
    : { client, target: { session_id: 'A', attachment_id: 'attachment-a', conversation_id: cut.conversation_id, runtime_incarnation: 'runtime-a' }, agentId: kind === 'child' ? 'child-agent' : undefined, cut: () => cut, owned: () => true };
  render(<JournalSourceProvider source={source}><Notices/></JournalSourceProvider>);
  await screen.findByText('System Prompt Updated');
  await screen.findByText('· Added 1, Updated 1');
  expect(calls.some(call => call.params.domain === 'trace.records.4.detail.request.effective_system_prompt')).toBe(false);
  fireEvent.click(screen.getByRole('button', { name: 'System Prompt Updated' }));
  await screen.findByText(/PROMPT_END/);
  expect(document.querySelector('[data-system-prompt-body]')?.textContent).toBe(f.current.request!.effective_system_prompt.text);
  fireEvent.click(screen.getByRole('button', { name: /Tools Updated/ }));
  await screen.findByText('Added: search'); await screen.findByText('Updated: bash');
  expect(calls.every(call => call.method.startsWith(kind === 'history' ? 'session/trace' : 'conversation/checkpoint'))).toBe(true);
  if (kind === 'child') expect(calls.every(call => call.params.agent_id === 'child-agent')).toBe(true);
});

it('names an unavailable predecessor honestly and supports keyboard disclosures in Chinese', async () => {
  localeController.setLocale('zh');
  const f = fixture(); f.after.request!.system_prompt.state = 'previous_unavailable'; f.after.request!.tool_catalog = 'unchanged';
  const client = { request: async (request: { params: { cut: CheckpointReadCut; domain: string; offset: number } }) => ({ page: await f.reader.page(request.params.cut, request.params.domain, request.params.offset, () => true) }) } as unknown as AppServerClient;
  render(<JournalSourceProvider source={{ client, sessionId: 'A', cut: () => cut, owned: () => true }}><Notices/></JournalSourceProvider>);
  const notice = await screen.findByRole('button', { name: translator('zh')('trajectory:copy.previous-system-prompt-unavailable') });
  expect(notice.getAttribute('aria-expanded')).toBe('false');
  fireEvent.keyDown(notice, { key: 'Enter' });
  await waitFor(() => expect(notice.getAttribute('aria-expanded')).toBe('true'));
});

it('discards a late Trace page when the selected Conversation source is replaced', async () => {
  const f = fixture();
  let resolve!: (value: { page: CheckpointPage }) => void;
  const pending = new Promise<{ page: CheckpointPage }>(done => { resolve = done; });
  const read = vi.fn(() => pending), oldClient = { request: read } as unknown as AppServerClient;
  let oldOwned = true;
  const oldSource = { client: oldClient, sessionId: 'A', cut: () => cut, owned: () => oldOwned };
  const nextCut = { ...cut, conversation_id: 'conversation-B' };
  const nextClient = { request: async () => ({ page: tracePage(nextCut, []) }) } as unknown as AppServerClient;
  const nextSource = { client: nextClient, sessionId: 'B', cut: () => nextCut, owned: () => true };
  const ui = render(<JournalSourceProvider source={oldSource}><Notices/></JournalSourceProvider>);
  await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  oldOwned = false; ui.rerender(<JournalSourceProvider source={nextSource}><Notices/></JournalSourceProvider>);
  await act(async () => { resolve({ page: tracePage(cut, [f.after]) }); await pending; });
  expect(ui.container.querySelector('[data-system-change]')).toBeNull();
});

it.each(['hidden', 'returned'] as const)('restarts system notices at the same Trace revision when the retired read settles while %s', async settlement => {
  const f = fixture(); f.after.request!.tool_catalog = 'unchanged';
  const stale = structuredClone(f.after); stale.request!.system_prompt.state = 'initial';
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  const guards: (() => boolean)[] = [];
  const request = vi.fn(async (_request: unknown, _expected: unknown, _acknowledged: unknown, current: () => boolean) => {
    guards.push(current);
    if (guards.length === 1) { await pending; return { page: tracePage(cut, [f.boundary, stale]) }; }
    return { page: tracePage(cut, [f.boundary, f.after]) };
  });
  const client = { request } as unknown as AppServerClient;
  let active = true;
  const source = { client, sessionId: 'A', cut: () => cut, owned: () => active };
  const view = () => <JournalSourceProvider source={source}><Notices/></JournalSourceProvider>;
  const ui = render(view());
  await waitFor(() => expect(request).toHaveBeenCalledTimes(1));
  active = false; ui.rerender(view());
  if (settlement === 'hidden') await act(async () => { release(); await pending; });
  expect(ui.container.querySelector('[data-system-change]')).toBeNull();
  active = true; ui.rerender(view());
  await screen.findByRole('button', { name: 'System Prompt Updated' });
  expect(request).toHaveBeenCalledTimes(2);
  expect(guards[0]()).toBe(false);
  if (settlement === 'returned') await act(async () => { release(); await pending; });
  expect(screen.queryByRole('button', { name: 'System Prompt' })).toBeNull();
  expect(screen.getByRole('button', { name: 'System Prompt Updated' })).toBeTruthy();
});

it.each(['hidden', 'returned'] as const)('restarts an expanded system prompt at the same Trace revision when its retired read settles while %s', async settlement => {
  const f = fixture(); f.after.request!.tool_catalog = 'unchanged';
  const promptDomain = 'trace.records.4.detail.request.effective_system_prompt';
  let release!: () => void;
  const pending = new Promise<void>(resolve => { release = resolve; });
  const guards: (() => boolean)[] = [];
  const client = { request: async (request: { method: string; params: { cut: CheckpointReadCut; domain: string; offset: number } }, _expected: unknown, _acknowledged: unknown, current: () => boolean) => {
    const p = request.params;
    if (!request.method.endsWith('Page')) return { chunk: await f.reader.value(p.cut, p as never, p.offset, () => true) };
    const page = await f.reader.page(p.cut, p.domain, p.offset, () => true);
    if (p.domain === promptDomain) {
      guards.push(current);
      if (guards.length === 1) await pending;
    }
    return { page };
  } } as unknown as AppServerClient;
  let active = true;
  const source = { client, sessionId: 'A', cut: () => cut, owned: () => active };
  const view = () => <JournalSourceProvider source={source}><Notices/></JournalSourceProvider>;
  const ui = render(view());
  fireEvent.click(await screen.findByRole('button', { name: 'System Prompt Updated' }));
  await waitFor(() => expect(guards).toHaveLength(1));
  active = false; ui.rerender(view());
  if (settlement === 'hidden') await act(async () => { release(); await pending; });
  expect(screen.queryByText(/PROMPT_END/)).toBeNull();
  active = true; ui.rerender(view());
  await screen.findByText(/PROMPT_END/);
  expect(guards).toHaveLength(2);
  expect(guards[0]()).toBe(false);
  if (settlement === 'returned') await act(async () => { release(); await pending; });
  expect(screen.getByRole('button', { name: 'System Prompt Updated' }).getAttribute('aria-expanded')).toBe('true');
  expect(document.querySelector('[data-system-prompt-body]')?.textContent).toBe(f.current.request!.effective_system_prompt.text);
});
