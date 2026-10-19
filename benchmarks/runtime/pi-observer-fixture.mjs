// Independent expected output of the fixed local runtime_benchmark provider scripts.
// Never derive this plan from a final observed/canonical message.
import assert from 'node:assert/strict';
export const NOTE = 'deterministic-note-payload-6d41';
export function observerFixture(scenario, workspace) {
  assert(['short-small','short-large','stream','large','tool','cancel'].includes(scenario));
  const count = scenario === 'short-small' ? 40 : scenario === 'short-large' ? 96 : scenario === 'tool' ? 2 : 1;
  const assistant = ordinal => {
    assert(Number.isInteger(ordinal) && ordinal >= 0 && ordinal < count, 'unexpected fixture assistant message');
    if(scenario === 'tool' && ordinal === 0) return {content:[{type:'toolCall',id:'benchmark-read',name:'read',arguments:{path:`${workspace}/note.txt`}}]};
    const text = scenario === 'large' ? 'L'.repeat(32768)
      : scenario === 'stream' ? Array.from({length:128},(_,i)=>`stream-delta-${String(i).padStart(4,'0')} `).join('')
      : scenario === 'cancel' ? 'partial' : scenario === 'tool' ? NOTE : `reply ${String(ordinal).padStart(4,'0')}`;
    return {content:[{type:'text',text}],allowCommit:scenario !== 'cancel'};
  };
  const tool = id => {
    assert(scenario === 'tool' && id === 'benchmark-read', 'unexpected fixture tool identity');
    return {name:'read',text:NOTE};
  };
  return {assistant,tool};
}
