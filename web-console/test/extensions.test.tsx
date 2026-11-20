// @vitest-environment jsdom
import {afterEach,expect,it} from 'vitest';
import {act,cleanup,fireEvent,screen,waitFor,within} from '@testing-library/react';
import {mcpConnectionIdentity,mcpTransport} from '../src/bindings/mcp';
import {McpEditor} from '../src/app/resources/mcp/McpEditor';
import {McpToolCard} from '../src/app/resources/mcp/McpToolCard';
import {AvailabilityDetail} from '../src/app/resources/shared/AvailabilityDetail';
import {McpPermissionDetail} from '../src/app/resources/shared/McpPermissionDetail';
import {chooseOption,renderEditor} from './settings-harness';
import {cfg3Source} from './cfg3-data';
import type {SourceScope,SourceSettings,WorkflowToolChoice} from '../../protocol/app-server/v80';
afterEach(cleanup);
const toolChoice = (name: string, description: string): WorkflowToolChoice =>
  ({ name, description, input_schema: {}, result: { type: 'text' } });
const noop=()=>{};
const mcpDetail=(source:SourceSettings,scope:SourceScope,name:string)=><>
 <McpEditor source={source} scope={scope} focus={{kind:'mcp',name}} onEditorDraftChange={noop} scopeControl={<span>{scope}</span>} close={noop}/>
 <AvailabilityDetail source={source} scope={scope} revision="cfg-1" family="mcp" name={name}/>
