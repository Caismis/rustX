// @vitest-environment jsdom
import {afterEach,expect,it} from 'vitest';
import {act,cleanup,renderHook} from '@testing-library/react';
import {cfg3Client,cfg3Host} from './cfg3-fixture';
import {userSettingsTarget,workspaceSettingsTarget,type SettingsTarget} from '../src/app/settings/projection';
import {useMcpProbes} from '../src/app/settings/mcp/useMcpProbes';
import type {McpProbeResult} from '../../protocol/app-server/v44';
afterEach(cleanup);
function setup(){
 const s=cfg3Client();
 for(const scope of ['user','workspace'] as const){
  s.source[scope==='user'?'user_mcp':'workspace_mcp']!.authored={exa:{definition:{url:'https://example.invalid/mcp'},retained_env:[],retained_headers:[]},other:{definition:{command:'never-start'},retained_env:[],retained_headers:[]}};
 }
 return s;
}
function deferred<T>(){let resolve!:(value:T)=>void;const promise=new Promise<T>(done=>resolve=done);return {promise,resolve};}
it('passive opening, rereading, scope changes and reopening never dispatch probes',()=>{
 const s=setup(),host=cfg3Host(s);
 const hook=renderHook(({target,active}:{target:SettingsTarget;active:boolean})=>useMcpProbes(s.client,host,target,s.source,active),{initialProps:{target:userSettingsTarget as SettingsTarget,active:true}});
 hook.rerender({target:workspaceSettingsTarget('A'),active:true});
 s.source.workspace_mcp!.revision='changed';hook.rerender({target:workspaceSettingsTarget('A'),active:true});
 hook.rerender({target:userSettingsTarget,active:false});hook.rerender({target:userSettingsTarget,active:true});
 expect(hook.result.current.states).toEqual({});expect(s.request).not.toHaveBeenCalled();
});
it('explicit actions dispatch only the selected current source definition and revision',async()=>{
 const s=setup(),host=cfg3Host(s);
 const {result,rerender}=renderHook(({target}:{target:SettingsTarget})=>useMcpProbes(s.client,host,target,s.source,true),{initialProps:{target:userSettingsTarget as SettingsTarget}});
 await act(()=>result.current.testConnection('exa'));
 expect(result.current.states.exa).toBe('reachable');expect(s.request).toHaveBeenCalledOnce();
 expect(s.request.mock.calls[0][0]).toEqual({method:'mcp/probe',params:{target:{kind:'user'},id:'exa',expected_revision:'mcp-1'}});
 rerender({target:workspaceSettingsTarget('A')});expect(result.current.states).toEqual({});
 await act(()=>result.current.testConnection('other'));
 expect(s.request.mock.calls[1][0]).toEqual({method:'mcp/probe',params:{target:{kind:'workspace',directory:'/workspace/A'},id:'other',expected_revision:'mcp-2'}});
 expect(result.current.states.exa).toBeUndefined();
});
it('duplicate clicks admit one operation and late results cannot cross scope or revision',async()=>{
 const s=setup(),host=cfg3Host(s),held=deferred<{type:'mcp_probe';result:McpProbeResult}>();
 s.request.mockImplementationOnce(()=>held.promise as never);
 const {result,rerender}=renderHook(({target}:{target:SettingsTarget})=>useMcpProbes(s.client,host,target,s.source,true),{initialProps:{target:userSettingsTarget as SettingsTarget}});
 let pending!:Promise<void>;
 act(()=>{pending=result.current.testConnection('exa');void result.current.testConnection('exa');});
 expect(s.request).toHaveBeenCalledOnce();expect(result.current.states.exa).toBe('checking');
 rerender({target:workspaceSettingsTarget('A')});
 await act(()=>result.current.testConnection('exa'));
 await act(async()=>{held.resolve({type:'mcp_probe',result:{id:'exa',revision:'mcp-1',outcome:'connection_failed'}});await pending;});
 expect(result.current.states.exa).toBe('reachable');
 s.source.workspace_mcp!.revision='changed';rerender({target:workspaceSettingsTarget('A')});
 expect(result.current.states).toEqual({});expect(s.request).toHaveBeenCalledTimes(2);
});
it.each(['connection_failed','cancelled','settlement_failed','timed_out','list_failed'] as const)('preserves native %s evidence without retrying',async outcome=>{
 const s=setup();s.request.mockResolvedValueOnce({type:'mcp_probe',result:{id:'exa',revision:'mcp-1',outcome}} as never);
 const {result,rerender}=renderHook(()=>useMcpProbes(s.client,undefined,userSettingsTarget,s.source,true));
 await act(()=>result.current.testConnection('exa'));rerender();
 expect(result.current.states.exa).toBe(outcome);expect(s.request).toHaveBeenCalledOnce();
});
it('mismatched evidence and retired registration admission remain unknown',async()=>{
 const s=setup(),host=cfg3Host(s);
 s.request.mockResolvedValueOnce({type:'mcp_probe',result:{id:'exa',revision:'other',outcome:'reachable'}} as never);
 const {result,rerender}=renderHook(({target}:{target:SettingsTarget})=>useMcpProbes(s.client,host,target,s.source,true),{initialProps:{target:userSettingsTarget as SettingsTarget}});
 await act(()=>result.current.testConnection('exa'));expect(result.current.states.exa).toBe('unknown');
 host.configureWorkspace=async()=>{throw new Error('Unknown Workspace registration');};
 rerender({target:workspaceSettingsTarget('A')});await act(()=>result.current.testConnection('exa'));
 expect(result.current.states.exa).toBe('unknown');expect(s.request).toHaveBeenCalledOnce();
});
it.each(['revision','disconnect','unmount'] as const)('%s fences pending replies without dispatching more work',async boundary=>{
 const s=setup(),held=deferred<{type:'mcp_probe';result:McpProbeResult}>();s.request.mockImplementationOnce(()=>held.promise as never);
 const {result,rerender,unmount}=renderHook(()=>useMcpProbes(s.client,undefined,userSettingsTarget,s.source,true));
 let pending!:Promise<void>;act(()=>{pending=result.current.testConnection('exa');});
 if(boundary==='revision'){s.source.user_mcp.revision='changed';rerender();}
 if(boundary==='disconnect')act(()=>s.publish({connection:'disconnected',generation:2}));
 if(boundary==='unmount')unmount();
 await act(async()=>{held.resolve({type:'mcp_probe',result:{id:'exa',revision:'mcp-1',outcome:'reachable'}});await pending;});
 if(boundary!=='unmount')expect(result.current.states).toEqual({});
 expect(s.request).toHaveBeenCalledOnce();
});

