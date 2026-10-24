/* Copyright (c) 2026 DeepSeek. MIT. Header layout adapted from ui-subagent/SubagentHeaderLineage; see PROVENANCE.md. */
import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from 'react';
import type { RuntimeClientAgent, AgentStatistics } from '../../../../protocol/app-server/v57';
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import { SubagentMenu } from './SubagentCatalog';
import { AgentCard } from '../components/ActivityCards';
import css from './Subagents.module.css';
import conversationCss from '../../presentation/agent/Conversation.module.css';

import { SubagentContext as Context, useSubagents } from './subagent-context';
import { meterDemand, type MeterScope } from '../../client/agent-meters';

const NO_AGENTS: RuntimeClientAgent[] = [];
export function SubagentScope({ client, sessionId, children, openAside, onMode }: { client: AppServerClient; sessionId?: string; children: ReactNode; onMode?: (mode: 'chat' | 'trajectory') => void; openAside?: (agent: RuntimeClientAgent) => void }) {
  const tx = useTranslation();
  const agents = useClientSelector(client, state => sessionId ? state.views[sessionId]?.snapshot?.agents : undefined) ?? NO_AGENTS;
  const agentRevision = useClientSelector(client, state => sessionId ? state.views[sessionId]?.checkpoint?.read_cut.domain_revisions.agents : undefined);
  const workflowRevision = useClientSelector(client, state => sessionId ? state.views[sessionId]?.snapshot?.workflows.revision : undefined);
  const generation = useClientSelector(client, state => state.generation);
  const target = useClientSelector(client, state => sessionId ? state.views[sessionId]?.target : undefined);
  const intent = useClientSelector(client, state => sessionId ? state.views[sessionId]?.attachmentIntent : undefined);
  const admission = useClientSelector(client, state => sessionId ? state.views[sessionId]?.attachmentObservation : undefined);
  const owner = JSON.stringify([generation, sessionId, target, admission]);
  const inventoryRef = useRef(agents);
  const scope = useMemo<MeterScope>(() => ({
    target,
    current: () => !!sessionId && client.isAttachmentObservationCurrent(sessionId, admission),
    inventory: () => inventoryRef.current,
  }), [client, owner, admission]);
  const [headerError, setHeaderError] = useState<{owner:string;id:string;lease?:object;message:string}>();
  const [headerWork, setHeaderWork] = useState(0);
  const headerReads = useRef<{owner:string;seen:Map<string,{revision:string;lease?:object}>;flight?:object}>({owner,seen:new Map()});
  const currentOwner = useRef(owner); currentOwner.current = owner;
  if(headerReads.current.owner !== owner) { headerReads.current.owner=owner; headerReads.current.seen.clear(); }
  const [selection, setSelection] = useState<{ owner: string; id?: string; opened: RuntimeClientAgent[]; leases?:Map<string,object>; oneShot?: Set<string>; full?: boolean }>();
  const id = selection?.owner === owner ? selection.id : undefined;
  const residents = selection?.owner === owner ? selection.opened : NO_AGENTS;
  const residentLeases = useRef(selection); residentLeases.current=selection;
  const inventory = [...agents, ...residents.filter(value => !agents.some(agent => agent.agent_id === value.agent_id))];
  inventoryRef.current = inventory;
  const navigation = useRef(0);
  const open = (id?: string, source?: RuntimeClientAgent, inspection = false) => { navigation.current++; setSelection(previous => {
    const opened = previous?.owner === owner ? previous.opened : [];
    const leases=previous?.owner===owner ? previous.leases??new Map<string,object>() : new Map<string,object>();
    const oneShot = new Set(previous?.owner === owner ? previous.oneShot : []);
    if (inspection && id) oneShot.add(id);
    if (!id) return { owner, opened, leases, oneShot };
    const agent = source ?? agents.find(value => value.agent_id === id) ?? opened.find(value => value.agent_id === id);
    if (!agent) return previous;
    if (opened.some(value => value.agent_id === id)) return { owner, id, opened, leases, oneShot };
    if (opened.length === 8) return { owner, id: previous?.id, opened, leases, oneShot, full: true };
    return { owner, id, opened: [...opened, agent], oneShot, leases:new Map([...leases,[id,{}]]) };
  }); };
  const openWorkflow = async (id: string, activation: string, title: string) => {
    if (!target || !scope.current()) return;
    const intent = ++navigation.current;
    const current = () => currentOwner.current === owner && navigation.current === intent && scope.current();
    const { page } = await client.request({ method: 'agent/status', params: { target, agent_id: id } }, 'domain_page', undefined, current);
    if (!current()) return;
    const header = page.items[0]?.value as unknown as RuntimeClientAgent | undefined;
    if (page.items.length !== 1 || page.domain !== `workflow_children.${activation}` || header?.agent_id !== id || header.activation_id !== activation) throw new Error('Workflow child source changed.');
    open(id, { ...header, title }, true);
    onMode?.('chat');
  };
  const close = (id: string) => setSelection(previous => previous?.owner === owner ? { owner, oneShot: new Set([...previous.oneShot ?? []].filter(key => key !== id)), id: previous.id === id ? undefined : previous.id, opened: previous.opened.filter(value => value.agent_id !== id), leases:new Map([...previous.leases??[]].filter(([key])=>key!==id)) } : previous);
  // Retained off-head views refresh their own finite native header. One actual
  // read stays charged across source/view replacement; no per-revision queue.
  useEffect(() => {
    const reads=headerReads.current, residentIds=new Set(residents.map(value=>value.agent_id));
    for(const id of reads.seen.keys()) if(!residentIds.has(id)) reads.seen.delete(id);
    if(reads.flight || !target || !scope.current()) return;
    const revision=JSON.stringify([agentRevision, workflowRevision]);
    const source=residents.find(value=>!agents.some(agent=>agent.agent_id===value.agent_id)&&(reads.seen.get(value.agent_id)?.revision!==revision||reads.seen.get(value.agent_id)?.lease!==selection?.leases?.get(value.agent_id)));
    if(!source) return;
    const lease=selection?.leases?.get(source.agent_id);
    const token={};reads.flight=token;reads.seen.set(source.agent_id,{revision,lease});
    const current=()=>currentOwner.current===owner&&residentLeases.current?.leases?.get(source.agent_id)===lease&&scope.current()&&inventoryRef.current.some(value=>value.agent_id===source.agent_id)&&(sessionId?client.getSnapshot().views[sessionId]?.checkpoint?.read_cut.domain_revisions.agents:undefined)===agentRevision&&(sessionId?client.getSnapshot().views[sessionId]?.snapshot?.workflows.revision:undefined)===workflowRevision;
    void client.request({method:'agent/status',params:{target,agent_id:source.agent_id}},'domain_page',undefined,current).then(({page})=>{
      if(!current()) return;
      const inspection = selection?.oneShot?.has(source.agent_id);
      if (!inspection && page.read_cut.domain_revisions.agents !== (agentRevision ?? '0')) return;
      const value=page.items[0]?.value;
      if(page.items.length!==1 || page.read_cut.conversation_id!==target.conversation_id || !value || typeof value!=='object' || !('agent_id' in value) || value.agent_id!==source.agent_id || !('activation_id' in value) || !('child_conversation_id' in value)) throw new Error('Native Agent header source changed.');
      const header=value as unknown as RuntimeClientAgent;
      setSelection(previous=>previous?.owner===owner&&previous.leases?.get(source.agent_id)===lease?{...previous,opened:previous.opened.map(agent=>agent.agent_id===header.agent_id?{ ...header, title: inspection ? source.title : header.title }:agent)}:previous);
      setHeaderError(previous=>previous?.owner===owner&&previous.lease===lease?undefined:previous);
    }).catch(error=>{if(current())setHeaderError({owner,id:source.agent_id,lease,message:String(error).slice(0,512)});}).finally(()=>{if(reads.flight===token)reads.flight=undefined;setHeaderWork(value=>value+1);});
  },[client,owner,target,scope,agentRevision,workflowRevision,agents,residents,headerWork]);
  const attached = useClientSelector(client, state => sessionId ? state.views[sessionId]?.attachment : undefined);
  const deleting = useClientSelector(client, state => sessionId ? state.views[sessionId]?.deleting : undefined);
  useLayoutEffect(() => () => client.agentMeters.retire(scope), [client, scope]);
  useLayoutEffect(() => { client.agentMeters.update(scope, id); }, [client, scope, agents, residents, attached, intent, deleting, id]);
  const readings = useSyncExternalStore(client.agentMeters.subscribe, client.agentMeters.getSnapshot);
  const metrics: Record<string, AgentStatistics> = {}, metricErrors: Record<string, string> = {};
  if (readings.scope === scope && scope.current()) for (const agent of inventory) {
    const reading = readings.readings.get(agent.agent_id);
    if (reading?.demand === meterDemand(agent)) {
      if (reading.metrics) metrics[agent.agent_id] = reading.metrics;
      if (reading.error) metricErrors[agent.agent_id] = reading.error;
    } else if (readings.blocked) metricErrors[agent.agent_id] = tx('agent:usage.reads-unresolved');
  }
  return <Context value={{ client, sessionId, onMode, agents: inventory, metrics, metricErrors, full: selection?.owner === owner && selection.full, headerError: headerError?.owner===owner&&selection?.leases?.get(headerError.id)===headerError.lease?headerError.message:undefined, oneShot: selection?.owner === owner ? selection.oneShot : undefined, openWorkflow, close, selected: inventory.find(agent => agent.agent_id === id), openAside: openAside ? (id: string) => { const agent = inventory.find(agent => agent.agent_id === id); if (agent) openAside(agent); } : undefined, opened: residents.map(value => agents.find(agent => agent.agent_id === value.agent_id) ?? value), open }}>{children}</Context>;
}
export function SubagentHeader({ title }: { title: string }) {
  const tx = useTranslation(), scope = useSubagents();
  if (!scope) return <span id="session-title">{title}</span>;
  const { selected, agents, open } = scope;
  const ancestors: RuntimeClientAgent[] = [];
  let parent = selected && agents.find(agent => agent.agent_id === selected.parent_agent_id);
  while (parent && !ancestors.includes(parent)) { ancestors.unshift(parent); parent = agents.find(agent => agent.agent_id === parent!.parent_agent_id); }
  const siblings = selected ? agents.filter(agent => agent.parent_agent_id === selected.parent_agent_id) : agents.filter(agent => !agents.some(parent => parent.agent_id === agent.parent_agent_id));
  const children = selected ? agents.filter(agent => agent.parent_agent_id === selected.agent_id) : [];
  return <div className={css.lineage}>
    {selected ? <><button className={css.parent} onClick={() => open()}>{title}</button><span className={css.separator}>/</span>
      {ancestors.map(agent => <span key={agent.agent_id} className={css.ancestor}><SubagentMenu agents={agents.filter(sibling => sibling.parent_agent_id === agent.parent_agent_id)} selected={agent.agent_id} label={agent.title} openTitle={() => open(agent.agent_id)} openChild={open}/><span className={css.separator}>/</span></span>)}
      <SubagentMenu agents={siblings} selected={selected.agent_id} label={selected.title} openChild={open}/>
      {!!children.length && <SubagentMenu agents={children} label={tx('common:subagents.count', { count: children.length })} openChild={open}/>}</>
      : <><span id="session-title" aria-label={tx(scope.sessionId ? 'agent:conversation-header.session-title' : 'agent:conversation-header.product-title')} className={css.title}>{title}</span>
        {!!siblings.length && <SubagentMenu agents={siblings} label={tx('common:subagents.count', { count: siblings.length })} openChild={open}/>}</>}
  </div>;
}
/** Eight resident Child views retain drafts until explicitly closed. */
export function SubagentSurface({ children, mode = 'chat' }: { children: ReactNode; mode?: 'chat' | 'trajectory' }) {
  const scope = useSubagents(), tx = useTranslation();
  return <>{scope?.full && <p role="status">{tx('common:subagents.view-capacity')}</p>}{scope?.headerError && <p role="alert">{scope.headerError}</p>}<div className={css.parentSurface} hidden={!!scope?.selected}>{children}</div>
    {scope?.opened.map(agent => <div key={`${scope.sessionId}:${agent.agent_id}`} hidden={scope.selected?.agent_id !== agent.agent_id} className={`${conversationCss.scrollBody} ${css.childSurface}`} data-conversation-scroll>
      <AgentCard inspection={scope.oneShot?.has(agent.agent_id)} close={() => scope.close(agent.agent_id)} onMode={scope.onMode} mode={mode} visible={scope.selected?.agent_id === agent.agent_id} agent={agent} metrics={scope.metrics[agent.agent_id]} metricsError={scope.metricErrors[agent.agent_id]} client={scope.client} sessionId={scope.sessionId}/>
    </div>)}</>;
}

