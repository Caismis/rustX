// Useful display fold over Pi Durable's production AgentEvent surface.
// Every assistant version is checked against an independent fixture plan.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
const hash = text => createHash('sha256').update(text).digest('hex');
function prefix(part, whole) {
  if(typeof whole === 'string') assert(typeof part === 'string' && whole.startsWith(part), 'observed value is outside fixture prefix');
  else if(Array.isArray(whole)) { assert(Array.isArray(part) && part.length <= whole.length); part.forEach((value,i)=>prefix(value,whole[i])); }
  else if(whole && typeof whole === 'object') {
    assert(part && typeof part === 'object' && !Array.isArray(part));
    for(const key of Object.keys(part)) { assert(Object.hasOwn(whole,key)); prefix(part[key],whole[key]); }
  } else assert.deepEqual(part,whole);
}
export class PiObserver {
  entries = []; message = null; tools = new Map();
  completedRuns = new Set(); explicitCompletedRuns = new Set(); activeRuns = new Set();
  toolResults = []; waiters = new Map(); counts = {};
  firstContentAt = null; lastAssistantText = ''; assistantOrdinal = 0;
  observedVersions = []; textHighWater = new Map(); failure = null;
  constructor({assistant,tool=()=>assert.fail('unexpected tool'),now=()=>performance.now()}) {
    assert.equal(typeof assistant,'function','independent expected assistant fixture required');
    this.expectedAssistant=assistant; this.expectedTool=tool; this.now=now;
  }
  observe(message,ordinal,complete,source) {
    if(!message || ['user','system'].includes(message.role)) return;
    const at=this.now();
    const witness={ordinal,role:message.role,source,complete,blocks:[],valid:false};
    if(message.toolCallId !== undefined) witness.toolCallId=message.toolCallId;
    this.observedVersions.push(witness);
    try {
      assert(Array.isArray(message.content),'observed content must be blocks');
      witness.blocks=message.content.map(block=>block.type === 'text' || block.type === 'thinking'
        ? {type:block.type,bytes:Buffer.byteLength(block[block.type]),sha256:hash(block[block.type])}
        : structuredClone(block));
      assert(['assistant','toolResult'].includes(message.role),'unexpected observed model role');
      const plan=message.role === 'assistant'?this.expectedAssistant(ordinal)
        : {content:[{type:'text',text:this.expectedTool(message.toolCallId).text}]};
      assert(Array.isArray(message.content) && message.content.length <= plan.content.length);
      for(let i=0;i<message.content.length;i++) {
        assert.equal(message.content[i].type,plan.content[i].type);
        prefix(message.content[i],plan.content[i]);
      }
      if(message.role === 'toolResult') {
        const expected=this.expectedTool(message.toolCallId);
        if(message.toolName !== undefined) assert.equal(message.toolName,expected.name);
        if(message.isError !== undefined) assert.equal(message.isError,false);
      }
      if(complete) { assert.notEqual(plan.allowCommit,false,'cancelled partial cannot become canonical'); assert.deepEqual(message.content,plan.content); }
      witness.valid=true;
    } catch(error) {
      witness.error=String(error);
      throw new Error(`invalid observed version ${JSON.stringify(witness)}`,{cause:error});
    }
    if(message.role !== 'assistant') return;
    const text=message.content.filter(block=>block.type === 'text').map(block=>block.text).join('');
    const length=Buffer.byteLength(text), prior=this.textHighWater.get(ordinal)??0;
    if(length > prior) {
      this.textHighWater.set(ordinal,length);
      if(this.firstContentAt === null) this.firstContentAt=at;
    }
    this.lastAssistantText=text;
  }
  observeEntry(entry,source) {
    for(const message of entry.model??[]) {
      this.observe(message,this.assistantOrdinal,true,source);
      if(message.role === 'assistant') this.assistantOrdinal++;
    }
  }
  append(entry,source) {
    assert(!this.entries.length || entry.id > this.entries.at(-1).id,'ordered entry identity');
    this.observeEntry(entry,source); this.entries.push(structuredClone(entry));
  }
  completeRun(id,explicit=false) {
    if(explicit) { assert(!this.explicitCompletedRuns.has(id),'duplicate explicit run completion'); this.explicitCompletedRuns.add(id); }
    this.completedRuns.add(id); this.activeRuns.delete(id);
    this.waiters.get(id)?.(); this.waiters.delete(id);
  }
  checkTool(id,text,source) {
    const witness={ordinal:this.assistantOrdinal,role:'tool_output',toolCallId:id,source,complete:false,
      blocks:[{type:'text',bytes:Buffer.byteLength(text),sha256:hash(text)}],valid:false};
    this.observedVersions.push(witness);
    try { assert(this.expectedTool(id).text.startsWith(text),'tool output outside independent fixture prefix'); witness.valid=true; }
    catch(error) { witness.error=String(error); throw new Error(`invalid observed version ${JSON.stringify(witness)}`,{cause:error}); }
  }
  consume(event) {
    if(this.failure) throw this.failure;
    try { this.fold(event); }
    catch(error) { this.failure=error; throw error; }
  }
  fold(event) {
    this.counts[event.type]=(this.counts[event.type]??0)+1;
    switch(event.type) {
      case 'snapshot': {
        // Native replacement is allowed. Previously checked versions and high
        // water marks survive even when an in-flight partial disappears.
        const nextRuns=new Set(event.run?.inputs??[]);
        for(const id of this.activeRuns) if(!nextRuns.has(id)) this.completeRun(id);
        this.activeRuns=nextRuns;
        this.assistantOrdinal=0;
        for(let i=0;i<event.entries.length;i++) {
          if(i) assert(event.entries[i-1].id < event.entries[i].id);
          this.observeEntry(event.entries[i],'snapshot_entry');
        }
        this.entries=structuredClone(event.entries);
        this.message=structuredClone(event.generation?.message??null);
        if(this.message) assert.equal(this.message.role,'assistant');
        this.observe(this.message,this.assistantOrdinal,false,'snapshot_generation');
        this.tools=new Map(event.tools.map(tool=>[tool.callId,structuredClone(tool)]));
        for(const [id,tool] of this.tools) { this.expectedTool(id); if(tool.output !== undefined) this.checkTool(id,tool.output,'snapshot_tool'); }
        break;
      }
      case 'run_start':
        for(const id of event.inputs) { assert(!this.activeRuns.has(id) && !this.completedRuns.has(id)); this.activeRuns.add(id); } break;
      case 'message_start':
        this.message=structuredClone(event.message); this.observe(this.message,this.assistantOrdinal,false,'message_start'); break;
      case 'message_update':
        assert(this.message,'update requires started message'); assert.equal(this.message.role,'assistant');
        for(const change of event.changes) {
          if(change.type === 'message') this.message=structuredClone(change.message);
          else {
            const i=change.contentIndex;
            assert(Number.isInteger(i) && i >= 0 && i <= this.message.content.length);
            if(['text_start','thinking_start','toolcall_start'].includes(change.type)) {
              assert.equal(i,this.message.content.length,'block start appends'); this.message.content.push(structuredClone(change.block));
            } else if(change.type === 'block') {
              assert(i < this.message.content.length); this.message.content[i]=structuredClone(change.block);
            } else {
              const block=this.message.content[i]; assert(block);
              if(change.type === 'text_delta' || change.type === 'thinking_delta') {
                const field=change.type === 'text_delta'?'text':'thinking';
                assert.equal(block.type,field); assert.equal(typeof block[field],'string'); block[field]+=change.delta;
              } else {
                assert.equal(change.type,'toolcall_delta'); assert.equal(block.type,'toolCall'); assert(change.path.length > 0);
                let target=block.arguments;
                for(const key of change.path.slice(0,-1)) { assert(Object.hasOwn(target,key)); target=target[key]; }
                const key=change.path.at(-1); assert(Object.hasOwn(target,key)); assert.equal(typeof target[key],'string'); target[key]+=change.delta;
              }
            }
          }
          this.observe(this.message,this.assistantOrdinal,false,`message_update:${change.type}`);
        }
        this.message.usage=structuredClone(event.usage); break;
      case 'message_end':
        this.append(event.entry,'message_end'); this.message=null; break;
      case 'entry_appended': this.append(event.entry,'entry_appended'); break;
      case 'tool_execution_start':
        assert(!this.tools.has(event.toolCallId)); this.expectedTool(event.toolCallId);
        this.tools.set(event.toolCallId,structuredClone(event)); break;
      case 'tool_execution_update': {
        const tool=this.tools.get(event.toolCallId); assert(tool,'tool update requires start');
        if(event.output) {
          tool.output='set' in event.output?event.output.set:(tool.output??'').slice(event.output.trimStart??0)+(event.output.append??'');
          this.checkTool(event.toolCallId,tool.output,'tool_execution_update');
        }
        tool.details=structuredClone(event.details); break;
      }
      case 'tool_execution_end':
        this.expectedTool(event.toolCallId);
        if(event.entry) {
          const expected=this.expectedTool(event.toolCallId);
          for(const message of event.entry.model??[]) if(message.role === 'toolResult') {
            assert.equal(message.toolCallId,event.toolCallId); assert.equal(message.toolName,expected.name);
            assert.equal(message.isError,false); assert.deepEqual(message.content,[{type:'text',text:expected.text}]);
            this.observe(message,this.assistantOrdinal,true,'tool_execution_end');
          }
          this.toolResults.push(structuredClone(event.entry));
        }
        this.tools.delete(event.toolCallId); break;
      case 'run_end': for(const id of event.inputs) this.completeRun(id,true); break;
    }
  }
  waitForRun(id) {
    if(this.failure) return Promise.reject(this.failure);
    if(this.completedRuns.has(id)) return Promise.resolve();
    assert(!this.waiters.has(id)); return new Promise(resolve=>this.waiters.set(id,resolve));
  }
}
