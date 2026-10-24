import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { ContentChunk, ContentRef, ForegroundToolExecution } from '../../protocol/app-server/v57';
import { Content } from '../src/app/agent/Message';
import { Tool } from '../src/app/agent/Tool';
import { JournalSourceProvider } from '../src/app/agent/ObservationSource';
import { nativeTool } from './native-tool-cases';
import { checkpoint, readCut, Server, snapshot } from './fixture';

afterEach(() => { cleanup(); vi.restoreAllMocks(); });
const original = nativeTool('tool-write', 'write', { path: 'report.md', content: '# Native write report\n\n保留水🙂原文' }, []);
const deferred: ForegroundToolExecution = { ...original, state: { ...original.state, arguments: '' } };
const bytes = new TextEncoder().encode(original.state.arguments);
const reference: ContentRef = { conversation_id: 'conversation-A', source: { type: 'canonical_message_block', component: 'payload', locator: original.occurrence.locator, block_index: 0 }, representation: 'raw_json_utf8', version: 1, content_revision: 1, readable_end: bytes.length };
const chunk = (offset: number): ContentChunk => {
  const end = Math.min(bytes.length, offset + 31), data = bytes.slice(offset, end);
  return { content_ref: reference, offset, end_offset: end, readable_end: bytes.length, data_base64: btoa(Array.from(data, b => String.fromCharCode(b)).join('')), next_offset: end === bytes.length ? null : end, complete: end === bytes.length };
};
async function fixture() {
  const server = new Server(); await server.attached('A');
  const cut = { ...checkpoint(snapshot()).read_cut, source_cut: { read: readCut(), ledger_through: '0', transcript_total: '0', surface_revision: '0', inherited_through: '0' } };
  const page = (domain: string) => ({ type: 'checkpoint_page' as const, page: { read_cut: cut, cursor: cut.revision, domain, total: '1', items: [{ offset: '0', identity: 'native-write:0', value: deferred as never, deferred: true, content: [{ path: ['state', 'arguments'], content_ref: reference, deferred: true }] }], collections: [], next_offset: null } });
  server.handlers.set('conversation/checkpointPage', request => { if (request.method !== 'conversation/checkpointPage') throw Error(); return page(request.params.domain); });
  const read = vi.spyOn(server.client.reads, 'readContent').mockImplementation(async (_target, _agent, _ref, offset) => chunk(offset));
  const source = { client: server.client, target: server.target('A'), cut: () => cut, owned: () => true };
  return { server, source, read, cut, page };
}
it('reads deferred write arguments only on expansion, preserves exact content and settlement, and caches reopening', async () => {
  const { server, source, read } = await fixture();
  try {
    const frozen = JSON.stringify(deferred);
    const ui = render(<JournalSourceProvider source={source}><Tool tool={deferred}/></JournalSourceProvider>);
    expect(read).not.toHaveBeenCalled();
    fireEvent.click(ui.getByRole('button', { expanded: false }));
    expect(await ui.findByText('# Native write report')).toBeTruthy();
    expect(ui.getByText('保留水🙂原文')).toBeTruthy();
    expect(ui.container.querySelector('[data-tool-renderer="write"] [data-state]')?.getAttribute('data-state')).toBe('ok');
    const requests = read.mock.calls.length;
    expect(requests).toBeGreaterThan(1);
    fireEvent.click(ui.getByRole('button', { expanded: true }));
    fireEvent.click(ui.getByRole('button', { expanded: false }));
    expect(read).toHaveBeenCalledTimes(requests);
    expect(JSON.stringify(deferred)).toBe(frozen);
  } finally { server.client.disconnect(); }
});
it('an accepted canonical call reads its omitted native descriptor instead of claiming it is assembling', async () => {
  const { server, source } = await fixture();
  try {
    const ui = render(<JournalSourceProvider source={source}><Content messageId={original.message_id!} blocks={[{ type: 'tool_call', id: original.call_id!, tool_id: 'tool-write', name: 'write', arguments: {} }]}/></JournalSourceProvider>);
    expect(ui.queryByText(/Assembling/)).toBeNull();
    const trigger = await ui.findByRole('button', { expanded: false });
    fireEvent.click(trigger);
    expect(await ui.findByText('# Native write report')).toBeTruthy();
    expect(server.requests.filter(item => item.request.method === 'conversation/checkpointPage')).toHaveLength(2);
  } finally { server.client.disconnect(); }
});
it('optional inline ids never hide an exact canonical block occurrence', () => {
  const ui = render(<Content messageId={original.message_id!} tools={[{ ...original, call_id: null, tool_id: null }]} blocks={[{ type: 'tool_call', id: original.call_id!, tool_id: 'tool-write', name: 'write', arguments: {} }]}/>);
  expect(ui.queryByText(/Assembling/)).toBeNull();
  expect(ui.getByRole('button', { expanded: false })).toBeTruthy();
});
it('a physically late argument read cannot enter a replacement source', async () => {
  const { server, source, read } = await fixture();
  try {
    let finish!: (chunk: ContentChunk) => void;
    read.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const ui = render(<JournalSourceProvider source={source}><Tool tool={deferred}/></JournalSourceProvider>);
    fireEvent.click(ui.getByRole('button', { expanded: false }));
    await waitFor(() => expect(read).toHaveBeenCalledOnce());
    ui.rerender(<JournalSourceProvider source={undefined}><Tool tool={original}/></JournalSourceProvider>);
    await act(async () => finish(chunk(0)));
    expect(ui.queryByRole('alert')).toBeNull();
    expect(ui.getByText('# Native write report')).toBeTruthy();
    expect(read).toHaveBeenCalledOnce();
  } finally { server.client.disconnect(); }
});


