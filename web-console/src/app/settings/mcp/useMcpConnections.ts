import {useEffect, useRef, useState} from 'react';
import type {McpConnectionSnapshot, SourceSettings} from '../../../../../protocol/app-server/v38';
import type {AppServerClient} from '../../../client/app-server';
import type {McpOperation, ProductHostWorkspaces} from '../../../workspaces/host';
import {extensionEntries} from '../extensions/inventory';

export interface McpConnectionContext {client: AppServerClient; host?: ProductHostWorkspaces; endpoint: string; workspaceId?: string; active: boolean}
export function useMcpConnections(source: SourceSettings, context: McpConnectionContext) {
  const [snapshots,setSnapshots]=useState<Record<string,McpConnectionSnapshot>>({});
  const [error,setError]=useState('');
  const [refresh,setRefresh]=useState(0);
  const latest=useRef(context);latest.current=context;
  const scope=source.target.kind;
  const selection=source[scope]?.authored?.agent?.tools?.sources;
  const entries=extensionEntries(source,scope,'mcp').filter(entry=>entry.owner===scope).map(entry=>({id:entry.name,revision:(entry.owner==='user'?source.user_mcp:source.workspace_mcp)?.revision,
    enabled:entry.valid!==false && (selection?.[entry.name]==='all' || Array.isArray(selection?.[entry.name]) && (selection![entry.name] as string[]).length>0)}));
  const signature=JSON.stringify({target:source.target,entries,configuration:[source.user.revision,source.workspace?.revision]});
  useEffect(()=>{
    if(!context.active) return;
    let disposed=false;
    let timer:ReturnType<typeof setTimeout> | undefined;
    setSnapshots({});setError('');
    const {target,entries:servers}=JSON.parse(signature) as {target:SourceSettings['target'];entries:typeof entries};
    if(servers.length===0)return;
    const request=async(operation:McpOperation):Promise<McpConnectionSnapshot[]>=>{
      const current=latest.current;
      if(current.workspaceId!==undefined){
        if(!current.host?.configureWorkspace) throw new Error('Workspace connection unavailable');
        const result=await current.host.configureWorkspace(current.workspaceId,current.endpoint,operation);
        if(result.kind!=='mcp') throw new Error('Unexpected MCP response');
        return result.connections;
      }
      const result=operation.kind==='mcp_status'?await current.client.request({method:'mcp/status',params:{target}},'mcp_connections')
        :operation.kind==='mcp_connect'?await current.client.request({method:'mcp/connect',params:{target,id:operation.id,expected_revision:operation.expected_revision,refresh:operation.refresh}},'mcp_connections')
          :await current.client.request({method:'mcp/disconnect',params:{target,id:operation.id}},'mcp_connections');
      return result.connections;
    };
    const merge=(rows:McpConnectionSnapshot[],replace=false)=>{
      if(disposed)return;
      setSnapshots(previous=>({...(!replace ? previous : {}),...Object.fromEntries(rows.filter(row=>servers.some(server=>server.id===row.id&&server.revision===row.revision)).map(row=>[row.id,row]))}));
    };
    const poll=async()=>{
      try{merge(await request({kind:'mcp_status'}),true);}
      catch(error){if(!disposed){setSnapshots({});setError(String(error));}}
      if(!disposed)timer=setTimeout(poll,1000);
    };
    void (async()=>{
      // Sequential admission keeps a large catalog from exhausting RPC slots;
      // each native connection starts independently and returns immediately.
      for(const server of servers){
        if(disposed) return;
        try{
          if(server.enabled&&server.revision)merge(await request({kind:'mcp_connect',id:server.id,expected_revision:server.revision,refresh:refresh>0}));
          else await request({kind:'mcp_disconnect',id:server.id});
        }catch(error){if(!disposed)setError(String(error));}
      }
      if(!disposed)void poll();
    })();
    return()=>{disposed=true;clearTimeout(timer);};
  },[signature,context.active,context.client,context.endpoint,context.workspaceId,refresh]);
  return {snapshots,error,enabled:new Set(entries.filter(entry=>entry.enabled).map(entry=>entry.id)),refresh:()=>setRefresh(value=>value+1)};
}
