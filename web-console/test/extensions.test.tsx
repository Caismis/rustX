// @vitest-environment jsdom
import {afterEach,expect,it} from 'vitest';
import {act,cleanup,fireEvent,screen,waitFor,within} from '@testing-library/react';
import {mcpTransport} from '../src/bindings/mcp';
import {McpEditor} from '../src/app/resources/mcp/McpEditor';
import {AvailabilityDetail} from '../src/app/resources/shared/AvailabilityDetail';
import {renderEditor} from './settings-harness';
import {cfg3Source} from './cfg3-data';
import type {SourceScope,SourceSettings} from '../../protocol/app-server/v77';
afterEach(cleanup);
const noop=()=>{};
const mcpDetail=(source:SourceSettings,scope:SourceScope,name:string)=><>
 <McpEditor source={source} scope={scope} revision="cfg-1" focus={{kind:'mcp',name}} onEditorDraftChange={noop} scopeControl={<span>{scope}</span>} close={noop}/>
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
