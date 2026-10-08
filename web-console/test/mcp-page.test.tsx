// @vitest-environment jsdom
import {afterEach,expect,it} from 'vitest';
import {cleanup,fireEvent,render,screen} from '@testing-library/react';
import {cfg3Client,cfg3Host} from './cfg3-fixture';
import {SettingsSurface,settingsReady,openSettingsPage,chooseOption} from './settings-harness';
import {userSettingsTarget} from '../src/app/settings/projection';
import {parseMcpJson} from '../src/app/settings/mcp/json';
afterEach(cleanup);
const writes=(s:ReturnType<typeof cfg3Client>)=>s.request.mock.calls.flatMap(([op])=>op.method==='configuration/sourceWrite'?[op.params]:[]);
async function open(){const s=cfg3Client(async (op,source)=>{if(op.method==='configuration/sourceWrite' && op.params.mutation.kind==='mcp') {const mutation=op.params.mutation;source.user_mcp.revision='mcp-saved';if(mutation.authored){const {env,headers,...definition}=mutation.authored.definition;source.user_mcp.authored![mutation.id]={definition,retained_env:Object.keys(env??{}),retained_headers:Object.keys(headers??{})};}else delete source.user_mcp.authored![mutation.id];}});render(<SettingsSurface client={s.client} target={userSettingsTarget} host={cfg3Host(s)}/>);await settingsReady();await openSettingsPage('MCP servers');return s;}
it('MCP has its own navigation and saves a new HTTP server before returning to the list',async()=>{
 const s=await open();
 expect(screen.queryByRole('tab',{name:'All'})).toBeNull();
 fireEvent.click(screen.getByRole('button',{name:'＋ New MCP server'}));
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'exa'}});
 await chooseOption('Transport','HTTP');
 fireEvent.change(screen.getByLabelText('MCP URL'),{target:{value:'https://mcp.example.com/mcp'}});
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'exa'});
 expect(writes(s)[0]).toMatchObject({target:{kind:'user'},expected_revision:'mcp-1',mutation:{kind:'mcp',id:'exa',authored:{definition:{type:'http',url:'https://mcp.example.com/mcp'}}}});
 await openSettingsPage('Extensions');
 expect(screen.queryByRole('listitem',{name:'exa'})).toBeNull();
});
it('JSON import requires a supported configuration and saves only the selected identity',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'⇩ Import'}));
 fireEvent.change(screen.getByLabelText('MCP configuration JSON'),{target:{value:JSON.stringify({mcpServers:{docs:{command:'npx',args:['-y','docs'],env:{TOKEN:'test-secret'}}}})}});
 fireEvent.click(screen.getByRole('button',{name:'Use configuration'}));
 expect((screen.getByLabelText('Name') as HTMLInputElement).value).toBe('docs');
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'docs'});
 expect(writes(s)).toHaveLength(1);
 expect(writes(s)[0].mutation).toMatchObject({kind:'mcp',id:'docs',authored:{definition:{command:'npx',args:['-y','docs'],env:{TOKEN:'test-secret'}}}});
 expect(screen.queryByDisplayValue(/test-secret/)).toBeNull();
});
it('JSON rejects foreign options instead of silently discarding them',()=>{
 expect(()=>parseMcpJson('{"command":"npx","timeoutMs":30000}')).toThrow('Unsupported field');
 expect(()=>parseMcpJson('{"type":"http","url":"file:///tmp/mcp"}')).toThrow();
 expect(parseMcpJson('{"command":"npx","sensitive_env":{"KEY":"TOKEN"}}')[''].definition.sensitive_env).toEqual({KEY:'TOKEN'});
});
it('canceling creation does not write a server',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'＋ New MCP server'}));
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'cancelled'}});
 fireEvent.change(screen.getByLabelText('MCP command'),{target:{value:'server'}});
 fireEvent.click(screen.getByRole('button',{name:'Cancel'}));
 expect(screen.getByText('No MCP servers installed')).toBeTruthy();expect(writes(s)).toHaveLength(0);
});
it('HTTP exposes request headers directly and saves the entered map',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'＋ New MCP server'}));
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'headers-server'}});
 await chooseOption('Transport','HTTP');
 fireEvent.change(screen.getByLabelText('MCP URL'),{target:{value:'https://example.com/mcp'}});
 expect(screen.queryByText('Environment variables (optional)')).toBeNull();
 fireEvent.click(screen.getByText('Request headers (optional)'));
 const headers=screen.getByLabelText('Request headers (optional)') as HTMLTextAreaElement;
 expect(headers.value).toBe('');expect(headers.placeholder).toContain('Authorization');
 fireEvent.change(headers,{target:{value:'{"Authorization":"Bearer test-token"}'}});
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'headers-server'});
 expect(writes(s)[0].mutation).toMatchObject({kind:'mcp',authored:{definition:{headers:{Authorization:'Bearer test-token'}}}});
});

it('workspace MCP management excludes user definitions from rows, counts, search and connection actions',async()=>{
 const s=cfg3Client();
 s.source.target={kind:'workspace',directory:'/workspace/A'};
 s.source.user_mcp.authored={exa:{definition:{type:'http',url:'https://mcp.exa.ai/mcp'},retained_env:[],retained_headers:[]}};
 s.source.prospective_resources={...s.effective.resources,definitions:[]};
 s.source.prospective_resources.definitions.push({family:'mcp',name:'exa',valid:true,location:{scope:'user',path:s.source.user_mcp.path}});
 const {McpPage}=await import('../src/app/settings/mcp/McpPage');
 const {vi}=await import('vitest');
 const onFocus=vi.fn();
 const props={source:s.source,scope:'workspace' as const,onFocus,scopeControl:<span>Workspace A</span>,refresh:vi.fn(),refreshing:false,connection:{client:s.client,endpoint:'',active:true}};
 const ui=render(<McpPage {...props}/>);
 expect(screen.getByText('No MCP servers installed')).toBeTruthy();
 expect(screen.queryByRole('listitem',{name:'exa'})).toBeNull();
 expect(screen.queryByText('Inherited from user')).toBeNull();
 expect(ui.container.querySelector('[class*="total"]')?.textContent).toBe('MCP 0');
 fireEvent.change(screen.getByRole('searchbox'),{target:{value:'exa'}});
 expect(screen.queryByRole('listitem',{name:'exa'})).toBeNull();
 await screen.findByText('No matching resources');
 expect(s.request.mock.calls.some(([op])=>op.method==='mcp/connect'||op.method==='mcp/disconnect')).toBe(false);
 // Switching scope reveals the same user definition without copying or editing it.
 s.source.target={kind:'user'};
 ui.rerender(<McpPage {...props} source={{...s.source}} scope="user"/>);
 expect(screen.getByRole('listitem',{name:'exa'})).toBeTruthy();
 expect(ui.container.querySelector('[class*="total"]')?.textContent).toBe('MCP 1');
 expect(writes(s)).toHaveLength(0);
 fireEvent.change(screen.getByRole('searchbox'),{target:{value:'missing'}});
 expect(ui.container.querySelector('[class*="total"]')?.textContent).toBe('MCP 0');
});
