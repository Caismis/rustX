import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import { interactionBody, readInteractionBody } from '../../../protocol/app-server/interaction';
import { observationCheckpoint } from '../../../protocol/app-server/projection';
import { OutcomeUncertain } from './errors';
import { interactionKey, type ClientView, type ConnectionRequest, type SessionView } from './contracts';
import type { AttachmentTarget, CheckpointReadCut, InteractionRef, InteractionResponse, RuntimeClientInteractionHeader } from '../../../protocol/app-server/v57';
import type { SessionLifecycles } from './session-lifecycle/system';
import type { SessionReads } from './session-reads';

/** What human-in-the-loop interactions read from and write to their client. */
export interface InteractionPort {
  request: ConnectionRequest;
  state(): Readonly<ClientView>;
  current(generation: number): boolean;
  setSession(id: string, patch: Partial<Pick<SessionView, 'interactionDetail' | 'interactionPage'>>): void;
  publish(patch: Partial<Pick<ClientView, 'uncertain' | 'interactionOperations'>>): void;
  target(id: string): AttachmentTarget;
  observe: SessionLifecycles['observe'];
  domainReader: SessionReads['domainReader'];
  refresh(id: string): Promise<void>;
}

/** Pending native interactions of attached Sessions: bounded header pages and
 * exact bodies at one read cut, and exactly-once responses whose local
 * in-flight/uncertain evidence only a fresh authoritative snapshot resolves. */
