/** Test-only projection of a native Session/node fixture into Thread metadata. */
import type { SessionSnapshot, SessionSummary, Thread, ThreadStatusEntry } from './v77.ts';
export function threadFor(session: SessionSnapshot): Thread {
  return {id:session.active_conversation_id,sessionId:session.id,nodeId:session.active_node,name:null,archived:false,dynamicTools:[],status:{type:'notLoaded'},statusRevision:'0'};
}

export function summaryFor<T extends SessionSummary>(summary:T,threadId=summary.active_node.replace(/^node_/,'conv_')):T&{thread_id:string;thread_status:ThreadStatusEntry} {
  return Object.assign({thread_id:threadId,thread_status:{threadId,status:{type:'notLoaded' as const},revision:'0',runtimeIncarnation:null}},summary);
}
