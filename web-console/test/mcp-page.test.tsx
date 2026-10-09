// @vitest-environment jsdom
import {afterEach,expect,it} from 'vitest';
import {cleanup,fireEvent,render,screen,waitFor} from '@testing-library/react';
import {cfg3Client,cfg3Host} from './cfg3-fixture';
import {SettingsSurface,settingsReady,openSettingsPage,chooseOption} from './settings-harness';
import {userSettingsTarget} from '../src/app/settings/projection';
import {formatMcpJson,parseMcpJson} from '../src/app/settings/mcp/json';
afterEach(cleanup);
const writes=(s:ReturnType<typeof cfg3Client>)=>s.request.mock.calls.flatMap(([op])=>op.method==='configuration/sourceWrite'?[op.params]:[]);
async function open(){const s=cfg3Client(async (op,source)=>{if(op.method==='configuration/sourceWrite' && op.params.mutation.kind==='mcp') {const mutation=op.params.mutation;source.user_mcp.revision='mcp-saved';if(mutation.authored){const {env,headers,...definition}=mutation.authored.definition;source.user_mcp.authored![mutation.id]={definition,retained_env:Object.keys(env??{}),retained_headers:Object.keys(headers??{})};}else delete source.user_mcp.authored![mutation.id];}});render(<SettingsSurface client={s.client} target={userSettingsTarget} host={cfg3Host(s)}/>);await settingsReady();await openSettingsPage('MCP servers');return s;}
it('MCP has its own navigation and saves a new HTTP server before returning to the list',async()=>{
 const s=await open();
 expect(screen.queryByRole('tab',{name:'All'})).toBeNull();
 fireEvent.click(screen.getByRole('button',{name:'＋ New MCP server'}));
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'exa'}});
 await chooseOption('Type','HTTP');
 fireEvent.change(screen.getByLabelText('URL'),{target:{value:'https://mcp.example.com/mcp'}});
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'exa'});
 fireEvent.click(screen.getByRole('button',{name:'MCP exa'}));
 expect((screen.getByLabelText('URL') as HTMLInputElement).value).toBe('https://mcp.example.com/mcp');
 expect(writes(s)[0]).toMatchObject({target:{kind:'user'},expected_revision:'mcp-1',mutation:{kind:'mcp',id:'exa',authored:{definition:{type:'http',url:'https://mcp.example.com/mcp'}}}});
 await openSettingsPage('Extensions');
 expect(screen.queryByRole('listitem',{name:'exa'})).toBeNull();
});
it('JSON import requires a supported configuration and saves only the selected identity',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'⇩ Import'}));
 fireEvent.change(screen.getByLabelText('Complete configuration'),{target:{value:JSON.stringify({mcpServers:{docs:{command:'npx',args:['-y','docs'],sensitive_env:{TOKEN:'$TOKEN'}}}})}});
 fireEvent.click(screen.getByRole('button',{name:'Form'}));
 expect((screen.getByLabelText('Name') as HTMLInputElement).value).toBe('docs');
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'docs'});
 expect(writes(s)).toHaveLength(1);
 expect(writes(s)[0].mutation).toMatchObject({kind:'mcp',id:'docs',authored:{definition:{command:'npx',args:['-y','docs'],sensitive_env:{TOKEN:'$TOKEN'}}}});
 expect(screen.queryByDisplayValue(/test-secret/)).toBeNull();
});
it('JSON rejects foreign options instead of silently discarding them',()=>{
 expect(()=>parseMcpJson('{"command":"npx","timeoutMs":30000}')).toThrow('Unsupported field');
 expect(()=>parseMcpJson('{"type":"http","url":"file:///tmp/mcp"}')).toThrow();
 expect(parseMcpJson('{"command":"npx","sensitive_env":{"KEY":"$TOKEN"}}')[''].definition.sensitive_env).toEqual({KEY:'$TOKEN'});
});
it('canceling creation does not write a server',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'＋ New MCP server'}));
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'cancelled'}});
 fireEvent.change(screen.getByLabelText('Command'),{target:{value:'server'}});
 fireEvent.click(screen.getByRole('button',{name:'Cancel'}));
 expect(screen.getByText('No MCP servers installed')).toBeTruthy();expect(writes(s)).toHaveLength(0);
});
it('uses a compact argument input and JSON reference editor without generic unit controls',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'＋ New MCP server'}));
 expect(screen.queryByText(/New User definition/)).toBeNull();
 expect(screen.queryByRole('button',{name:'Add Arguments'})).toBeNull();
 expect(screen.queryByText('Empty list · no entries')).toBeNull();
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'compact'}});
 fireEvent.change(screen.getByLabelText('Command'),{target:{value:'npx'}});
 fireEvent.change(screen.getByLabelText('Arguments (space-separated)'),{target:{value:'-y server --path "/a b" ""'}});
 fireEvent.change(screen.getByLabelText('Environment variables (JSON)'),{target:{value:'{"TOKEN":"$TOKEN"}'}});
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'compact'});
 expect(writes(s)[0].mutation).toMatchObject({id:'compact',authored:{definition:{command:'npx',args:['-y','server','--path','/a b',''],sensitive_env:{TOKEN:'$TOKEN'}}}});
});
it('invalid reference JSON cannot save an earlier valid draft or escape through JSON mode',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'＋ New MCP server'}));
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'invalid-map'}});
 fireEvent.change(screen.getByLabelText('Command'),{target:{value:'server'}});
 const refs=screen.getByLabelText('Environment variables (JSON)');
 fireEvent.change(refs,{target:{value:'{"TOKEN":"$TOKEN"}'}});
 fireEvent.change(refs,{target:{value:'{"TOKEN":'}});
 expect((screen.getByRole('button',{name:'Save'}) as HTMLButtonElement).disabled).toBe(true);
 expect((screen.getByRole('button',{name:'JSON'}) as HTMLButtonElement).disabled).toBe(true);
 fireEvent.submit(screen.getByRole('button',{name:'Save'}).closest('form')!);
 expect(writes(s)).toHaveLength(0);
 fireEvent.change(refs,{target:{value:'{}'}});
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'invalid-map'});
 expect(writes(s)[0].mutation).toMatchObject({authored:{definition:{sensitive_env:{}}}});
});
it('saves directly from JSON using the same native source transaction',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'⇩ Import'}));
 fireEvent.change(screen.getByLabelText('Complete configuration'),{target:{value:'{"mcpServers":{"json-save":{"url":"https://example.com/mcp"}}}'}});
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'json-save'});
 expect(writes(s)).toHaveLength(1);
 expect(writes(s)[0]).toMatchObject({expected_revision:'mcp-1',mutation:{kind:'mcp',id:'json-save',authored:{definition:{url:'https://example.com/mcp'}}}});
});
it('shows the complete form immediately and keeps fields entered before the name through renaming',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'＋ New MCP server'}));
 const save=()=>screen.getByRole('button',{name:'Save'}) as HTMLButtonElement;
 expect(screen.getByLabelText('Command')).toBeTruthy();
 expect(screen.getByText('Arguments (space-separated)')).toBeTruthy();
 expect(screen.queryByLabelText('Working directory')).toBeNull();
 expect(screen.getByText('Environment references (optional)')).toBeTruthy();
 expect(save().disabled).toBe(true);
 fireEvent.change(screen.getByLabelText('Command'),{target:{value:'npx'}});
 expect(save().disabled).toBe(true);
 // Even an explicit submit event cannot write an unnamed server.
 fireEvent.submit(save().closest('form')!);expect(writes(s)).toHaveLength(0);
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'first'}});
 await waitFor(()=>expect((screen.getByLabelText('Command') as HTMLInputElement).value).toBe('npx'));
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'renamed'}});
 await waitFor(()=>expect(save().disabled).toBe(false));
 expect((screen.getByLabelText('Command') as HTMLInputElement).value).toBe('npx');
 fireEvent.click(save());await screen.findByRole('listitem',{name:'renamed'});
 expect(writes(s)).toHaveLength(1);
 expect(writes(s)[0].mutation).toMatchObject({kind:'mcp',id:'renamed',authored:{definition:{command:'npx'}}});
 fireEvent.click(screen.getByRole('button',{name:'＋ New'}));
 expect((screen.getByLabelText('Command') as HTMLInputElement).value).toBe('');
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'first'}});
 expect((screen.getByLabelText('Command') as HTMLInputElement).value).toBe('');
});
it('JSON edits survive repeated mode selection and are applied when returning to the form',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'＋ New MCP server'}));
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'json-form'}});
 fireEvent.change(screen.getByLabelText('Command'),{target:{value:'original'}});
 fireEvent.click(screen.getByRole('button',{name:'JSON'}));
 const json='{"type":"http","url":"https://example.com/mcp"}';
 fireEvent.change(screen.getByLabelText('Complete configuration'),{target:{value:json}});
 fireEvent.click(screen.getByRole('button',{name:'JSON'}));
 expect((screen.getByLabelText('Complete configuration') as HTMLTextAreaElement).value).toBe(json);
 fireEvent.click(screen.getByRole('button',{name:'Form'}));
 await waitFor(()=>expect((screen.getByLabelText('URL') as HTMLInputElement).value).toBe('https://example.com/mcp'));
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'json-form'});
 expect(writes(s)[0].mutation).toMatchObject({id:'json-form',authored:{definition:{type:'http',url:'https://example.com/mcp'}}});
});
it('invalid JSON remains editable when switching back to the form',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'⇩ Import'}));
 fireEvent.change(screen.getByLabelText('Complete configuration'),{target:{value:'{broken'}});
 fireEvent.click(screen.getByRole('button',{name:'Form'}));
 expect((screen.getByLabelText('Complete configuration') as HTMLTextAreaElement).value).toBe('{broken');
 expect(screen.getByRole('alert').textContent).toContain('Invalid JSON');expect(writes(s)).toHaveLength(0);
});
it('an unfinished form can visit JSON and return without being forced to complete it',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'＋ New MCP server'}));
 fireEvent.click(screen.getByRole('button',{name:'JSON'}));
 fireEvent.click(screen.getByRole('button',{name:'Form'}));
 expect(screen.getByLabelText('Command')).toBeTruthy();
 expect(screen.queryByRole('alert')).toBeNull();expect(writes(s)).toHaveLength(0);
});
it('a duplicate name cannot overwrite a server and resolving it preserves the new draft',async()=>{
 const s=await open();
 // Create the existing definition through the same native transaction path.
 fireEvent.click(screen.getByRole('button',{name:'＋ New MCP server'}));
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'existing'}});
 fireEvent.change(screen.getByLabelText('Command'),{target:{value:'original'}});
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'existing'});
 fireEvent.click(screen.getByRole('button',{name:'＋ New'}));
 fireEvent.change(screen.getByLabelText('Command'),{target:{value:'new-command'}});
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'existing'}});
 expect(screen.getByRole('alert').textContent).toContain('already exists');
 expect((screen.getByRole('button',{name:'Save'}) as HTMLButtonElement).disabled).toBe(true);
 fireEvent.submit(screen.getByRole('button',{name:'Save'}).closest('form')!);
 expect(writes(s)).toHaveLength(1);
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'different'}});
 await waitFor(()=>expect((screen.getByLabelText('Command') as HTMLInputElement).value).toBe('new-command'));
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'different'});
 expect(writes(s)).toHaveLength(2);
 expect(s.source.user_mcp.authored!.existing.definition.command).toBe('original');
 expect(writes(s)[1].mutation).toMatchObject({id:'different',authored:{definition:{command:'new-command'}}});
});
it('HTTP authors header references without literal credential fields',async()=>{
 const s=await open();fireEvent.click(screen.getByRole('button',{name:'＋ New MCP server'}));
 fireEvent.change(screen.getByLabelText('Name'),{target:{value:'headers-server'}});
 await chooseOption('Type','HTTP');
 fireEvent.change(screen.getByLabelText('URL'),{target:{value:'https://example.com/mcp'}});
 expect(screen.queryByText('Environment variables (optional)')).toBeNull();
 fireEvent.change(screen.getByLabelText('HTTP headers (JSON)'),{target:{value:JSON.stringify({Authorization:'$TOKEN'})}});
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'headers-server'});
 expect(writes(s)[0].mutation).toMatchObject({kind:'mcp',authored:{definition:{sensitive_headers:{Authorization:'$TOKEN'}}}});
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
 const props={source:s.source,scope:'workspace' as const,onFocus,scopeControl:<span>Workspace A</span>,refresh:vi.fn(),refreshing:false};
 const ui=render(<McpPage {...props}/>);
 expect(screen.getByText('No MCP servers installed')).toBeTruthy();
 expect(screen.queryByRole('listitem',{name:'exa'})).toBeNull();
 expect(screen.queryByText('Inherited from user')).toBeNull();
 expect(ui.container.querySelector('[class*="total"]')?.textContent).toBe('MCP 0');
 fireEvent.change(screen.getByRole('searchbox'),{target:{value:'exa'}});
 expect(screen.queryByRole('listitem',{name:'exa'})).toBeNull();
 await screen.findByText('No matching resources');
 expect(s.request.mock.calls.some(([op])=>String(op.method).startsWith('mcp/'))).toBe(false);
 // Switching scope reveals the same user definition without copying or editing it.
 s.source.target={kind:'user'};
 ui.rerender(<McpPage {...props} source={{...s.source}} scope="user"/>);
 expect(screen.getByRole('listitem',{name:'exa'})).toBeTruthy();
 expect(ui.container.querySelector('[class*="total"]')?.textContent).toBe('MCP 1');
 expect(writes(s)).toHaveLength(0);
 fireEvent.change(screen.getByRole('searchbox'),{target:{value:'missing'}});
 expect(ui.container.querySelector('[class*="total"]')?.textContent).toBe('MCP 0');
});

