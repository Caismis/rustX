// Useful display fold over Pi Durable's production AgentEvent surface.
// This is benchmark consumer state, never a replacement engine/provider.
import assert from 'node:assert/strict';
export class PiObserver {
  entries = [];
  message = null;
  tools = new Map();
  completedRuns = new Set();
  activeRuns = new Set();
  toolResults = [];
  waiters = new Map();
  counts = {};
  firstContentAt = null;
  lastAssistantText = '';
  constructor(now = () => performance.now()) { this.now = now; }
  append(entry) {
    assert(!this.entries.length || entry.id > this.entries.at(-1).id, 'ordered entry identity');
    this.entries.push(structuredClone(entry));
  }
  completeRun(id) {
    assert(!this.completedRuns.has(id), 'duplicate run completion');
    this.completedRuns.add(id); this.activeRuns.delete(id);
    this.waiters.get(id)?.(); this.waiters.delete(id);
  }
  consume(event) {
    this.counts[event.type] = (this.counts[event.type] ?? 0) + 1;
    switch(event.type) {
      case 'snapshot': {
        // These fixtures never compact or rewrite history. Overflow snapshots
        // may replace delivery, but cannot change already consumed entries.
        assert(event.entries.length >= this.entries.length, 'snapshot lost consumed entries');
        assert.deepEqual(event.entries.slice(0,this.entries.length),this.entries);
        const nextRuns=new Set(event.run?.inputs ?? []);
        for(const id of this.activeRuns) if(!nextRuns.has(id)) this.completeRun(id);
        this.activeRuns=nextRuns;
        for(let i=1;i<event.entries.length;i++) assert(event.entries[i-1].id < event.entries[i].id);
        this.entries = structuredClone(event.entries);
        this.message = structuredClone(event.generation?.message ?? null);
        this.tools = new Map(event.tools.map(t => [t.callId, structuredClone(t)]));
        break;
      }
      case 'run_start':
        for(const id of event.inputs) { assert(!this.activeRuns.has(id) && !this.completedRuns.has(id)); this.activeRuns.add(id); }
        break;
      case 'message_start': this.message = structuredClone(event.message); break;
      case 'message_update':
        assert(this.message, 'update requires started message');
        for(const change of event.changes) {
          if(change.type === 'message') { this.message = structuredClone(change.message); continue; }
          const i = change.contentIndex;
          assert(Number.isInteger(i) && i >= 0 && i <= this.message.content.length);
          if(['text_start','thinking_start','toolcall_start'].includes(change.type)) {
            assert.equal(i, this.message.content.length, 'block start appends');
            this.message.content.push(structuredClone(change.block));
          } else if(change.type === 'block') {
            assert(i < this.message.content.length); this.message.content[i] = structuredClone(change.block);
          } else {
            const block = this.message.content[i]; assert(block);
            if(change.type === 'text_delta' || change.type === 'thinking_delta') {
              const field = change.type === 'text_delta' ? 'text' : 'thinking';
              assert.equal(block.type, field); assert.equal(typeof block[field], 'string');
              block[field] += change.delta;
            } else {
              assert.equal(change.type, 'toolcall_delta'); assert.equal(block.type, 'toolCall');
              assert(change.path.length > 0);
              let target = block.arguments;
              for(const key of change.path.slice(0,-1)) { assert(Object.hasOwn(target,key)); target=target[key]; }
              const key=change.path.at(-1); assert(Object.hasOwn(target,key)); assert.equal(typeof target[key],'string');
              target[key] += change.delta;
            }
          }
        }
        this.message.usage = structuredClone(event.usage);
        break;
      case 'message_end': {
        const final = event.entry.model?.[0];
        if(this.message?.role === 'assistant' && final?.role === 'assistant') {
          // A terminal native message may complete the last unsent partial.
          // The visible prefix must be exact, never bless a changed byte.
          for(let i=0;i<this.message.content.length;i++) {
            const block=this.message.content[i], end=final.content[i]; assert(end); assert.equal(block.type,end.type);
            if(block.type==='text') assert(end.text.startsWith(block.text), 'visible text prefix changed');
            else if(block.type==='thinking') assert(end.thinking.startsWith(block.thinking));
            else if(block.type==='toolCall') {
              assert.equal(end.id,block.id); assert.equal(end.name,block.name);
              const prefix=(part,whole)=>{
                if(typeof part==='string') assert(typeof whole==='string' && whole.startsWith(part), 'tool argument prefix changed');
                else if(part && typeof part==='object') for(const key of Object.keys(part)) { assert(Object.hasOwn(whole,key)); prefix(part[key],whole[key]); }
                else assert.deepEqual(whole,part);
              };
              prefix(block.arguments,end.arguments);
            }
          }
        }
        this.append(event.entry); this.message=null; break;
      }
      case 'entry_appended': this.append(event.entry); break;
      case 'tool_execution_start':
        assert(!this.tools.has(event.toolCallId)); this.tools.set(event.toolCallId,structuredClone(event)); break;
      case 'tool_execution_update': {
        const tool=this.tools.get(event.toolCallId); assert(tool, 'tool update requires start');
        if(event.output) tool.output = 'set' in event.output ? event.output.set : (tool.output??'').slice(event.output.trimStart??0)+(event.output.append??'');
        tool.details=structuredClone(event.details); break;
      }
      case 'tool_execution_end':
        // Native not-offered/faulted calls may end without a start.
        if(event.entry) this.toolResults.push(structuredClone(event.entry));
        this.tools.delete(event.toolCallId); break;
      case 'run_end':
        for(const id of event.inputs) this.completeRun(id);
        break;
    }
    if(this.message?.role === 'assistant') this.lastAssistantText=this.message.content.filter(b=>b.type==='text').map(b=>b.text).join('');
    if(this.firstContentAt === null && this.message?.role === 'assistant' && this.message.content.some(b=>b.type==='text' && b.text.length)) this.firstContentAt=this.now();
  }
  waitForRun(id) {
    if(this.completedRuns.has(id)) return Promise.resolve();
    assert(!this.waiters.has(id)); return new Promise(resolve=>this.waiters.set(id,resolve));
  }
}