</>;
it('MCP authoring is independent of root permissions and keeps its pinned source revision',async()=>{
 const source=cfg3Source();
 source.workspace_mcp!.authored={search:{definition:{type:'stdio',command:'server'},retained_env:['TOKEN'],retained_headers:[]}};
 const {writes,rerender}=await renderEditor(mcpDetail(source,'workspace','search'),{source,context:source});
 fireEvent.change(screen.getByLabelText('Command'),{target:{value:'edited-server'}});fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await waitFor(()=>expect(writes).toHaveBeenCalledTimes(1));
 expect(writes.mock.calls[0]).toEqual([{kind:'mcp',id:'search',authored:{definition:{type:'stdio',command:'edited-server'},retained_env:['TOKEN'],retained_headers:[]}},'mcp-2']);
 const fresh=structuredClone(source);fresh.workspace_mcp!.revision='external';rerender(mcpDetail(fresh,'workspace','search'));
 expect(screen.getByLabelText('Command')).toHaveProperty('value','edited-server');
 expect(writes.mock.calls.every(([mutation])=>mutation.kind==='mcp')).toBe(true);
 expect(screen.getByRole('form',{name:'Tools: search'})).toBeTruthy();
});
it('an open MCP removal confirmation cannot submit after its draft becomes stale', async () => {
 const source = cfg3Source();
 source.target = { kind: 'workspace', directory: '/workspace/A' };
 source.workspace_mcp!.authored = { search: { definition: { type: 'stdio', command: 'server' }, retained_env: [], retained_headers: [] } };
 const latest = structuredClone(source);
 latest.workspace_mcp!.revision = 'external-mcp';
 const { actor, context, writes, rerender } = await renderEditor(mcpDetail(source, 'workspace', 'search'), {
  source, context: source, reread: async () => latest,
 });
 fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'edited-server' } });
 fireEvent.click(screen.getByRole('button', { name: 'Remove MCP search' }));
 const dialog = await screen.findByRole('alertdialog', { name: 'Delete this MCP server?' });
 await act(async () => actor.send({ type: 'REFRESH' }));
 await waitFor(() => expect(context().observation?.workspace_mcp?.revision).toBe('external-mcp'));
 rerender(mcpDetail(latest, 'workspace', 'search'));
 fireEvent.click(within(dialog).getByRole('button', { name: 'Remove MCP search' }));
 expect(writes).not.toHaveBeenCalled();
 expect(screen.getByRole('button', { name: 'Remove MCP search' })).toHaveProperty('disabled', true);
});
it.each(['http','stdio'] as const)('preserves implicit %s and retained credential references when editing through the standalone MCP module',async transport=>{
 const source=cfg3Source();
 const authored=transport==='http'?{definition:{url:'https://remote.invalid/mcp',sensitive_headers:{'X-Key':'$KEY'}},retained_headers:['Authorization'],retained_env:[]}
  :{definition:{command:'server',sensitive_env:{KEY:'$KEY'}},retained_headers:[],retained_env:['TOKEN']};
 source.workspace_mcp!.authored={inferred:authored};
 const {writes}=await renderEditor(mcpDetail(source,'workspace','inferred'),{source,context:source});
 expect(writes).not.toHaveBeenCalled();
 fireEvent.change(screen.getByLabelText(transport==='http'?'URL':'Command'),{target:{value:transport==='http'?'https://remote.invalid/edited':'edited-server'}});
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await waitFor(()=>expect(writes).toHaveBeenCalledTimes(1));
 expect(writes.mock.calls[0]).toEqual([{kind:'mcp',id:'inferred',authored:{...authored,definition:{...authored.definition,...(transport==='http'?{url:'https://remote.invalid/edited'}:{command:'edited-server'})}}},'mcp-2']);
});
it('honors explicit transport and leaves new definitions local',()=>{
 expect(mcpTransport({type:'stdio',url:'https://example.invalid'})).toBe('stdio');
 expect(mcpTransport({url:'https://example.invalid'})).toBe('http');expect(mcpTransport({command:'server'})).toBe('stdio');expect(mcpTransport({})).toBe('stdio');
});
it('mcpConnectionIdentity ignores server and tool permission changes while tracking connection changes', () => {
 const base = {
  definition: { type: 'stdio' as const, command: 'server', args: ['--a'], sensitive_env: { B: '$B', A: '$A' } },
  retained_env: ['TOKEN'], retained_headers: [],
 };
 const withRules = {
  ...base,
  definition: {
   ...base.definition,
   sensitive_env: { A: '$A', B: '$B' },
   permissions: { plan: 'ask' as const, automatic: 'allow' as const, full_access: 'allow' as const },
   tools: { read_note: { permissions: { plan: 'deny' as const, automatic: 'deny' as const, full_access: 'deny' as const } } },
  },
 };
 expect(mcpConnectionIdentity(withRules)).toBe(mcpConnectionIdentity(base));
 expect(mcpConnectionIdentity(base, 'revision-1')).not.toBe(mcpConnectionIdentity(base, 'revision-2'));
 const publicConnection = { ...base, retained_env: [] };
 expect(mcpConnectionIdentity(publicConnection, 'revision-1')).toBe(mcpConnectionIdentity(publicConnection, 'revision-2'));
 expect(mcpConnectionIdentity({ ...base, definition: { ...base.definition, command: 'server-2' } })).not.toBe(mcpConnectionIdentity(base));
 expect(mcpConnectionIdentity({ ...base, retained_env: ['TOKEN', 'OTHER'] })).not.toBe(mcpConnectionIdentity(base));
 expect(mcpConnectionIdentity(undefined)).toBeUndefined();
});
it('switching transport in the MCP form preserves server permissions and per-tool rules', async () => {
 const source = cfg3Source();
 const permissions = { plan: 'ask' as const, automatic: 'default' as const, full_access: 'allow' as const };
 const tools = { read_note: { permissions: { plan: 'deny' as const, automatic: 'deny' as const, full_access: 'deny' as const } } };
 source.workspace_mcp!.authored = {
  search: { definition: { type: 'stdio', command: 'server', args: ['--v'], permissions, tools }, retained_env: ['TOKEN'], retained_headers: [] },
 };
 const { writes } = await renderEditor(mcpDetail(source, 'workspace', 'search'), { source, context: source });
 await chooseOption('Type', 'HTTP');
 fireEvent.change(screen.getByLabelText('URL'), { target: { value: 'https://mcp.example.com/mcp' } });
 fireEvent.click(screen.getByRole('button', { name: 'Save' }));
 await waitFor(() => expect(writes).toHaveBeenCalledTimes(1));
 expect(writes.mock.calls[0]).toEqual([{
  kind: 'mcp', id: 'search',
  authored: { definition: { type: 'http', url: 'https://mcp.example.com/mcp', permissions, tools }, retained_env: [], retained_headers: [] },
 }, 'mcp-2']);
});
it('rebasing a stale Form-mode MCP connection draft preserves newer server and tool rules', async () => {
 const source = cfg3Source();
 source.target = { kind: 'workspace', directory: '/workspace/A' };
 source.workspace_mcp!.authored = {
  search: { definition: { type: 'stdio', command: 'server' }, retained_env: ['OLD'], retained_headers: [] },
 };
 const latest = structuredClone(source);
 const permissions = { plan: 'ask' as const, automatic: 'allow' as const, full_access: 'allow' as const };
 const tools = { read_note: { permissions: { plan: 'deny' as const, automatic: 'deny' as const, full_access: 'deny' as const } } };
 latest.workspace_mcp!.revision = 'mcp-3';
 latest.workspace_mcp!.authored!.search = {
  definition: { type: 'stdio', command: 'server', permissions, tools },
  retained_env: ['OLD', 'NEW'], retained_headers: [],
 };
 const { actor, context, writes, rerender } = await renderEditor(mcpDetail(source, 'workspace', 'search'), {
  source, context: source, reread: async () => latest,
 });
 fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'edited-server' } });
 await act(async () => actor.send({ type: 'REFRESH' }));
 await waitFor(() => expect(context().observation?.workspace_mcp?.revision).toBe('mcp-3'));
 rerender(mcpDetail(latest, 'workspace', 'search'));
 fireEvent.click(screen.getByRole('button', { name: 'Use latest setting' }));
 fireEvent.click(screen.getByRole('button', { name: 'Save' }));
 await waitFor(() => expect(writes).toHaveBeenCalledTimes(1));
 expect(writes.mock.calls[0]).toEqual([{
  kind: 'mcp', id: 'search',
  authored: {
   definition: { type: 'stdio', command: 'edited-server', permissions, tools },
   retained_env: ['OLD', 'NEW'], retained_headers: [],
  },
 }, 'mcp-3']);
});
it('authored Workspace MCP tool and permission cards do not show inherited credential warnings', async () => {
 const source = cfg3Source();
 source.target = { kind: 'workspace', directory: '/workspace/A' };
 source.user_mcp.authored = {
  search: { definition: { type: 'stdio', command: 'global-server' }, retained_env: ['LITERAL_TOKEN'], retained_headers: [] },
 };
 const workspaceAuthored = { definition: { type: 'stdio' as const, command: 'workspace-server' }, retained_env: [], retained_headers: [] };
 source.workspace_mcp!.authored = { search: workspaceAuthored };
 await renderEditor(<>
  <McpPermissionDetail source={source} scope="workspace" revision="workspace-1" name="search" />
  <ul><McpToolCard tool={toolChoice('read_note', 'Read note')} server="search" authored={workspaceAuthored} revision="mcp-2" /></ul>
 </>, { source, context: source });
 expect(screen.queryByText(/Re-enter these credentials/)).toBeNull();
});
it('AvailabilityDetail keeps the exact picker open when unchecking the last tool and resets on Discard', async () => {
 const source = cfg3Source();
 source.user.authored = { agent: { tools: { sources: { notes: ['read_note'] } } } };
 const tools = [toolChoice('read_note', 'Read note'), toolChoice('search_notes', 'Search notes')];
 await renderEditor(<AvailabilityDetail source={source} scope="user" revision="user-1" family="mcp" name="notes" tools={tools} />, {
  source, context: source,
 });
 fireEvent.click(screen.getByRole('checkbox', { name: 'read_note' }));
 expect(screen.getByRole('checkbox', { name: 'search_notes' })).toBeTruthy();
 fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
 expect((screen.getByRole('checkbox', { name: 'read_note' }) as HTMLInputElement).checked).toBe(true);

 cleanup();
 const empty = cfg3Source();
 await renderEditor(<AvailabilityDetail source={empty} scope="user" revision="user-1" family="mcp" name="notes" tools={tools} />, {
  source: empty, context: empty,
 });
 await chooseOption('Selection', 'Selected tools');
 fireEvent.click(screen.getByRole('checkbox', { name: 'read_note' }));
 fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
 expect(screen.queryByRole('checkbox', { name: 'read_note' })).toBeNull();
});
