import type {ThreadStatusEntry} from './v74.ts';

/** Authority is established by the owning connection. Within that authority,
 * one native publication revision orders both snapshots and notifications. */
export function newerThreadStatus(previous:ThreadStatusEntry|undefined,next:ThreadStatusEntry):ThreadStatusEntry {
  return previous?.threadId===next.threadId&&BigInt(previous.revision)>BigInt(next.revision)?previous:next;
}

export function threadStatusText(entry:ThreadStatusEntry|undefined):string {
  if(!entry)return 'Status unavailable';
  switch(entry.status.type) {
    case 'notLoaded':return 'Not loaded';
    case 'idle':return 'Idle';
    case 'systemError':return 'System error';
    case 'active': {
      const waits=entry.status.activeFlags.map(flag=>flag==='waitingOnApproval'?'approval':'user input');
      return waits.length?`Waiting for ${waits.join(' and ')}`:'Running';
    }
  }
}

/** Finite early-notification evidence for snapshots introducing previously
 * unknown Threads. Eviction forgets a revision floor, never a fake idle.
 * This cache owns no runtime, membership, execution or subscription. */
export class ThreadStatusNotifications {
  private readonly entries = new Map<string,{entry:ThreadStatusEntry;available:boolean;received:number}>();
  private forgottenThrough = 0n;
  private sequence = 0;
  // Visible proofs can outlive the finite early-notification cache. Receipt
  // evidence follows those existing objects without retaining their lifetime.
  private receipts = new WeakMap<ThreadStatusEntry,{received:number;revoked:boolean}>();
  checkpoint():number {return this.sequence;}
  latestSince(threadId:string,checkpoint:number,...retained:(ThreadStatusEntry|null|undefined)[]):ThreadStatusEntry|undefined {
    const proof=this.entries.get(threadId);
    let latest=proof?.available&&proof.received>checkpoint?proof.entry:undefined;
    for(const entry of retained){
      const receipt=entry&&this.receipts.get(entry);
      if(entry?.threadId!==threadId||!receipt||receipt.revoked||receipt.received<=checkpoint)continue;
      if(proof&&!proof.available&&BigInt(proof.entry.revision)>=BigInt(entry.revision))continue;
      latest=newerThreadStatus(latest,entry);
    }
    return latest&&proof?.available?newerThreadStatus(latest,proof.entry):latest;
  }
  private retain(entry:ThreadStatusEntry,available:boolean,received:number):void {
    this.entries.delete(entry.threadId);
    this.entries.set(entry.threadId,{entry,available,received});
    if(this.entries.size>128){
      const [id,forgotten]=this.entries.entries().next().value!;
      this.entries.delete(id);
      if(BigInt(forgotten.entry.revision)>this.forgottenThrough)this.forgottenThrough=BigInt(forgotten.entry.revision);
    }
  }
  observe(entry:ThreadStatusEntry):boolean {
    const previous=this.entries.get(entry.threadId);
    if(previous&&BigInt(previous.entry.revision)>=BigInt(entry.revision))return false;
    if(!previous&&BigInt(entry.revision)<=this.forgottenThrough)return false;
    this.receipts.set(entry,{received:++this.sequence,revoked:false});
    this.retain(entry,true,this.sequence);return true;
  }
  snapshot(entry:ThreadStatusEntry,previous?:ThreadStatusEntry):ThreadStatusEntry|undefined {
    let proof=this.entries.get(entry.threadId);
    if(previous?.threadId===entry.threadId&&(!proof||BigInt(previous.revision)>BigInt(proof.entry.revision))){
      this.retain(previous,true,0);proof=this.entries.get(entry.threadId);
    }
    if(proof&&BigInt(proof.entry.revision)>=BigInt(entry.revision))return proof.available?proof.entry:undefined;
    if(!proof&&BigInt(entry.revision)<this.forgottenThrough)return undefined;
    // A newer snapshot includes the recovery already certified by the older
    // available proof. Preserve its cut; this is not a new notification.
    const receipt=proof?.available?this.receipts.get(proof.entry):undefined;
    if(receipt&&!receipt.revoked)this.receipts.set(entry,receipt);
    this.retain(entry,true,0);return entry;
  }
  latest(threadId:string):ThreadStatusEntry|undefined {const proof=this.entries.get(threadId);return proof?.available?proof.entry:undefined;}
  // Revocation retains the exact revision fence. A late pre-failure event must
  // not become a new recovery proof merely because the visible status vanished.
  forget(threadId:string,previous?:ThreadStatusEntry|null):void {
    const proof=this.entries.get(threadId);
    const entry=previous?.threadId===threadId?newerThreadStatus(proof?.entry,previous):proof?.entry;
    for(const value of [proof?.entry,previous?.threadId===threadId?previous:undefined]){
      const receipt=value&&this.receipts.get(value);if(receipt)receipt.revoked=true;
    }
    if(entry)this.retain(entry,false,0);
  }
  clear():void {this.entries.clear();this.forgottenThrough=0n;this.receipts=new WeakMap();}
}
