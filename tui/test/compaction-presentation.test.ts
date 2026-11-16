import assert from 'node:assert/strict';
import { it } from 'node:test';
import type { RuntimeClientContextView, RuntimeClientProjectionHeadView } from '../src/protocol/app-server.ts';
import { expandPreference } from '../src/commands/outcome.ts';
import { compactionNotice, workingStatus } from '../src/ui/components/status.ts';
import { renderTranscript } from '../src/ui/components/transcript.ts';
import { CompactionSummary } from '../src/ui/components/compaction-summary.ts';
import { compactionKey, withAllCollapsed, withToggledCompaction } from '../src/ui/preferences.ts';
import { plainText, plainWidth } from '../src/ui/theme.ts';
import { stateOf, prefs, blockText } from './support/render.ts';
import { userMessage, assistantMessage } from './support/fixtures.ts';

type Operation = NonNullable<RuntimeClientContextView['compaction_operations']>[number];
const operation = (phase: Operation['status']['phase'], live = true): Operation => ({ live,
  status: { operation_id: 'summary-operation', admission_generation: '1', triggering_attempt_id: 'ended-attempt',
    waiting_attempt_id: null, phase, accepted_fragments: 21, diagnostic: null } });
const base = { compaction_count: 0, compaction_in_progress: false };
const head: RuntimeClientProjectionHeadView = {
  checkpoint_id: 'checkpoint-one', activation_id: '1', transcript_anchor: '2', base_frontier: '2', activation_ledger: '2',
  origin: { conversation_id: 'conversation-one', activation: '1', completed_journal: '10' },
  destination_installation: null, original_items: 1, derived_items: 1,
  summary_preview: { text: '## 保留的事实\n\n继续处理 CJK 与 Unicode。\n\n- next step', truncated: false },
};

it('uses one updating work label and a static Ready/terminal notice across Attempts', () => {
  const state = stateOf({ context: { ...base, compaction_operations: [operation('generating')] } });
  assert.equal(workingStatus(state), 'Summarizing previous context…');
  state.attempt = undefined;
  assert.equal(workingStatus(state), 'Summarizing previous context…');
  state.context.compaction_operations = [operation('ready')];
  assert.equal(workingStatus(state), undefined);
  assert.equal(compactionNotice(state), 'Context summary ready');
  state.context.compaction_operations = [operation('no_effect', false)];
  assert.equal(workingStatus(state), undefined);
  assert.equal(compactionNotice(state), 'Context summary not applied');
  state.context.compaction_operations = [operation('generating', false)];
  assert.equal(workingStatus(state), undefined);
  assert.match(compactionNotice(state)!, /no longer active/);
});

it('keeps background and foreground identities distinct in a single work label', () => {
  const state = stateOf({ context: { ...base, compaction_in_progress: true,
    compaction_progress: { operation_id: 'foreground', mode: 'overflow', accepted_fragments: 1, source_ranges: [], generation: null },
    compaction_operations: [operation('retrying')] } });
  assert.match(workingStatus(state)!, /Context limit reached.*Retrying context summary/);
  state.context.compaction_progress!.operation_id = 'summary-operation';
  assert.equal(workingStatus(state)!.includes('Retrying context summary'), false);
});

it('seats one collapsed checkpoint at its native transcript anchor and expands only by a view preference', () => {
  const state = stateOf({ messages: [userMessage('one', 'old input'), assistantMessage('two', 'old response'), userMessage('three', 'fresh input')],
    context: { ...base, projection_head: head } });
  const preferences = prefs();
  const before = JSON.stringify(state);
  const collapsed = renderTranscript(state, preferences);
  const marker = collapsed.findIndex(block => block.kind === 'compaction');
  assert.ok(marker >= 0);
  assert.ok(marker > collapsed.findIndex(block => blockText(block).includes('old response')));
  assert.ok(marker < collapsed.findIndex(block => blockText(block).includes('fresh input')));
  assert.equal(collapsed.filter(block => block.kind === 'compaction').length, 1);
  assert.doesNotMatch(blockText(collapsed[marker]!), /保留的事实/);
  const expanded = withToggledCompaction(preferences, compactionKey(head));
  assert.match(blockText(renderTranscript(state, expanded).find(block => block.kind === 'compaction')!), /保留的事实/);
  assert.equal(JSON.stringify(state), before);
  assert.equal(withAllCollapsed(expanded).expandedCompactions.size, 0);
  assert.deepEqual(expandPreference('compaction'), { kind: 'preference', preference: { type: 'expand_compaction', checkpointId: undefined } });
});

it('does not create a marker from readiness or seat a checkpoint outside the loaded window', () => {
  const state = stateOf({ messages: [userMessage('one', 'loaded')], context: { ...base, compaction_operations: [operation('ready')] } });
  assert.equal(renderTranscript(state, prefs()).some(block => block.kind === 'compaction'), false);
  state.context.projection_head = { ...head, transcript_anchor: '99' };
  assert.equal(renderTranscript(state, prefs()).some(block => block.kind === 'compaction'), false);
});

it('deduplicates a native history page against the active head and orders equal-anchor activations', () => {
  const active = { ...head, activation_id: '2' };
  const state = stateOf({ messages: [userMessage('one', 'old'), assistantMessage('two', 'response'), userMessage('three', 'fresh')],
    context: { ...base, projection_head: active } });
  const prior = { ...head, checkpoint_id: 'prior-checkpoint', activation_id: '1', transcript_anchor: '2' };
  const blocks = renderTranscript(state, prefs(), undefined, [prior, active, prior]);
  const markers = blocks.filter(block => block.kind === 'compaction');
  assert.equal(markers.length, 2);
  assert.equal(markers[0]!.head.checkpoint_id, prior.checkpoint_id);
  assert.equal(markers[1]!.head.checkpoint_id, head.checkpoint_id);
});

it('renders CJK, bounded summaries and exact token quantities within narrow terminal cells', () => {
  const receipt = { operation_id: 'op', generation: '1', checkpoint_id: head.checkpoint_id, activation_id: '1',
    tokens_before: { input_tokens: '9007199254740993', source: 'estimated' as const }, estimated_tokens_after: '100' };
  for (const width of [4, 12, 40, 80]) {
    for (const expanded of [false, true]) {
      const lines = new CompactionSummary({ ...head, summary_preview: { ...head.summary_preview!, truncated: true } }, expanded, receipt).render(width);
      assert.ok(lines.every(line => plainWidth(line) <= width), `terminal width ${width}`);
    }
  }
  const text = new CompactionSummary(head, false, receipt).render(120).map(plainText).join('\n');
  assert.match(text, /~9,007,199,254,740,993/);
  assert.doesNotMatch(text, /frontier|Origin:|conversation-one/);
});