export class Interactions {
  constructor(private readonly port: InteractionPort) {}
  pendingInteractions(id:string) {
    const view=this.port.state().views[id],loaded=view?.interactionPage;
    if(loaded&&sameTarget(view.target,loaded.target)&&this.interactionCutCurrent(view,loaded.page.read_cut))return loaded.rows;
    return view?.snapshot?.pending_interactions??[];
  }
  interactionBody(id:string,header:RuntimeClientInteractionHeader) {
    const view=this.port.state().views[id],detail=view?.interactionDetail;
    if(detail&&sameTarget(view.target,detail.target)&&this.interactionCutCurrent(view,detail.cut)&&interactionKey(detail.body.interaction)===interactionKey(header.interaction))return detail.body;
    return interactionBody(header);
  }
  async loadInteractionBody(id:string,index:number,ownsView:()=>boolean=()=>true) {
    const view=this.port.state().views[id],observation=this.port.observe(id),header=this.pendingInteractions(id)[index];
    if(!observation||!header||!view.checkpoint)throw new Error('Interaction observation is unavailable.');
    const viewport=view.interactionPage;
    const selected=viewport&&sameTarget(view.target,viewport.target)&&this.interactionCutCurrent(view,viewport.page.read_cut)?viewport:undefined;
    const cut=selected?.page.read_cut??view.checkpoint.read_cut,target=observation.target;
    const offset=Number(header.source_offset);
    if(!Number.isSafeInteger(offset)||offset<0)throw new Error("Invalid native interaction source offset.");
    const current=()=>ownsView()&&observation.current()&&!!this.port.state().views[id]&&this.interactionCutCurrent(this.port.state().views[id],cut)
      &&this.pendingInteractions(id)[index]?.request_id===header.request_id;
    const {page}=await this.port.request({method:'conversation/checkpointPage',params:{target,agent_id:null,cut,domain:'pending_interactions',offset,limit:1}},'checkpoint_page',undefined,current);
    if(page.items.length!==1)throw new Error('Interaction body is unavailable.');
    const body=await readInteractionBody(page,page.items[0]!,header,this.port.domainReader(target,current));
    if(current())this.port.setSession(id,{interactionDetail:{target,cut,body}});
  }
  async interactionValueChunk(id:string,index:number,path:string[],offset:number,ownsView:()=>boolean) {
    const view=this.port.state().views[id],observation=this.port.observe(id),header=this.pendingInteractions(id)[index];
    if(!observation||!header||!view.checkpoint)throw new Error('Interaction observation is unavailable.');
    const selected=view.interactionPage&&sameTarget(view.target,view.interactionPage.target)&&this.interactionCutCurrent(view,view.interactionPage.page.read_cut)?view.interactionPage:undefined;
    const cut=selected?.page.read_cut??view.checkpoint.read_cut,item_offset=Number(header.source_offset);
    if(!Number.isSafeInteger(item_offset)||item_offset<0)throw new Error("Invalid native interaction source offset.");
    const current=()=>ownsView()&&observation.current()&&!!this.port.state().views[id]&&this.interactionCutCurrent(this.port.state().views[id],cut)&&this.pendingInteractions(id)[index]?.request_id===header.request_id;
    const {chunk}=await this.port.request({method:'conversation/checkpointValueRead',params:{target:observation.target,agent_id:null,cut,domain:'pending_interactions',item_offset,path,offset,max_bytes:65536}},'checkpoint_value_chunk',undefined,current);
    if(!current()||JSON.stringify(chunk.cut)!==JSON.stringify(cut)||chunk.domain!=='pending_interactions'||chunk.item_offset!==item_offset||JSON.stringify(chunk.path)!==JSON.stringify(path)||chunk.offset!==offset)throw new Error('Interaction range identity changed.');
    if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
    const bytes=Uint8Array.from(atob(chunk.bytes_base64),value=>value.charCodeAt(0));
    if(bytes.length>65536||!Number.isSafeInteger(chunk.readable_end)||offset+bytes.length>chunk.readable_end||offset<chunk.readable_end&&!bytes.length||(chunk.next_offset??null)!==(offset+bytes.length<chunk.readable_end?offset+bytes.length:null))throw new Error('Interaction range failed to advance exactly.');
    return {bytes,next:chunk.next_offset??undefined,readableEnd:chunk.readable_end};
  }
  private interactionCutCurrent(view:SessionView,cut:CheckpointReadCut) {
    const current=view.checkpoint?.read_cut;
    return !!current&&current.conversation_id===cut.conversation_id&&current.activation_id===cut.activation_id
      &&(cut.domain_revisions.pending_interactions!==undefined
        ?current.domain_revisions.pending_interactions===cut.domain_revisions.pending_interactions:current.revision===cut.revision);
  }
  async interactionPage(id:string,offset:number) {
    const view=this.port.state().views[id],observation=this.port.observe(id),checkpoint=view?.checkpoint;
    if(!observation||!checkpoint||!Number.isSafeInteger(offset)||offset<0)throw new Error('Interaction observation is unavailable.');
    const cut=checkpoint.read_cut,target=observation.target;
    const current=()=>observation.current()&&!!this.port.state().views[id]&&this.interactionCutCurrent(this.port.state().views[id],cut);
    const {page}=await this.port.request({method:'conversation/checkpointPage',params:{target,agent_id:null,cut,domain:'pending_interactions',offset,limit:32}},'checkpoint_page',undefined,current);
    if(page.items.length>32)throw new Error('Interaction page exceeds its item bound.');
    const rows=page.items.map(item=>{if(item.value==null)throw new Error('Interaction descriptor is missing.');return item.value as RuntimeClientInteractionHeader;});
    if(!current())throw new Error('Interaction observation was replaced.');
    this.port.setSession(id,{interactionPage:{target,page,rows,offset}});
  }
  async answer(id: string, interaction: InteractionRef, response?: InteractionResponse) {
    const key = interactionKey(interaction);
    if (this.port.state().interactionOperations[key]) throw new Error('Response already in flight or uncertain. Refresh authoritative state.');
    const target = this.port.target(id);
    if (!this.pendingInteractions(id).some(item => interactionKey(item.interaction) === key)) throw new Error('Interaction is no longer pending.');
    const generation = this.port.state().generation;
    this.port.publish({ interactionOperations: { ...this.port.state().interactionOperations, [key]: { sessionId: id, status: 'in-flight' } } });
    let acknowledged = false;
    try {
      await this.port.request(response ? { method: 'interaction/respond', params: { target, interaction, response } }
        : { method: 'interaction/cancel', params: { target, interaction } }, 'interaction_settled');
      if (!this.port.current(generation) || !sameTarget(this.port.state().views[id]?.target, target)) return;
      acknowledged = true;
      this.port.publish({ interactionOperations: { ...this.port.state().interactionOperations, [key]: { sessionId: id, status: 'acknowledged' } } });
      await this.port.refresh(id);
    } catch (error) {
      if (this.port.current(generation) && !(error instanceof OutcomeUncertain) && !acknowledged) {
        const operations = { ...this.port.state().interactionOperations }; delete operations[key];
        this.port.publish({ interactionOperations: operations });
        await this.port.refresh(id).catch(() => {});
      }
      throw error;
    }
  }
  /** Only an authoritative fresh snapshot can establish an interaction's absence. */
  reconcile(id: string) {
    const snapshot = this.port.state().views[id].snapshot!;
    const frontier=observationCheckpoint(snapshot,this.port.state().views[id].cursor??'0')?.collections.find(value=>value.domain==='pending_interactions');
    // A bounded head cannot establish absence from the complete native set.
    if(frontier&&BigInt(frontier.total)>BigInt(snapshot.pending_interactions?.length??0))return;
    const pending = new Set(snapshot.pending_interactions?.map(item => interactionKey(item.interaction)));
    const operations = { ...this.port.state().interactionOperations };
    // Only an authoritative fresh snapshot can establish absence. Include routed
    // children by using the recorded Session route, not just root ConversationId.
    const resolved = this.port.state().uncertain.filter(item => item.sessionId === id && item.interactionKey && !pending.has(item.interactionKey));
    for (const item of resolved) delete operations[item.interactionKey!];
    for (const key of Object.keys(operations)) {
      if (operations[key].sessionId === id && !pending.has(key)) delete operations[key];
    }
    this.port.publish({ uncertain: this.port.state().uncertain.filter(item => !resolved.includes(item)), interactionOperations: operations });
  }
  /** Native settlement or removal ends local evidence of those interactions. */
  settled(keys: ReadonlySet<string>) {
    const state = this.port.state();
    this.port.publish({ interactionOperations: Object.fromEntries(Object.entries(state.interactionOperations).filter(([key]) => !keys.has(key))),
      uncertain: state.uncertain.filter(value => !value.interactionKey || !keys.has(value.interactionKey)) });
  }
}
