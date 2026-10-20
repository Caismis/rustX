import assert from 'node:assert/strict';

/** Measurement observer only. Never sends a request or alters a reply. */
export class DrainTracker {
  pending=new Set();cuts=new Map();latest;generation=0;epoch=0;events=[];
  open(){this.epoch++;this.pending.clear();this.cuts.clear();this.latest=undefined;this.generation++;this.events.push({op:'open',epoch:this.epoch,generation:this.generation});return this.epoch;}
  advance(cut){
    if(!cut)return;
    if(this.latest&&BigInt(cut.revision)<BigInt(this.latest.revision))return;
    this.latest=cut;
  }
  capture(page,outer){
    const cut=page?.read_cut??outer;if(!cut)return;
    for(const row of [...page?.records??[],...page?.updates??[]])this.cuts.set(row.id,cut);
    assert(this.cuts.size<=513,'drain observer exceeded the finite loaded-window workload');
  }
  observe(message,direction,epoch=this.epoch){
    if(epoch!==this.epoch)return;
    this.generation++;
    // Retain exactly the finite observer input,independently replayable without
    // copying large detail/content bodies or deriving product semantics.
    const page=p=>p&&({read_cut:p.read_cut,records:(p.records??[]).map(r=>({id:r.id})),updates:(p.updates??[]).map(r=>({id:r.id}))});
    const checkpoint=c=>c&&({read_cut:c.read_cut,snapshot:{trace:page(c.snapshot.trace)}});
    const fact=f=>({type:f.type,...(f.trace!=null?{trace:page(f.trace)}:{}),...(f.checkpoint?{checkpoint:checkpoint(f.checkpoint)}:{})});
    const input={...(message.id?{id:message.id}:{})};
    if(message.result?.type==='trace')input.result={type:'trace',page:page(message.result.page)};
    if(message.result?.type==='checkpoint')input.result={type:'checkpoint',checkpoint:checkpoint(message.result.checkpoint)};
    if(message.method==='session/event'){
      const e=message.params.event;
      input.method=message.method;input.params={event:e.type==='projection_transition'?{type:e.type,metadata:{read_cut:e.metadata?.read_cut},events:e.events.map(fact)}:fact(e)};
    }
    this.events.push({op:'message',epoch,generation:this.generation,direction,message:input});
    if(direction==='out'){if(message.id)this.pending.add(message.id);return;}
    if(message.id)this.pending.delete(message.id);
    const result=message.result;
    if(result?.type==='trace')this.capture(result.page);
    if(result?.type==='checkpoint'){
      const c=result.checkpoint;this.advance(c.read_cut);this.capture(c.snapshot.trace,c.read_cut);
    }
    if(message.method!=='session/event')return;
    const e=message.params.event,outer=e.metadata?.read_cut;
    const facts=e.type==='projection_transition'?e.events:[e];
    // Match the product's Trace obligations, not unrelated Session metadata.
    if(outer&&facts.some(f=>f.type==='trace_changed'||f.type==='checkpoint_advanced'
      ||f.type==='read_domains_updated'&&f.trace!=null))this.advance(outer);
    for(const f of facts){
      if(f.type==='read_domains_updated')this.capture(f.trace,outer);
      if(f.type==='checkpoint_advanced'){
        const c=f.checkpoint;this.advance(c.read_cut);this.capture(c.snapshot.trace,c.read_cut);
      }
    }
  }
  proof(rows){
    const cut=this.latest;
    const missing=[];
    if(cut)for(const id of rows.keys()){
      const rowCut=this.cuts.get(id);
      if(!rowCut||BigInt(rowCut.revision)<BigInt(cut.revision)
        ||JSON.stringify(rowCut.source_cut)!==JSON.stringify(cut.source_cut))missing.push(id);
    }
    return {ready:!!cut&&rows.size>0&&missing.length===0&&this.pending.size===0,
      generation:this.generation,transport_epoch:this.epoch,loaded_rows:rows.size,covered_rows:rows.size-missing.length,
      loaded_ids:[...rows.keys()],missing_ids:missing,pending_browser_requests:[...this.pending],target_cut:cut??null};
  }
}
