import type { Server } from '../fixture';
import type { ChangesSummary, RuntimeClientSnapshot } from '../../../protocol/app-server/v60';

export function workspaceChangesFixture(server:Server,current:RuntimeClientSnapshot) {
  const completed=structuredClone(current);
  completed.attempt=null;
  completed.conversation_id='conversation-A';
  const answer=completed.messages.find(message=>message.role==='assistant')!;
  if(answer.role!=='assistant')throw Error('missing answer');
  answer.content=[{type:'text',text:'## Changes complete\n\n'+Array.from({length:45},(_,i)=>`- Verified source change ${i+1} against the current workspace.`).join('\n')+'\n\nThe implementation and relevant checks are complete.'}];
  completed.transcript.entries![1]={cursor:'2',item:{type:'message',message:answer},completed_response:{
    closing_locator:{type:'by_message_id',message_id:answer.id},deferred_fields:[],closing_message_id:answer.id,
    origin:{conversation_id:'conversation-A',attempt_id:'attempt-1',attempt_id_source:null,closing_message_id:answer.id},surface_revision:'2',completed_at:'2026-10-22T05:00:00Z',
  }};
  let summary:ChangesSummary|null=null;
  const summarize=(count:number):ChangesSummary=>({files:Array.from({length:count},(_,i)=>({path:`/workspace/A/src/file-${i+1}.ts`,display:`src/file-${i+1}.ts`,added:2,deleted:1,binary:false,oversized:false})),total:count,added:count*2,deleted:count});
  server.handlers.set('conversation/changes',()=>({type:'workspace_changes',summary}));
  server.handlers.set('conversation/changeDiff',request=>({type:'workspace_change_diff',comparison:request.method==='conversation/changeDiff'&&request.params.index<6 ? {
    hunks:[{old_start:1,new_start:1,old_text:'export const value = 1;\n',new_text:'export const value = 2;\nexport const enabled = true;\n'}],binary:false,oversized:false,truncated:false,
  }:null}));
  current.messages=current.messages.filter(message=>message.role==='user');current.transcript.entries=current.transcript.entries!.slice(0,1);
  current.attempt={attempt_id:'attempt-1',phase:{type:'running'},turn:1,in_flight:{message_id:answer.id,blocks:[{type:'text',block_index:0,text:'Updating workspace files…'}]}};
  if(new URLSearchParams(location.search).has('complete')) {summary=summarize(6);Object.assign(current,structuredClone(completed));}
  window.workspaceChangesFixture={
    async settle(count=6) {
      summary=summarize(count);
      await server.update('A',completed);
    },
    requests:()=>server.requests.map(value=>value.request).filter(request=>request.method==='conversation/changes'||request.method==='conversation/changeDiff'),
  };
}
declare global { interface Window { workspaceChangesFixture:{settle(count?:number):Promise<void>;requests():unknown[]} } }
