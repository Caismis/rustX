import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { CheckpointPage, CheckpointReadCut, TraceInspectionCompactionCall, TraceInspectionCompactionCalls } from '../../protocol/app-server/v82';
import { CompactionInspection } from '../src/app/trajectory/CompactionInspection';
import { compactionPageOffset, compactionRequestsPage } from '../src/client/compaction-inspection';
import type { TraceFieldReader } from '../src/client/trace';
import { checkpoint, snapshot } from './fixture';

afterEach(cleanup);
const cut: CheckpointReadCut = { ...checkpoint(snapshot()).read_cut, through: '100' };
const domain = 'trace.records.10.detail.calls';
const call = (id: string, started = '10'): TraceInspectionCompactionCall => ({ call_id: id, started_sequence: started,
  outcome: 'completed', settled_sequence: '90', strategy: 'native', request_count: '1', requests_domain: `${domain}.${id}.requests` });
const calls = (items: TraceInspectionCompactionCall[], total = String(items.length), next?: string): TraceInspectionCompactionCalls => ({ domain, total, items, next_offset: next });
function page(cut: CheckpointReadCut, domain: string, values: { id: string; offset: string; value: unknown }[], total = String(values.length), next?: string): CheckpointPage {
  return { read_cut: cut, cursor: cut.revision, domain, total, next_offset: next, collections: [],
    items: values.map(item => ({ identity: item.id, offset: item.offset, value: item.value, content: [], deferred: false, body_complete: true })) };
}
function request(selected: TraceInspectionCompactionCall, provider = 'historical-provider') {
  return { call_id: selected.call_id, ordinal: 0, request_id: `request-${selected.call_id}`, model: 'wire-model',
    invocation: { provider, model_ref: 'logical/summary', profile: 'precise' }, local_terminal: 'completed',
    usage: { input_tokens: '18446744073709551615', output_tokens: '0', details: {} }, request_evidence: null,
    ignored_private_body: { system_prompt: 'TRUSTED INPUT MUST NOT DISPLAY', reasoning: 'PRIVATE REASONING' } };
}
const requestPage = (cut: CheckpointReadCut, selected: TraceInspectionCompactionCall, provider?: string) => page(cut, selected.requests_domain,
  [{ id: `request-${selected.call_id}`, offset: '0', value: request(selected, provider) }]);
function reader(scope: string, read: TraceFieldReader['page']): TraceFieldReader {
  return { scope, page: read, value: async () => { throw Error('No original value read'); }, content: async () => { throw Error('No original content read'); } };
}
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { resolve, promise }; }

it('shows actual frozen binding and exact known usage while preserving unknown measurements', async () => {
  const selected = call('A');
  const fields = reader('scope', vi.fn(async () => requestPage(cut, selected)));
  const view = render(<CompactionInspection initial={calls([selected])} cut={cut} reader={fields} accepted={21}/>);
  expect(await screen.findByText('historical-provider')).toBeTruthy();
  expect(screen.getByText('logical/summary')).toBeTruthy();
  expect(screen.getByText('precise')).toBeTruthy();
  expect(screen.getByText('wire-model')).toBeTruthy();
  expect(screen.getByText('18,446,744,073,709,551,615')).toBeTruthy();
  expect(view.container.textContent).not.toMatch(/TRUSTED INPUT|PRIVATE REASONING/);
  expect(view.container.textContent).toContain('Usage not reported');
  const decoded = compactionRequestsPage(requestPage(cut, selected), cut, selected, '0');
  expect(decoded.items[0].output).toBe('0');
  expect(decoded.items[0].reasoning).toBeUndefined();
  expect(() => compactionPageOffset('9007199254740993')).toThrow(/supported page range/);
  expect(() => compactionRequestsPage(requestPage(cut, selected), cut, call('foreign'), '0')).toThrow();
});

it('retries a failed physical request read for the selected Call', async () => {
  const selected = call('A');
  const read = vi.fn(async () => requestPage(cut, selected)).mockRejectedValueOnce(new Error('read failed'));
  render(<CompactionInspection initial={calls([selected])} cut={cut} reader={reader('scope', read)}/>);
  expect((await screen.findByRole('alert')).textContent).toContain('read failed');
  fireEvent.click(screen.getByRole('button', { name: 'Read again' }));
  expect(await screen.findByText('historical-provider')).toBeTruthy();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(read).toHaveBeenCalledTimes(2);
});

it('retains a selected Call beyond the first page when the native cut advances', async () => {
  const a = call('A'), b = call('B', '20');
  const first = reader('same-source', vi.fn(async (_cut, requested) => requested === domain
    ? page(cut, domain, [{ id: b.call_id, offset: b.started_sequence, value: b }], '2')
    : requestPage(cut, requested === a.requests_domain ? a : b, requested === a.requests_domain ? 'provider-A' : 'provider-B')));
  const view = render(<CompactionInspection initial={calls([a], '2', '10')} cut={cut} reader={first}/>);
  expect(await screen.findByText('provider-A')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Load more Calls' }));
  await waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(2));
  fireEvent.change(screen.getByRole('combobox'), { target: { value: 'B' } });
  expect(await screen.findByText('provider-B')).toBeTruthy();
  const later = { ...cut, revision: '2', through: '101' };
  const read = vi.fn(async (_cut: CheckpointReadCut, requested: string) => requested === domain
    ? page(later, domain, [{ id: b.call_id, offset: b.started_sequence, value: b }], '2')
    : requestPage(later, b, 'provider-B-new'));
  view.rerender(<CompactionInspection initial={calls([a], '2', '10')} cut={later} reader={reader('same-source', read)}/>);
  expect(await screen.findByText('provider-B-new')).toBeTruthy();
  expect((screen.getByRole('combobox') as HTMLSelectElement).value).toBe('B');
  expect(read.mock.calls.some(args => args[1] === domain)).toBe(true);
});

it('keeps resident values during same-source refresh and rejects a late response from a replaced source', async () => {
  const selected = call('A');
  const first = reader('scope-one', vi.fn(async () => requestPage(cut, selected, 'original-provider')));
  const view = render(<CompactionInspection initial={calls([selected])} cut={cut} reader={first}/>);
  expect(await screen.findByText('original-provider')).toBeTruthy();
  const later = { ...cut, revision: '2', through: '101' }, pending = deferred<CheckpointPage>();
  view.rerender(<CompactionInspection initial={calls([selected])} cut={later} reader={reader('scope-one', vi.fn(() => pending.promise))}/>);
  expect(screen.getByText('original-provider')).toBeTruthy();
  expect(screen.getByText(/previously read values remain visible/)).toBeTruthy();
  const replaced = reader('scope-two', vi.fn(async () => requestPage(later, selected, 'replacement-provider')));
  view.rerender(<CompactionInspection initial={calls([selected])} cut={later} reader={replaced}/>);
  expect(screen.queryByText('original-provider')).toBeNull();
  expect(await screen.findByText('replacement-provider')).toBeTruthy();
  await act(async () => pending.resolve(requestPage(later, selected, 'LATE OLD PROVIDER')));
  expect(screen.queryByText('LATE OLD PROVIDER')).toBeNull();
  expect(screen.getByText('replacement-provider')).toBeTruthy();
});
