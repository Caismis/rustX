import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { CheckpointPage, CheckpointReadCut, TraceContextPresentation } from '../../protocol/app-server/v57';
import { useTraceContexts } from '../src/app/trajectory/useTraceContexts';
import type { TraceCache, TraceFieldReader } from '../src/client/trace';
import { traceRecord } from './trace-fixture';
afterEach(cleanup);
const cut: CheckpointReadCut = { conversation_id: 'original', revision: '4', through: '4', domain_revisions: {}, collection_cuts: {} };
const row = (index: number): TraceContextPresentation => ({ locator: { type: 'by_ledger_position', position: index + 1 }, message_id: null, fields: [], producer: { Native: 'workspace_instructions' }, context_kind: 'native_environment', source: { type: 'runtime' }, preview: { text: `original context ${index}`, truncated: false }, attachments: [], truncated: false });
const page = (): CheckpointPage => ({ cursor: '4', read_cut: cut, domain: 'trace.records.1.context', total: '3', items: [1, 2].map(index => ({ offset: String(index), identity: `context ${index}`, value: row(index), content: [], deferred: false })), next_offset: null, collections: [] });
const cache = (pageRead: TraceFieldReader['page']): TraceCache => {
 const record = traceRecord(1); record.request = { ...record.request!, context_domain: 'trace.records.1.context', context_total: '3', context_additions: [row(0)], context_next_offset: 1 };
 return { epoch: 1, details: {}, readCuts: { [record.id]: cut }, fields: { page: pageRead, value: async () => { throw Error('No whole Context read'); }, content: async () => { throw Error('No whole Context read'); } }, page: { records: [record] } };
};
it('native Context continuation adds ordinary ledger rows at the exact original cut', async () => {
 const read = vi.fn(async () => page()), value = cache(read);
 const ui = renderHook(() => useTraceContexts(value, 'trace:1'));
 await waitFor(() => expect(ui.result.current.records[0].request?.context_additions.map(row => row.preview?.text)).toEqual(['original context 0', 'original context 1', 'original context 2']));
 expect(read.mock.calls[0]).toEqual([cut, 'trace.records.1.context', 1, expect.any(Function)]);
});
it('late Context pages cannot repaint a selected replacement cut', async () => {
 let release!: (page: CheckpointPage) => void;
 const read = vi.fn(() => new Promise<CheckpointPage>(resolve => { release = resolve; })), value = cache(read);
 const ui = renderHook(({ value }) => useTraceContexts(value, 'trace:1'), { initialProps: { value } }); const finish = release;
 ui.rerender({ value: { ...value, readCuts: { 'trace:1': { ...cut, revision: '5' } } } });
 await act(async () => finish(page())); expect(ui.result.current.records[0].request?.context_additions).toHaveLength(1); expect(read).toHaveBeenCalledTimes(2);
});
it('oversized Context pages fail before visiting their selected values', async () => {
 let visits = 0;
 const oversized = { ...page(), items: Array.from({ length: 33 }, () => ({ offset: '1', identity: 'untrusted', get value() { visits++; throw Error('Do not visit'); }, content: [], deferred: false })) };
 const ui = renderHook(() => useTraceContexts(cache(async () => oversized), 'trace:1'));
 await waitFor(() => expect(ui.result.current.error).toContain('Original Context page changed')); expect(visits).toBe(0);
});
