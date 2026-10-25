import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { TrajectoryInspector, type TrajectoryInspectorProps } from '../src/app/trajectory/TrajectoryInspector';
import { parseJson } from '../src/presentation/primitives/json';
import { readTraceBlocks, readTraceCollection, readTraceValue } from '../src/client/trace-content';
import { nativeTraceFixture } from './native-trace-fixture';
import { traceRecord, traceTool, requestDetail, toolDetail } from './trace-fixture';
import type { TraceDetail, TraceRecord } from '../../protocol/app-server/v57';
afterEach(cleanup);
const props = (record: TraceRecord): TrajectoryInspectorProps => ({ item: { type: 'RecordRow', record, display_key: record.id, owner_record_id: record.id, facet: 'overview', label: '', preview: '' }, facet: 'overview', onFacet: vi.fn(), onLoadDetail: vi.fn(), onClose: vi.fn(), records: [record], executions: new Map(), requestNumbers: new Map(), completeHistory: true, onOpen: vi.fn() });
const assistant = () => {
  const record = traceRecord(4, { kind: 'assistant', request: null, message_id: 'assistant-4', preview: { text: 'Inspect the **workspace**.', truncated: true } });
  const call = { type: 'tool_call' as const, call_id: 'original-call', tool_id: 'tool-bash', name: 'bash', arguments: { value: { command: 'git status' }, truncated: false } };
  record.calls = [{ occurrence: { locator: { type: 'by_message_id', message_id: 'assistant-4' }, block_index: 2 }, fields: [], call_id: call.call_id, tool_id: call.tool_id, name: call.name }];
  const detail: TraceDetail = { id: record.id, kind: record.kind, projected_messages: [], messages: [{ message_id: 'assistant-4', role: 'assistant', truncated: false, blocks: [{ type: 'reasoning', text: { text: 'Plan **the review** carefully.', truncated: false } }, { type: 'text', text: { text: 'Inspect the **workspace**.', truncated: false } }, call] }], truncated: false };
  return { record, detail };
};
function normalize(node: Element) {
  const copy = node.cloneNode(true) as Element;
  for (const element of [copy, ...copy.querySelectorAll('*')]) for (const name of ['id', 'aria-controls', 'aria-labelledby']) element.removeAttribute(name);
  return copy.innerHTML;
}
it.each(['overview', 'rendered', 'raw'] as const)('native assistant %s uses the exact established semantic markup', async facet => {
  const { record, detail } = assistant(), native = nativeTraceFixture(record, detail), base = { ...props(record), facet };
  const ui = render(<TrajectoryInspector {...base} detail={detail}/>);
  const reference = normalize(screen.getByRole('tabpanel')); ui.unmount();
  render(<TrajectoryInspector {...base} detailPage={native.page} detailReader={native.reader}/>);
  await waitFor(() => expect(normalize(screen.getByRole('tabpanel'))).toBe(reference));
  expect(screen.queryByRole('combobox')).toBeNull(); expect(screen.queryByText('assistant-4')).toBeNull(); expect(screen.queryByText('original-call')).toBeNull();
  expect(screen.getByRole('tabpanel').textContent!.split('git status')).toHaveLength(2);
  if (facet !== 'raw') { const thinking = screen.getByRole('button', { name: 'Thinking' }); fireEvent.click(thinking); expect(thinking.getAttribute('aria-expanded')).toBe('false'); expect(screen.queryByText('carefully.', { exact: false })).toBeNull(); }
});
it.each(['input', 'output', 'schema'] as const)('native Tool %s reuses the styled source, result and Schema presentations', async facet => {
  const record = traceTool(5), detail = toolDetail(5), native = nativeTraceFixture(record, detail), base = { ...props(record), facet };
  // The source contract, arguments and program agree, as they do on the native wire.
  detail.tool!.source!.text.text = (detail.tool!.arguments!.value as { command: string }).command;
  const ui = render(<TrajectoryInspector {...base} detail={detail}/>); const reference = normalize(screen.getByRole('tabpanel')); ui.unmount();
  render(<TrajectoryInspector {...base} detailPage={native.page} detailReader={native.reader}/>);
  await waitFor(() => expect(normalize(screen.getByRole('tabpanel'))).toBe(reference)); expect(screen.queryByRole('combobox')).toBeNull();
});
it('native Request options and prompt use the original JSON tree and Markdown', async () => {
  const record = traceRecord(3), detail = requestDetail(3), native = nativeTraceFixture(record, detail);
  const base = { ...props(record), item: { type: 'RequestBoundary' as const, record, display_key: record.id, owner_record_id: record.id, facet: 'overview' as const, label: '', preview: '' } };
  const reference = render(<TrajectoryInspector {...base} facet="options" detail={detail}/>); const html = normalize(screen.getByRole('tabpanel')); reference.unmount();
  const ui = render(<TrajectoryInspector {...base} facet="options" detailPage={native.page} detailReader={native.reader}/>);
  await waitFor(() => expect(normalize(screen.getByRole('tabpanel'))).toBe(html)); expect(screen.queryByRole('combobox')).toBeNull();
  ui.rerender(<TrajectoryInspector {...base} item={{...base.item,type:"SystemPromptCell",facet:"system-prompt",label:"Initial System Prompt"}} facet="system-prompt" detailPage={native.page} detailReader={native.reader}/>);
  await screen.findByText(detail.request!.effective_system_prompt.text); expect(screen.queryByText('truncated')).toBeNull();
});
it('original JSON numbers keep exact spelling in the tree data and clipboard serialization', () => {
  const raw = '{"integer":18446744073709551615,"decimal":1.0000,"exponent":1e400}';
  expect(JSON.stringify(parseJson(raw))).toBe(raw);
});
it('a selected native page cannot publish its late blocks into a replacement event', async () => {
  const { record, detail } = assistant(), native = nativeTraceFixture(record, detail);
  let finish!: (page: Awaited<ReturnType<typeof native.reader.page>>) => void;
  const page = native.reader.page;
  native.reader.page = vi.fn(() => new Promise<Awaited<ReturnType<typeof native.reader.page>>>(resolve => { finish = resolve; }));
  const ui = render(<TrajectoryInspector {...props(record)} facet="rendered" detailPage={native.page} detailReader={native.reader}/>);
  await waitFor(() => expect(native.reader.page).toHaveBeenCalled());
  const replacement = traceRecord(8, { kind: 'user', request: null });
  ui.rerender(<TrajectoryInspector {...props(replacement)} facet="rendered" detail={requestDetail(8, { kind: 'user', request: null, messages: [] })}/>);
  await act(async () => finish(await page(native.page.read_cut, 'messages.ledger:100.blocks', 0, () => true)));
  expect(screen.queryByText(/Plan the review/)).toBeNull(); expect(screen.queryByRole('button', { name: 'Thinking' })).toBeNull();
});
it.each(['cut', 'cursor', 'offset', 'total', 'empty'] as const)('rejects a changed native block page %s before reading content', async fault => {
  const { record, detail } = assistant(), native = nativeTraceFixture(record, detail), original = native.reader.page;
  native.reader.page = async (...args) => { const page = await original(...args); if (fault === 'cut') page.read_cut = { ...page.read_cut, revision: '999' }; if (fault === 'cursor') page.cursor = '999'; if (fault === 'offset') page.items[0].offset = '1'; if (fault === 'total') page.total = '900'; if (fault === 'empty') page.items = []; return page; };
  const read = vi.spyOn(native.reader, 'content');
  await expect(readTraceBlocks(native.reader, native.page.read_cut, [{ domain: 'messages.ledger:100.blocks', total: '3', revision: '1', loaded: '0' }], () => true)).rejects.toThrow(); expect(read).not.toHaveBeenCalled();
});
it('sparse native Tool index coordinates stay intact at the read boundary without being rendered twice', async () => {
  const { record, detail } = assistant(), native = nativeTraceFixture(record, detail), cut = native.page.read_cut, offsets: number[] = [];
  native.reader.page = async (_cut, domain, offset) => { offsets.push(offset); const at = offset === 0 ? 2 : offset; return { read_cut: cut, cursor: cut.revision, domain, total: '3', next_offset: at === 2 ? '7' : at === 7 ? '11' : null, collections: [], items: [{ offset: String(at), identity: 'call', value: null, content: [], deferred: true }] }; };
  const items = await readTraceCollection(native.reader, cut, { domain: 'messages.ledger:100.tools', total: '3' }, () => true);
  expect(items.map(item => item.offset)).toEqual(['2', '7', '11']); expect(offsets).toEqual([0, 7, 11]);
});
it('original field chunks retain their declared extent and UTF8 boundaries', async () => {
  const { record, detail } = assistant(), native = nativeTraceFixture(record, detail), cut = native.page.read_cut;
  const raw = '"' + 'x'.repeat(65534) + '🙂"', bytes = new TextEncoder().encode(raw), offsets: number[] = [];
  native.reader.value = async (cut, field, offset) => { offsets.push(offset); const end = Math.min(offset + 65536, bytes.length); return { cut, ...field, offset, readable_end: bytes.length, next_offset: end < bytes.length ? end : null, representation: 'canonical_json_v1', bytes_base64: btoa(String.fromCharCode(...bytes.slice(offset, end))) }; };
  expect((await readTraceValue(native.reader, cut, { domain: 'source', item_offset: 0, path: [] }, () => true)).text).toBe(raw); expect(offsets).toEqual([0, 65536]);
});

