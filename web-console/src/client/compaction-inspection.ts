import type { CheckpointPage, CheckpointReadCut, TraceInspectionCompactionCall, TraceInspectionCompactionCalls } from '../../../protocol/app-server/v81';
import { checkpointReadCutKey } from '../../../protocol/app-server/content-identity';

const object = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);
const quantity = (value: unknown): value is string => typeof value === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(value) && BigInt(value) <= 18446744073709551615n;
const name = (value: unknown): value is string => typeof value === 'string' && value.length > 0 && value.length <= 4096;
const optionalQuantity = (value: unknown) => value == null || quantity(value);

/** Native continuations are never rounded into JavaScript's integer range. */
export function compactionPageOffset(value: string): number {
  if (!quantity(value) || BigInt(value) > BigInt(Number.MAX_SAFE_INTEGER)) throw Error('Native inspection continuation is outside the supported page range.');
  return Number(value);
}

function checkPage(page: CheckpointPage, cut: CheckpointReadCut, domain: string) {
  if (page.domain !== domain || checkpointReadCutKey(page.read_cut) !== checkpointReadCutKey(cut)
    || page.cursor !== cut.revision || !quantity(page.total) || page.items.length > 32
    || new TextEncoder().encode(JSON.stringify(page)).length > 786432) throw Error('Compaction inspection page changed.');
}

export function compactionCallsPage(page: CheckpointPage, cut: CheckpointReadCut, original: TraceInspectionCompactionCalls, after: string): TraceInspectionCompactionCalls {
  checkPage(page, cut, original.domain);
  if (page.total !== original.total || !quantity(after)) throw Error('Compaction Call frontier changed.');
  let previous = BigInt(after);
  const items = page.items.map(item => {
    const value = item.value;
    if (item.body_complete !== true || !object(value) || !name(value.call_id)
      || !quantity(value.started_sequence) || BigInt(value.started_sequence) <= previous
      || item.offset !== value.started_sequence || item.identity !== value.call_id
      || value.requests_domain !== `${original.domain}.${value.call_id}.requests` || !quantity(value.request_count)
      || !optionalQuantity(value.settled_sequence)) throw Error('Compaction Call identity changed.');
    previous = BigInt(value.started_sequence);
    if (value.outcome != null && !['completed', 'failed', 'cancelled'].includes(String(value.outcome))) throw Error('Compaction Call outcome changed.');
    for (const reference of [value.preceding_call, value.previous_summary]) {
      if (reference != null && (!object(reference) || !name(reference.call_id))) throw Error('Compaction Call causality changed.');
    }
    const strategy = value.strategy;
    if (strategy != null && strategy !== 'native' && strategy !== 'prompt'
      && !(object(strategy) && object(strategy.prompt_fallback) && name(strategy.prompt_fallback.operation_id))) throw Error('Compaction format strategy changed.');
    return value as unknown as TraceInspectionCompactionCall;
  });
  if (page.next_offset != null && (!quantity(page.next_offset) || BigInt(page.next_offset) !== previous || !items.length)) throw Error('Compaction Call page did not advance.');
  return { domain: original.domain, total: original.total, items, next_offset: page.next_offset };
}

/** Display metadata only: no request inputs, generated fragments or reasoning text. */
export interface CompactionPhysicalRequest {
  id: string; ordinal: number; wireModel: string; provider: string; model: string;
  profile: string | null | undefined; terminal: 'completed' | 'failed' | undefined;
  failure?: string; input?: string; output?: string; reasoning?: string;
  remoteUncertain: boolean;
}

export function compactionRequestsPage(page: CheckpointPage, cut: CheckpointReadCut, call: TraceInspectionCompactionCall, offset: string): { items: CompactionPhysicalRequest[]; next?: string | null } {
  checkPage(page, cut, call.requests_domain);
  if (page.total !== call.request_count || !quantity(offset)) throw Error('Compaction request frontier changed.');
  let expected = BigInt(offset);
  const items = page.items.map((item): CompactionPhysicalRequest => {
    const value = item.value;
    if (item.body_complete !== true || !object(value) || value.call_id !== call.call_id || !name(value.request_id)
      || item.identity !== value.request_id || BigInt(item.offset) !== expected++
      || typeof value.ordinal !== 'number' || !Number.isSafeInteger(value.ordinal) || value.ordinal < 0 || String(value.ordinal) !== item.offset
      || !name(value.model) || !object(value.invocation) || !name(value.invocation.provider) || !name(value.invocation.model_ref)) throw Error('Compaction physical request identity changed.');
    const binding = value.invocation;
    if (binding.profile != null && !name(binding.profile)) throw Error('Compaction Profile identity changed.');
    const terminal = value.local_terminal;
    if (terminal != null && terminal !== 'completed' && terminal !== 'failed') throw Error('Compaction physical terminal changed.');
    const usage = object(value.usage) ? value.usage : {};
    const details = object(usage.details) ? usage.details : {};
    if (![usage.input_tokens, usage.output_tokens, details.reasoning_tokens].every(optionalQuantity)) throw Error('Compaction usage measurement changed.');
    const evidence = object(value.request_evidence) ? value.request_evidence : {};
    return {
      id: value.request_id, ordinal: value.ordinal, wireModel: value.model,
      provider: String(binding.provider), model: String(binding.model_ref),
      profile: binding.profile == null ? binding.profile : String(binding.profile),
      terminal: terminal == null ? undefined : terminal,
      failure: typeof value.normalized_failure_kind === 'string' ? value.normalized_failure_kind : undefined,
      input: usage.input_tokens == null ? undefined : String(usage.input_tokens),
      output: usage.output_tokens == null ? undefined : String(usage.output_tokens),
      reasoning: details.reasoning_tokens == null ? undefined : String(details.reasoning_tokens),
      remoteUncertain: evidence.remote_execution_uncertainty === true,
    };
  });
  if (page.next_offset != null && (!quantity(page.next_offset) || BigInt(page.next_offset) !== expected || !items.length)) throw Error('Compaction request page did not advance.');
  if (expected > BigInt(page.total)) throw Error('Compaction request page exceeded its frontier.');
  return { items, next: page.next_offset };
}