it.each(['root','child'] as const)('a frozen %s descriptor stays at C0 after live C1 advances; argument hydration cannot upgrade its lifecycle', async mode => {
  const { server, source, cut, page } = await fixture();
  try {
    const frozen = { ...deferred, state: { type: 'running' as const, arguments: '' } };
    server.handlers.set('session/historyPage', request => {
      if (request.method !== 'session/historyPage') throw Error();
      expect(request.params.cut).toEqual(cut.source_cut);
      const result = page(request.params.domain);
      result.page.items[0].value = frozen as never;
      return result;
    });
    const current = { ...source, agentId: mode === 'child' ? 'worker' : undefined, cut: () => ({ ...cut, activation_id: mode === 'child' ? 'worker-activation' : undefined, revision: '999', through: '999', source_cut: { ...cut.source_cut!, read: { ...cut.source_cut!.read, journal: '999' } } }) };
    server.handlers.set('conversation/checkpointPage', request => {
      if (request.method !== 'conversation/checkpointPage') throw Error();
      const result = page(request.params.domain);
      result.page.read_cut = request.params.cut as typeof cut;
      result.page.items[0].value = frozen as never;
      return result;
    });
    const ui = render(<JournalSourceProvider historical source={current} history={{ source_cut: cut.source_cut, entries: [] }}><Content messageId={original.message_id!} blocks={[{ type: 'tool_call', id: original.call_id!, tool_id: 'tool-write', name: 'write', arguments: {} }]}/></JournalSourceProvider>);
    fireEvent.click(await ui.findByRole('button', { expanded: false }));
    expect(await ui.findByText('# Native write report')).toBeTruthy();
    expect(ui.container.querySelector('[data-tool-renderer="write"] [data-state]')?.getAttribute('data-state')).toBe('running');
    const requests = server.requests.filter(({request})=>request.method === (mode === 'child' ? 'conversation/checkpointPage' : 'session/historyPage'));
    expect(requests[0].request.params).toMatchObject(mode === 'child' ? {agent_id:'worker',cut:{activation_id:'worker-activation',revision:'0',source_cut:cut.source_cut}} : {cut:cut.source_cut});
  } finally { server.client.disconnect(); }
});

it('a deferred live descriptor refreshes J state at the new native cut without closing its disclosure', async () => {
  const { server, source, cut, page } = await fixture();
  try {
    let latest = cut;
    const live = { ...source, cut: () => latest };
    server.handlers.set('conversation/checkpointPage', request => {
      if (request.method !== 'conversation/checkpointPage') throw Error();
      const result = page(request.params.domain);
      result.page.read_cut = request.params.cut as typeof cut;
      result.page.items[0].value = { ...original, state: latest.revision === cut.revision ? { type: 'running', arguments: original.state.arguments } : original.state } as never;
      return result;
    });
    const subject = () => <JournalSourceProvider source={live}><Content messageId={original.message_id!} blocks={[{ type: 'tool_call', id: original.call_id!, tool_id: 'tool-write', name: 'write', arguments: {} }]}/></JournalSourceProvider>;
    const ui = render(subject());
    fireEvent.click(await ui.findByRole('button', { expanded: false }));
    expect(ui.getByText('# Native write report')).toBeTruthy();
    expect(ui.container.querySelector('[data-state]')?.getAttribute('data-state')).toBe('running');
    latest = { ...cut, revision: '1' };
    ui.rerender(subject());
    await waitFor(() => expect(ui.container.querySelector('[data-state]')?.getAttribute('data-state')).toBe('ok'));
    expect(ui.getByRole('button', { expanded: true })).toBeTruthy();
    expect(ui.getByText('# Native write report')).toBeTruthy();
  } finally { server.client.disconnect(); }
});