it.each(['pending','completed'] as const)('registration retirement invalidates %s evidence through the existing catalog owner',async phase=>{
 const {WorkspaceAuthority}=await import('../src/workspaces/authority');
 const {WorkspaceAssociations}=await import('../src/workspaces/associations');
 const {waitFor}=await import('@testing-library/react');
 const s=setup(),host=cfg3Host(s),list=host.listWorkspaces;
 const endpoint='ws://127.0.0.1:3000';s.publish({endpoint});
 let retired=false;
 host.listWorkspaces=async()=>{const catalog=await list();return {...catalog,endpoint,workspaces:catalog.workspaces.filter(row=>!retired||row.id!=='A')};};
 const authority=new WorkspaceAuthority(host),associations=new WorkspaceAssociations(s.client,authority);
 const release=associations.start();
 await waitFor(()=>expect(associations.getSnapshot().status).toBe('ready'));
 const held=deferred<{type:'mcp_probe';result:McpProbeResult}>();
 if(phase==='pending')s.request.mockImplementationOnce(()=>held.promise as never);
 const {result}=renderHook(()=>useMcpProbes(s.client,host,workspaceSettingsTarget('A'),s.source,true,associations));
 let pending!:Promise<void>;act(()=>{pending=result.current.testConnection('exa');});
 if(phase==='completed')await act(()=>pending);
 act(()=>{retired=true;expect(associations.captureMutation()?.commit('A')).toBe(true);});
 expect(result.current.states).toEqual({});expect(result.current.available).toBe(false);
 if(phase==='pending')await act(async()=>{held.resolve({type:'mcp_probe',result:{id:'exa',revision:'mcp-2',outcome:'reachable'}});await pending;});
 expect(result.current.states).toEqual({});await act(()=>result.current.testConnection('exa'));
 expect(s.request).toHaveBeenCalledOnce();release();
});

it.each([userSettingsTarget,workspaceSettingsTarget('A')])('an obsolete click handler cannot dispatch after native authority replacement: $kind',async target=>{
 const s=setup(),host=cfg3Host(s);
 const {result}=renderHook(()=>useMcpProbes(s.client,host,target,s.source,true));
 s.state.generation=2;
 await act(()=>result.current.testConnection('exa'));
 expect(s.request).not.toHaveBeenCalled();
});