/** Sidebar chat uses the same native Agent conversation and resident composer. */
export function SubagentAside({ id, visible }: { id: string; visible: boolean }) {
  const scope = useSubagents(), tx = useTranslation();
  const [mode, setMode] = useState<'chat' | 'trajectory'>('chat');
  const agent = scope?.agents.find(agent => agent.agent_id === id);
  if (!scope || !agent) return <p role="status">{tx('common:activity.unavailable')}</p>;
  return <section className={`conversation-panel ${conversationCss.body} ${conversationCss.embeddedBody}`} data-content-phase="active">
    <div className={conversationCss.tabs} role="tablist" aria-label={tx('agent:conversation-header.conversation-view')}>{(['chat', 'trajectory'] as const).map(value => <button type="button" key={value} role="tab" aria-selected={value === mode} className={`${conversationCss.tab} ${value === mode ? conversationCss.tabActive : ''}`} onClick={() => setMode(value)}>{tx(value === 'chat' ? 'agent:conversation-header.chat' : 'agent:conversation-header.trajectory')}</button>)}</div>
    <div className={`${conversationCss.scrollBody} ${css.aside}`} data-conversation-scroll><AgentCard inspection={scope.oneShot?.has(agent.agent_id)} onMode={setMode} mode={mode} embedded visible={visible} agent={agent} metrics={scope.metrics[id]} metricsError={scope.metricErrors[id]} client={scope.client} sessionId={scope.sessionId}/></div>
  </section>;
}
