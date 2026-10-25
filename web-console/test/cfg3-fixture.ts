import {selectedRevision,revisionSelector} from '../src/app/settings/projection';
import {applicationReceipt} from './fixture';
import { materializeSource } from '../../protocol/app-server/source-observation';
import { boundedSettings } from "./fixture";
import { cfg3Source, cfg3Effective, cfg3Application } from './cfg3-data';
import { snapshot } from './fixture';
import { vi } from 'vitest';
import type { AttachmentTarget, EffectiveConfiguration, MethodResult, Request1, SourceSettings } from '../../protocol/app-server/v57';
import { AppServerClient } from '../src/client/app-server';
import type { ClientView } from '../src/client/contracts';
export const cfg3Session = 'ses_00000000-0000-7000-8000-000000000001';
export const cfg3Target: AttachmentTarget = { session_id: cfg3Session, conversation_id: 'conv_00000000-0000-7000-8000-000000000001', runtime_incarnation: '1', attachment_id: 'attachment-1' };
export function cfg3Client(handler?: (operation: Request1, source: SourceSettings, effective: EffectiveConfiguration) => Promise<MethodResult | void>) { const source = cfg3Source(), effective = cfg3Effective(), client = new AppServerClient(); let state: ClientView = { connection: 'connected', generation: 1, sessions: [], uncertain: [], interactionOperations: {}, views: { [cfg3Session]: { id: cfg3Session, attachmentIntent: 'wanted', attachment: 'attached', nodeId: 'node-fixture', attachmentNodeId: 'node-fixture', attachmentObservation: { generation: 1, nodeId: 'node-fixture', target: cfg3Target, intentRevision: 0 }, target: cfg3Target, snapshot: snapshot() } } }; vi.spyOn(client, 'getSnapshot').mockImplementation(() => state);
    // Like the real client, a published snapshot change notifies every subscriber synchronously.
    const listeners = new Set<() => void>(), subscribe = client.subscribe; vi.spyOn(client, 'subscribe').mockImplementation(listener => { listeners.add(listener); const release = subscribe(listener); return () => { listeners.delete(listener); release(); }; });
    const publish = (patch: Partial<ClientView>) => { state = { ...state, ...patch }; for (const listener of listeners) listener(); }; const request = vi.spyOn(client, 'request').mockImplementation(async (operation) => { const overridden = await handler?.(operation, source, effective); if (overridden)
    return overridden as never; if (operation.method === 'mcp/probe') return {type:'mcp_probe',result:{id:operation.params.id,revision:operation.params.expected_revision,outcome:'reachable'}} as never; if (operation.method === 'session/configuration')
    return { type: 'session_configuration', projection:(applicationReceipt((source.application ?? null))) } as never; if (operation.method === 'session/effectiveConfiguration')
    return { type: 'effective_configuration', projection: {revision:'fixture',inline:structuredClone(effective),fields:[],collections:[]} } as never; if (operation.method === 'session/adoptConfiguration') {
    source.application = { ...source.application!, version: '3', candidate: null, units: { instructions: { status: 'applied'} } };
    effective.generation = '8';
    effective.adopted_binding = '2';
    return {type:'configuration_receipt',scope:(source.application)?.scope??'configuration',version:(source.application)?.version??'0'} as never;
} if (operation.method === 'configuration/reconcile')
    return {type:'configuration_receipt',scope:(source.application ?? cfg3Application())?.scope??'configuration',version:(source.application ?? cfg3Application())?.version??'0'} as never; if (operation.method === 'configuration/sourceWrite') {
    source.application = cfg3Application();
    source[operation.params.target.kind]!.revision = 'saved-2';
    return {type:'sources_written',revision:selectedRevision(source,revisionSelector(operation.params.mutation)),configuration_revision:'1'} as never;
} if ('target' in (operation.params ?? {}) && operation.method.startsWith('configuration/'))
    source.target = (operation as Extract<Request1, {
        method: 'configuration/sourcesRead';
    }>).params.target; return { type: 'source_settings', projection:boundedSettings(structuredClone(source)) } as never; }); return { client, request, source, effective, get state() { return state; }, publish, host: undefined as import('../src/workspaces/host').ProductHostWorkspaces | undefined }; }
export function cfg3Host(subject: ReturnType<typeof cfg3Client>): import('../src/workspaces/host').ProductHostWorkspaces {
  const read = async (target: Extract<Request1, {method: 'configuration/sourcesRead'}>['params']['target']) => {
    const { projection } = await subject.client.request({method:'configuration/sourcesRead',params:{target}},'source_settings');
    return materializeSource(projection, async (reference, offset) => (await subject.client.request({method:'configuration/sourceValueRead',params:{target,reference,offset,max_bytes:65536}},'source_value_chunk')).chunk, () => true);
  };
  return { listWorkspaces: async () => ({authorityId:'fixture-host',endpoint:'',workspaces:['A','B'].map(id=>({id,displayName:id,location:id,displayPath:'/workspace/'+id})),picker:{kind:'unavailable',reason:'fixture'}}),
    configureWorkspace: async (id,_endpoint,operation) => {
      const target={kind:'workspace' as const,directory:'/workspace/'+id};
      if (operation.kind==='mcp_probe') { const {result}=await subject.client.request({method:'mcp/probe',params:{target,id:operation.id,expected_revision:operation.expected_revision}},'mcp_probe'); return {kind:'mcp_probe',result}; }
      if (operation.kind==='write') {
        const receipt = await subject.client.request({method:'configuration/sourceWrite',params:{target,expected_revision:operation.expected_revision,mutation:operation.mutation}},'sources_written');
        const acknowledgement={target,revision:receipt.revision};
        let reread: import('../src/workspaces/host').WorkspaceConfigurationReread;
        try { reread={status:'observed',projection:await read(target)}; } catch(error) { reread={status:'failed',error}; }
        return {kind:'write',commit:{acknowledgement,reread}};
      }
      if(operation.kind==='reconcile') await subject.client.request({method:'configuration/reconcile',params:{target}},'configuration_receipt');
      return {kind:operation.kind,projection:await read(target)};
    }, adoptWorkspace:async()=>{},renameWorkspace:async()=>{},reorderWorkspace:async()=>{},removeWorkspace:async()=>{},resolveWorkspace:async(id)=>({cwd:'/workspace/'+id}),classifyLocations:async(cwds)=>cwds.map(()=>({authorized:true})) };
}
