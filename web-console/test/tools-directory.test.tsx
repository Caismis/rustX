import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CommandPanel } from '../src/app/commands/CommandPanel';
import { domainReceipt, Server, snapshot } from './fixture';

let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); });
async function setup() { server = new Server(); await server.connect(); await server.client.attach('A'); }
function panel(current = () => true) {
  const succeeded = vi.fn();
  render(<CommandPanel request={{ id: 'tools' }} client={server.client} sessionId="A" current={current} close={() => {}} succeeded={succeeded} navigate={() => {}}/>);
  return succeeded;
}
it('shows an inline complete tool directory without supplementary reads', async () => {
  await setup(); server.handlers.set('resources/read', () => domainReceipt('capabilities', [{ configured_tools: [{ name: 'complete-tool' }], revision: '0' }]));
  panel(); expect(await screen.findByText(/complete-tool/)).toBeTruthy();
  expect(server.requests.some(row => row.request.method === 'conversation/checkpointPage')).toBe(false);
});
it.each([false, true])('automatically completes a bounded directory with deferred header=%s and deferred tool bodies', async deferred => {
  await setup();
  const receipt = domainReceipt('capabilities', [deferred ? null : { revision: '0', configured_tools: [{ name: 'first-tool' }] }]);
  receipt.page.items[0]!.deferred = deferred; receipt.page.items[0]!.body_complete = false;
  receipt.page.collections = [{ domain: 'capabilities.configured_tools', revision: '0', total: '2', loaded: deferred ? '0' : '1', next_offset: deferred ? '0' : '1' }];
  server.handlers.set('resources/read', () => receipt);
  server.handlers.set('conversation/checkpointPage', request => {
    if (request.method !== 'conversation/checkpointPage') throw Error('Wrong request');
    expect(request.params.cut).toEqual(receipt.page.read_cut);
    const page = { ...receipt.page, domain: request.params.domain, total: '2', collections: [], next_offset: request.params.offset === 0 ? '1' : null,
      items: [{ offset: String(request.params.offset), identity: `tool-${request.params.offset}`, content: [], deferred: request.params.offset !== 0,
        value: request.params.offset === 0 ? { name: 'first-tool' } : null }] };
    return { type: 'checkpoint_page', page };
  });
  server.handlers.set('conversation/checkpointValueRead', request => {
    if (request.method !== 'conversation/checkpointValueRead') throw Error('Wrong request');
    const { cut, domain, item_offset, path, offset } = request.params;
    const text = JSON.stringify(path[0] === 'revision' ? '0' : { name: 'last-tool', schema: { description: 'original-large-schema' } });
    return { type: 'checkpoint_value_chunk', chunk: { cut, domain, item_offset, path, offset, readable_end: text.length,
      representation: 'canonical_json_v1', bytes_base64: btoa(text), next_offset: null } };
  });
  const succeeded = panel();
  expect(await screen.findByText(/last-tool/)).toBeTruthy();
  expect(screen.getByText(/original-large-schema/)).toBeTruthy();
  expect(screen.queryByText('Loading…')).toBeNull(); expect(succeeded).toHaveBeenCalledOnce();
});
it('retires a directory read when its attachment changes and does not publish or request another page', async () => {
  await setup();
  const receipt = domainReceipt('capabilities', [{ revision: '0', configured_tools: [] }]);
  receipt.page.collections = [{ domain: 'capabilities.configured_tools', revision: '0', total: '2', loaded: '0', next_offset: '0' }];
  server.handlers.set('resources/read', () => receipt); server.held.add('conversation/checkpointPage');
  const original = server.client.target('A');
  const succeeded = panel();
  const request = await server.waitFor('conversation/checkpointPage', 1);
  server.nodeSnapshots.set('replacement-node', { ...snapshot('A'), conversation_id: 'replacement-conversation' });
  await server.client.switchNode('A', 'replacement-node'); await server.client.attach('A');
  expect(server.client.target('A')).not.toEqual(original);
  server.handlers.set('conversation/checkpointPage', () => ({ type: 'checkpoint_page', page: { ...receipt.page, domain: 'capabilities.configured_tools', total: '2', next_offset: '1',
    items: [{ offset: '0', identity: 'obsolete', content: [], deferred: false, value: { name: 'obsolete-tool' } }] } }));
  await act(async () => server.reply(request));
  expect(screen.queryByText(/obsolete-tool/)).toBeNull(); expect(succeeded).not.toHaveBeenCalled();
  expect(server.requests.filter(row => row.request.method === 'conversation/checkpointPage')).toHaveLength(1);
});
