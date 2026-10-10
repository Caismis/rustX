// @vitest-environment jsdom
import {afterEach,expect,it} from 'vitest';
import {act,cleanup,renderHook,waitFor} from '@testing-library/react';
import {cfg3Client,cfg3Host} from './cfg3-fixture';
import {userSettingsTarget,workspaceSettingsTarget,type SettingsTarget} from '../src/app/settings/projection';
import {useMcpProbes} from '../src/app/settings/mcp/useMcpProbes';
import type {McpProbeResult} from '../../protocol/app-server/v40';
afterEach(cleanup);
function setup(){
 const s=cfg3Client();
 for(const scope of ['user','workspace'] as const){
  s.source[scope]!.authored!.agent={tools:{sources:{exa:'all'}}};
  s.source[scope==='user'?'user_mcp':'workspace_mcp']!.authored={exa:{definition:{url:'https://example.invalid/mcp'},retained_env:[],retained_headers:[]}};
 }
 return s;
}
function deferred<T>(){let resolve!:(value:T)=>void;const promise=new Promise<T>(done=>resolve=done);return {promise,resolve};}
it('checks both scopes through their owner and refreshes without writing configuration',async()=>{
 const s=setup(),host=cfg3Host(s);
 const {result,rerender}=renderHook(({target}:{target:SettingsTarget})=>useMcpProbes(s.client,host,target,s.source,true),{initialProps:{target:userSettingsTarget as SettingsTarget}});
 await waitFor(()=>expect(result.current.states.exa).toBe('reachable'));
 expect(s.request.mock.calls.filter(([op])=>op.method==='mcp/probe')[0][0]).toMatchObject({params:{target:{kind:'user'},expected_revision:'mcp-1'}});
 rerender({target:workspaceSettingsTarget('A')});
 await waitFor(()=>expect(result.current.states.exa).toBe('reachable'));
 expect(s.request.mock.calls.filter(([op])=>op.method==='mcp/probe')[1][0]).toMatchObject({params:{target:{kind:'workspace',directory:'/workspace/A'},expected_revision:'mcp-2'}});
 act(()=>result.current.refresh());
 await waitFor(()=>expect(result.current.busy).toBe(false));
 expect(s.request.mock.calls.filter(([op])=>op.method==='mcp/probe')).toHaveLength(3);
 expect(s.request.mock.calls.some(([op])=>op.method==='configuration/sourceWrite')).toBe(false);
});
it('late results cannot cross scope changes or dispatch the rest of an obsolete queue',async()=>{
 const s=setup(),host=cfg3Host(s),held=deferred<{type:'mcp_probe';result:McpProbeResult}>();
 s.source.user.authored!.agent!.tools!.sources!['later']='all';
 s.source.user_mcp.authored!.later=s.source.user_mcp.authored!.exa;
 s.request.mockImplementationOnce(()=>held.promise as never);
 const {result,rerender}=renderHook(({target}:{target:SettingsTarget})=>useMcpProbes(s.client,host,target,s.source,true),{initialProps:{target:userSettingsTarget as SettingsTarget}});
 await waitFor(()=>expect(result.current.states.exa).toBe('checking'));
 rerender({target:workspaceSettingsTarget('A')});
 await waitFor(()=>expect(result.current.states.exa).toBe('reachable'));
 await act(async()=>held.resolve({type:'mcp_probe',result:{id:'exa',revision:'mcp-1',outcome:'connection_failed'}}));
 expect(result.current.states.exa).toBe('reachable');
 expect(s.request.mock.calls.some(([op])=>op.method==='mcp/probe'&&op.params.id==='later')).toBe(false);
});
it('configuration edits invalidate results, ordinary rerenders do not repeat checks, and disabling clears the light',async()=>{
 const s=setup(),host=cfg3Host(s);
 const {result,rerender}=renderHook(()=>useMcpProbes(s.client,host,userSettingsTarget,s.source,true));
 await waitFor(()=>expect(result.current.states.exa).toBe('reachable'));
 rerender();expect(s.request.mock.calls).toHaveLength(1);
 s.source.user_mcp.revision='changed';
 s.request.mockResolvedValueOnce({type:'mcp_probe',result:{id:'exa',revision:'changed',outcome:'connection_failed'}} as never);
 rerender();await waitFor(()=>expect(result.current.states.exa).toBe('connection_failed'));
 s.source.user.authored!.agent!.tools!.sources!.exa=[];
 rerender();await waitFor(()=>expect(result.current.states.exa).toBeUndefined());
 expect(s.request.mock.calls).toHaveLength(2);
});
it('unmount discards replies and prevents later dispatch',async()=>{
 const s=setup(),held=deferred<{type:'mcp_probe';result:McpProbeResult}>();
 s.source.user.authored!.agent!.tools!.sources!.later='all';s.source.user_mcp.authored!.later=s.source.user_mcp.authored!.exa;
 s.request.mockImplementationOnce(()=>held.promise as never);
 const {unmount}=renderHook(()=>useMcpProbes(s.client,undefined,userSettingsTarget,s.source,true));
 await waitFor(()=>expect(s.request).toHaveBeenCalledOnce());
 unmount();
 await act(async()=>held.resolve({type:'mcp_probe',result:{id:'exa',revision:'mcp-1',outcome:'reachable'}}));
 expect(s.request).toHaveBeenCalledOnce();
});
it('missing or mismatched native evidence stays unknown',async()=>{
 const s=setup();s.request.mockResolvedValueOnce({type:'mcp_probe',result:{id:'exa',revision:'other',outcome:'reachable'}} as never);
 const {result}=renderHook(()=>useMcpProbes(s.client,undefined,userSettingsTarget,s.source,true));
 await waitFor(()=>expect(result.current.busy).toBe(false));expect(result.current.states.exa).toBe('unknown');
});

it('transport replacement fences an already dispatched result before any rerender',async()=>{
 const s=setup(),held=deferred<{type:'mcp_probe';result:McpProbeResult}>();
 s.request.mockImplementationOnce(()=>held.promise as never);
 const {result}=renderHook(()=>useMcpProbes(s.client,undefined,userSettingsTarget,s.source,true));
 await waitFor(()=>expect(s.request).toHaveBeenCalledOnce());
 await act(async()=>{
  s.publish({connection:'disconnected',generation:2});
  held.resolve({type:'mcp_probe',result:{id:'exa',revision:'mcp-1',outcome:'reachable'}});
 });
 expect(result.current.states.exa).toBeUndefined();
 expect(s.request).toHaveBeenCalledOnce();
});
