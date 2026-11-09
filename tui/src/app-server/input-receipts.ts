import type {InboundAcceptance} from '../../../protocol/app-server/v75.ts';

/** Local operation evidence, owned across transport replacements. Nothing here
 * can admit or replay input. The server alone proves durable acceptance. */
export class InputReceipts {
  readonly records = new Map<string, {authorityId:string; conversationId:string; inputId:string; acceptance?:InboundAcceptance}>();
  private key(authorityId:string,conversationId:string,inputId:string) { return JSON.stringify([authorityId,conversationId,inputId]); }
  reserve(authorityId:string,conversationId:string,inputId:string) {
    const key=this.key(authorityId,conversationId,inputId);
    if(!this.records.has(key)&&this.records.size>=64)throw new Error('Unconfirmed input capacity reached. Inspect input outcomes before submitting more.');
    this.records.set(key,{authorityId,conversationId,inputId});
  }
  release(authorityId:string,conversationId:string,inputId:string) { this.records.delete(this.key(authorityId,conversationId,inputId)); }
  accepted(authorityId:string,conversationId:string,inputId:string,acceptance:InboundAcceptance) {
    const key=this.key(authorityId,conversationId,inputId),record=this.records.get(key);
    if(record) { record.acceptance=acceptance; this.records.delete(key); }
  }
}
