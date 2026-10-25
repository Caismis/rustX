import {expect,it} from 'vitest';
import {checkpointSnapshot,foldRuntimeEvent,observationCheckpoint} from '../../protocol/app-server/projection';
import type {RuntimeClientEvent} from '../../protocol/app-server/v57';
import {checkpoint,snapshot} from './fixture';

const message={id:'original-inbound-4',source:'human' as const,content:[{type:'text' as const,text:'Queued during the Goal round'}]};
const enqueued:RuntimeClientEvent={type:'inbound_enqueued',sequence:'4',message,transcript_cursor:'12'};

it('a complete pending head and its positive enqueue fact equal the whole native checkpoint',()=>{
 const row={sequence:'4',revision:'2',deferred:true,message:{...message,content:[{type:'text' as const,text:'newer native body'}]}};
 const initial=checkpoint(snapshot(),'71');
 const expected=checkpoint({...snapshot(),inbound:{pending:[row]}},'72');
 const {snapshot:_,cursor:__,...metadata}=expected;
 const event:RuntimeClientEvent={type:'projection_transition',events:[{type:'pending_inbound_changed',pending:[row]},enqueued],metadata};
 const folded=foldRuntimeEvent(checkpointSnapshot(initial),event);
 expect(observationCheckpoint(folded,'72')).toEqual(expected);
 expect(folded.inbound.pending).toEqual([row]);
 expect(folded.inbound.pending![0]).toBe(row);
 expect(foldRuntimeEvent(folded,enqueued)).toBe(folded);
 const drained=foldRuntimeEvent(folded,{type:'inbound_drained',watermark:'4',count:1,message_ids:[message.id]});
 expect(drained.inbound).toEqual({pending:[],last_drain:{watermark:'4',count:1}});
});

it('a standalone enqueue adds its occurrence once and a conflicting identity cannot mutate it',()=>{
 const initial=checkpointSnapshot(checkpoint(snapshot()));
 const next=foldRuntimeEvent(initial,enqueued);
 expect(next.inbound.pending).toEqual([{sequence:'4',revision:'0',message}]);
 expect(initial.inbound.pending).toBeUndefined();
 expect(foldRuntimeEvent(next,enqueued)).toBe(next);
 expect(()=>foldRuntimeEvent(next,{...enqueued,message:{...message,id:'different-original'}})).toThrow('identity changed');
 expect(next.inbound.pending).toEqual([{sequence:'4',revision:'0',message}]);
});