it('deferred call identities still navigate by the original native block occurrence', async () => {
 const {record,detail}=assistant();record.calls[0].call_id=null;record.calls[0].name=null;
 const tool=traceTool(5);tool.tool!.occurrence={locator:{type:'by_ledger_position',position:100},block_index:2};
 const native=nativeTraceFixture(record,detail),onOpen=vi.fn();
 render(<TrajectoryInspector {...props(record)} facet="rendered" records={[record,tool]} onOpen={onOpen} detailPage={native.page} detailReader={native.reader}/>);
 fireEvent.click(await screen.findByRole('button',{name:/bash.*git status/}));expect(onOpen).toHaveBeenCalledWith('trace:5');
});

it('a native failure renders its complete human-readable error instead of result transport metadata', async () => {
 const record=traceTool(5),detail=toolDetail(5),native=nativeTraceFixture(record,detail),message='Original failure beyond its preview. '.repeat(100);
 const head=native.page.items[0].value as {record:TraceRecord};head.record.tool!.outcome='failed';head.record.fields.push({type:'value',field:'result',domain:'native.result',item_offset:0,path:[],encoded_bytes:String(JSON.stringify({status:{type:'failed',error:message}}).length)});
 native.values.set(JSON.stringify(['native.result',0,[]]),JSON.stringify({status:{type:'failed',error:message}}));
 render(<TrajectoryInspector {...props(record)} facet="output" detailPage={native.page} detailReader={native.reader}/>);
 await waitFor(()=>expect([...document.querySelectorAll('pre')].some(node=>node.textContent===message)).toBe(true));expect(document.body.textContent).not.toContain('tool_call_id');
});

it('deferred Tool aliases fill the normal title and retain the native code contract',async()=>{
 const record=traceTool(5),detail=toolDetail(5),native=nativeTraceFixture(record,detail),head=native.page.items[0].value as {record:TraceRecord},name='Original name '.repeat(80).trim();
 head.record.tool!.name=null;head.record.tool!.tool_id=null;
 for(const [field,value] of [['name',name],['tool_id','tool-bash']] as const){const domain=`native.${field}`,raw=JSON.stringify(value);head.record.fields.push({type:'value',field,domain,item_offset:0,path:[],encoded_bytes:String(raw.length)});native.values.set(JSON.stringify([domain,0,[]]),raw);}
 render(<TrajectoryInspector {...props(record)} facet="input" detailPage={native.page} detailReader={native.reader}/>);
 await screen.findByTitle(name);expect(screen.getByRole('tab',{name:'Code'})).toBeTruthy();expect(screen.queryByRole('combobox')).toBeNull();
});
