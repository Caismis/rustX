import {expect,it} from 'vitest';
import {ChildObservation} from '../../protocol/app-server/child-observation';
import type {AgentObservation} from '../../protocol/app-server/v57';
import {checkpoint,snapshot} from './fixture';

it('T05/T18 Child repair retains Applied and rejects older or mismatched native checkpoints',()=>{
 const scopedCheckpoint=(state:ReturnType<typeof snapshot>,cursor:string)=>{const value=checkpoint(state,cursor);value.read_cut.activation_id='activation';return value;};
 const state=snapshot(),child=new ChildObservation('agent',state.conversation_id,'activation');
 const initial:Extract<AgentObservation,{type:'checkpoint'}>={type:'checkpoint',agent_id:'agent',conversation_id:state.conversation_id,activation_id:'activation',source:'live',cursor:{agent_id:'agent',conversation_id:state.conversation_id,activation_id:'activation',cursor:'5'},checkpoint:scopedCheckpoint(state,'5')};
 child.apply(initial);
 child.apply({type:'resync_required',agent_id:'agent',conversation_id:state.conversation_id,activation_id:'activation',source:'live',reason:'replay gap'});
 expect(child.appliedCursor?.cursor).toBe('5');
 for(const invalid of [
  {...initial,cursor:{...initial.cursor!,cursor:'4'},checkpoint:scopedCheckpoint(state,'4')},
  {...initial,cursor:{...initial.cursor!,cursor:'6'}},
  {...initial,checkpoint:{...initial.checkpoint,read_cut:{...initial.checkpoint.read_cut,conversation_id:'foreign'}}},
 ])expect(()=>child.apply(invalid)).toThrow();
 expect(child.appliedCursor?.cursor).toBe('5');expect(child.phase).toBe('resyncRequired');
 child.apply({...initial,cursor:{...initial.cursor!,cursor:'6'},checkpoint:scopedCheckpoint(state,'6')});
 expect(child.appliedCursor?.cursor).toBe('6');expect(child.phase).toBe('liveFollowing');
});

it.each(['stale revision', 'foreign source', 'source Journal', 'source Ledger', 'checkpoint revision', 'nested revision'] as const)('T14/T17 matching Child identity rejects %s before folding or Applied', failure => {
 const state=snapshot(),identity={agent_id:'agent',conversation_id:state.conversation_id,activation_id:'activation'};
 const scoped=(cursor:string)=>{const value=checkpoint(state,cursor);value.read_cut.activation_id=identity.activation_id;return value;};
 const baseline=scoped('4'),child=new ChildObservation(identity.agent_id,identity.conversation_id,identity.activation_id);
 child.apply({type:'checkpoint',...identity,source:'live',cursor:{...identity,cursor:'4'},checkpoint:baseline});
 const before=child.snapshot,cut=scoped('5').read_cut;
 cut.source_cut={read:{conversation_id:state.conversation_id,journal:'5',transcript:'0',mutation_revision:'0',ledger_through:'0',surface_revision:'0',transcript_total:'0',pending_through:'0',pending_total:'0'},ledger_through:'0',surface_revision:'0',transcript_total:'0',inherited_through:'0'};
 let event:import('../../protocol/app-server/v57').RuntimeClientEvent={type:'projection_transition',events:[{type:'runtime_shutdown'}],metadata:{read_cut:cut,content:[],collections:[]}};
 if(failure==='stale revision')cut.revision='4';
 if(failure==='foreign source')cut.source_cut.read.conversation_id='foreign';
 if(failure==='source Journal')cut.source_cut.read.journal='4';
 if(failure==='source Ledger')cut.source_cut.read.ledger_through='1';
 if(failure==='checkpoint revision'){const invalid=scoped('5');invalid.read_cut.revision='4';event={type:'checkpoint_advanced',transition:'invalid',checkpoint:invalid};}
 if(failure==='nested revision'&&event.type==='projection_transition')event.events=[{type:'checkpoint_advanced',transition:'invalid',checkpoint:scoped('4')}];
 expect(()=>child.apply({type:'events',after:{...identity,cursor:'4'},cursor:{...identity,cursor:'5'},events:[{cursor:'5',event}]})).toThrow('Child event cut evidence mismatch');
 expect(child.snapshot).toBe(before);expect(child.snapshot?.shutting_down).toBe(false);expect(child.checkpoint).toBe(baseline);expect(child.appliedCursor?.cursor).toBe('4');expect(child.phase).toBe('resyncRequired');
 child.apply({type:'checkpoint',...identity,source:'live',cursor:{...identity,cursor:'5'},checkpoint:scoped('5')});
 child.apply({type:'events',after:{...identity,cursor:'5'},cursor:{...identity,cursor:'7'},events:['6','7'].map(cursor=>({cursor,event:{type:'projection_transition' as const,events:[],metadata:{read_cut:scoped(cursor).read_cut,collections:[],content:[]}}}))});
 expect(child.appliedCursor?.cursor).toBe('7');expect(child.checkpoint?.read_cut.revision).toBe('7');expect(child.phase).toBe('liveFollowing');
});

it('T14/T18 Child checkpoint source evidence is checked before replacing its baseline',()=>{
 const state=snapshot(),identity={agent_id:'agent',conversation_id:state.conversation_id,activation_id:'activation'};
 const scoped=(cursor:string)=>{const value=checkpoint(state,cursor);value.read_cut.activation_id=identity.activation_id;return value;};
 const child=new ChildObservation(identity.agent_id,identity.conversation_id,identity.activation_id),baseline=scoped('4');
 child.apply({type:'checkpoint',...identity,source:'live',cursor:{...identity,cursor:'4'},checkpoint:baseline});
 const invalid=scoped('5');invalid.read_cut.source_cut={read:{conversation_id:'foreign',journal:'5',transcript:'0',mutation_revision:'0',ledger_through:'0',surface_revision:'0',transcript_total:'0',pending_through:'0',pending_total:'0'},ledger_through:'0',surface_revision:'0',transcript_total:'0',inherited_through:'0'};
 expect(()=>child.apply({type:'checkpoint',...identity,source:'live',cursor:{...identity,cursor:'5'},checkpoint:invalid})).toThrow('checkpoint source cut');
 expect(child.checkpoint).toBe(baseline);expect(child.appliedCursor?.cursor).toBe('4');
});
