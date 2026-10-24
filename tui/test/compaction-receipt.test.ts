import assert from 'node:assert/strict';
import { it } from 'node:test';
import type { RuntimeClientCompactionView } from '../../protocol/app-server/v57.ts';
import { harness, nextRequest } from './support/app-server-harness.ts';
import { snapshot } from './support/fixtures.ts';

const receipt = (generation: string): RuntimeClientCompactionView => ({
  generation, checkpoint_id: `conv_8498648a-66ba-716f-8243-8cf147533e5${generation}`, activation_id: generation,
  tokens_before: {input_tokens:'8400',source:'estimated'}, estimated_tokens_after:'1900',
});

for (const unavailable of [false, true]) {
  it(`compaction returns its exact held native receipt with ${unavailable ? 'unavailable' : 'later'} published context`, async () => {
    const h = await harness(snapshot({context:{compaction_count:9,compaction_in_progress:false,latest_compaction:receipt('9')}}));
    try {
      if (unavailable) h.transport.notify({jsonrpc:'2.0',method:'session/observationUnavailable',params:{target:h.target,registration_id:'harness-registration-1',message:'held source'}});
      const before = h.session.state;
      const compacting = h.session.compactContext();
      const request = await nextRequest(h, 'context/compact');
      const original = receipt('7');
      h.transport.respond(request.id, {type:'context_compacted',compaction:original});
      assert.deepEqual(await compacting, original);
      assert.equal(h.session.state, before);
      assert.equal(h.transport.log.count('session/checkpoint'), 1);
      assert.equal(h.transport.log.count('conversation/checkpointPage'), 0);
      assert.equal(h.transport.log.count('context/compact'), 1);
      assert.equal(h.session.observationPhase, unavailable ? 'unavailable' : 'following');
    } finally { h.client.close(); }
  });
}

it('compact command prints the invocation receipt while a newer projection remains intact', async () => {
  const h = await harness(snapshot({context:{compaction_count:9,compaction_in_progress:false,latest_compaction:receipt('9')}}));
  try {
    const running = h.dispatcher.submit('/compact');
    const request = await nextRequest(h, 'context/compact');
    h.transport.respond(request.id, {type:'context_compacted',compaction:receipt('7')});
    const outcome = await running;
    assert.equal(outcome.kind, 'transient');
    if (outcome.kind !== 'transient') assert.fail('exact command receipt');
    assert.match(outcome.text, /generation 7: 8400 → 1900 tokens/);
    assert.equal(h.session.state.context.latest_compaction?.generation, '9');
    assert.equal(h.transport.log.count('session/checkpoint'), 1);
  } finally { h.client.close(); }
});

it('a native compaction error is returned without observation repair or operation replay', async () => {
  const h = await harness();
  try {
    const compacting = h.session.compactContext();
    const failure = assert.rejects(compacting, /native compaction busy/);
    const request = await nextRequest(h, 'context/compact');
    h.transport.respondError(request.id, {code:-32000,message:'native compaction busy'});
    await failure;
    assert.equal(h.transport.log.count('context/compact'), 1);
    assert.equal(h.transport.log.count('session/checkpoint'), 1);
  } finally { h.client.close(); }
});