it('literal imports and malformed JSON never echo secret input',()=>{
 for(const text of ['{"command":"server","env":{"TOKEN":"private-value"}}','{"url":"https://example.invalid","headers":{"Authorization":"private-value"}}','{"private-value":']) {
  try {parseMcpJson(text);throw new Error('accepted');} catch(error) {expect(String(error)).not.toContain('private-value');expect(String(error)).not.toContain('accepted');}
 }
});

it('a shadowed User definition stays editable at its exact source and retained headers survive form and JSON edits',async()=>{
 const s=cfg3Client();
 s.source.user_mcp.authored={docs:{definition:{type:'http',url:'https://user.invalid'},retained_env:[],retained_headers:['Authorization']}};
 s.source.workspace_mcp!.authored={docs:{definition:{type:'http',url:'https://workspace.invalid'},retained_env:[],retained_headers:[]}};
 s.source.prospective_resources={...s.effective.resources,definitions:[{family:'mcp',name:'docs',valid:true,location:{scope:'workspace',path:'/workspace/.agents/mcp.toml',shadowed:s.source.user_mcp.path}}]};
 render(<SettingsSurface client={s.client} target={userSettingsTarget} host={cfg3Host(s)}/>);
 await settingsReady();await openSettingsPage('MCP servers');
 fireEvent.click(screen.getByRole('button',{name:'MCP docs'}));
 expect((screen.getByLabelText('URL') as HTMLInputElement).value).toBe('https://user.invalid');
 expect(screen.queryByRole('textbox',{name:'Authorization'})).toBeNull();
 expect(screen.queryByLabelText('Authorization')).toBeNull();
 fireEvent.click(screen.getByRole('button',{name:'JSON'}));
 fireEvent.change(screen.getByLabelText('Complete configuration'),{target:{value:'{"docs":{"type":"http","url":"https://user.invalid/updated"}}'}});
 fireEvent.click(screen.getByRole('button',{name:'Save'}));
 await screen.findByRole('listitem',{name:'docs'});
 expect(writes(s)).toHaveLength(1);
 expect(writes(s)[0]).toMatchObject({target:{kind:'user'},expected_revision:'mcp-1',mutation:{kind:'mcp',id:'docs',authored:{retained_headers:['Authorization'],definition:{url:'https://user.invalid/updated'}}}});
 expect(s.source.workspace_mcp!.authored!.docs.definition.url).toBe('https://workspace.invalid');
});

it('JSON presents a named definition without transaction metadata and accepts both reference formats',()=>{
 const value={definition:{type:'stdio' as const,command:'server',args:['a b']},retained_env:['TOKEN'],retained_headers:[]};
 const text=formatMcpJson('docs',value);
 expect(JSON.parse(text)).toEqual({docs:value.definition});
 expect(parseMcpJson(text).docs.definition).toEqual(value.definition);
 expect(parseMcpJson(JSON.stringify({mcpServers:{docs:value.definition}})).docs.definition).toEqual(value.definition);
 expect(text).not.toContain('retained_env');
});
