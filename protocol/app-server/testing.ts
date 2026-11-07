/** Test-only projection of a native Session/node fixture into Thread metadata. */
import type { SessionSnapshot, Thread } from './v73.ts';
export function threadFor(session: SessionSnapshot): Thread {
  return {id:session.active_conversation_id,sessionId:session.id,nodeId:session.active_node,name:null,archived:false,dynamicTools:[]};
}
