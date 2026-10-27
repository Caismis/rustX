import { attemptOwnerKey } from '../../../protocol/app-server/attempt-identity';
import type { InFlightBlock, MessageBlock, RuntimeClientTranscriptEntry, TurnProcessOutcome } from '../../../protocol/app-server/v61';
import type { StepActivity, StepCounts } from '../presentation/agent/StepGroup';

/** One member of a step group: an Assistant message's listed blocks, or a whole bodied entry. */
export interface StepMessageHead {
  readonly message: MessageBlock;
  readonly owner?: Parameters<typeof attemptOwnerKey>[0];
  readonly blocks?: InFlightBlock[];
  readonly independentBefore?: boolean;
}
export type StepMember = { readonly entry: RuntimeClientTranscriptEntry; readonly blocks?: readonly number[] }
  | { readonly head: StepMessageHead; readonly entry?: never; readonly blocks: readonly number[] };
export interface StepGroup {
  readonly key: string;
  readonly members: readonly StepMember[];
  /** Distinct-call category ranking, ties by first appearance; empty for reasoning only. */
  readonly counts: StepCounts;
}
/** A process keeps its message/block identity when publication adds a cursor. */
export const messageStepKey = (messageId: string, block: number) => JSON.stringify(['message', messageId, block]);
export const headStepSeatKey = (messageId: string) => JSON.stringify(['head', messageId]);
export const stepMemberKey = (member: StepMember) => 'head' in member ? messageStepKey(member.head.message.id, member.blocks[0]) : member.entry.item.type === 'message'
  ? messageStepKey(member.entry.item.message.id, member.blocks?.[0] ?? 0)
  : JSON.stringify(['entry', member.entry.cursor]);
/** What one transcript entry renders in place: a reply (its listed blocks) or the
 * whole group that starts in it. Blocks continuing an earlier group render there. */
export type StepPiece = { readonly kind: 'reply'; readonly blocks: readonly number[] } | { readonly kind: 'group'; readonly group: StepGroup };

/** Harness's tool-name classification, over rustX's native Tool names. */
export function stepActivity(name: string): StepActivity {
  if (name === 'read') return 'read';
  if (name === 'read_image') return 'readImage';
  if (name === 'grep' || name === 'glob') return 'search';
  if (name === 'write') return 'write';
  if (name === 'edit') return 'edit';
  if (name === 'bash' || name.startsWith('job_')) return 'commands';
  if (name === 'web_search') return 'webSearch';
  if (name === 'web_fetch') return 'webFetch';
  if (['subagent', 'list_agents', 'send_message', 'wait_agent', 'interrupt_agent'].includes(name)) return 'subagents';
  if (['todo', 'create_goal', 'update_goal', 'get_goal'].includes(name)) return 'plan';
  if (name === 'ask_user') return 'questions';
  return 'tools';
}

/** Page-cut Attempts stay ungrouped until their ownership is complete. */
const SETTLED: ReadonlySet<TurnProcessOutcome> = new Set(['completed', 'cancelled', 'failed', 'timed_out', 'limit_exceeded']);

/**
 * Harness process-group segmentation over native transcript order: reasoning,
 * Tool calls and other bodied records of one Attempt collect into a group
 * until a reply (visible Assistant text, refusal or media) or an independent
 * message closes it. Groups never cross an Attempt and never reorder entries.
 * @param entries - the loaded transcript page in canonical order.
 * @param bodied - whether a non-message entry renders a body of its own.
 * @param independentBefore - an independent annotation before this entry closes the preceding group.
 * @param hasDeferredText - positive native content evidence when the inline text is empty.
 * @returns each entry's in-place pieces; an entry absent from the map renders unchanged.
 */
export function stepGroups(entries: readonly RuntimeClientTranscriptEntry[], bodied: (entry: RuntimeClientTranscriptEntry) => boolean, independentBefore: (entry: RuntimeClientTranscriptEntry) => boolean = () => false,
  hasDeferredText: (messageId: string, blockIndex: number, text: string) => boolean = () => false, heads: readonly StepMessageHead[] = []) {
  const pieces = new Map<string, StepPiece[]>();
  let owner: string | undefined;
  let pending: { members: StepMember[]; calls: Map<string, StepActivity>; seat: StepPiece[] } | undefined;
  const flush = () => {
    if (pending) {
      const counts = new Map<StepActivity, number>();
      for (const kind of pending.calls.values()) counts.set(kind, (counts.get(kind) ?? 0) + 1);
      const first = pending.members[0];
      pending.seat.push({ kind: 'group', group: { key: stepMemberKey(first), members: pending.members,
        counts: [...counts].map(([kind, count]) => ({ kind, count })).sort((a, b) => b.count - a.count) } });
    }
    pending = undefined;
  };
  const add = (entry: RuntimeClientTranscriptEntry, seat: StepPiece[], block?: number) => {
    pending ??= { members: [], calls: new Map(), seat };
    const last = pending.members.at(-1);
    if (block === undefined) pending.members.push({ entry });
    else if (last && 'entry' in last && last.entry === entry && last.blocks) (last.blocks as number[]).push(block);
    else pending.members.push({ entry, blocks: [block] });
  };
  let ownerAttempt: string | undefined;
  for (const entry of entries) {
    if (independentBefore(entry)) flush();
    const process = entry.turn_process;
    const item = entry.item;
    // Interaction audits carry their Attempt but no process membership; they stay
    // inside the settled Attempt whose rows surround them.
    const audit = !process && (item.type === 'interaction_requested' || item.type === 'interaction_settled') && ownerAttempt != null && item.attempt_id === ownerAttempt;
    const key = audit ? owner : process && (SETTLED.has(process.outcome) || process.outcome === 'running') ? attemptOwnerKey(process) : undefined;
    if (key !== owner) { flush(); owner = key; ownerAttempt = key ? process?.attempt_id ?? undefined : undefined; }
    if (!key) continue;
    // Frozen partial prose stays in transcript order like an Assistant reply,
    // never inside a Tool/reasoning group or a raw recovery disclosure.
    if (item.type === 'publication_audit') { flush(); continue; }
    if (item.type === 'attempt_terminal' || item.type === 'message' && item.message.role === 'tool') continue;
    const seat: StepPiece[] = [];
    pieces.set(entry.cursor, seat);
    if (item.type !== 'message') { if (bodied(entry)) add(entry, seat); continue; }
    const message = item.message;
    if (message.role !== 'assistant') { flush(); pieces.delete(entry.cursor); continue; }
    message.content.forEach((block, index) => {
      if (block.type === 'reasoning') { if (block.text?.trim() || hasDeferredText(message.id, index, block.text ?? '')) add(entry, seat, index); return; }
      if (block.type === 'tool_call') { add(entry, seat, index); pending!.calls.set(block.id, stepActivity(block.name)); return; }
      if (block.type === 'text' && !block.text.trim() && !hasDeferredText(message.id, index, block.text)) return;
      flush();
      const reply = seat.at(-1);
      if (reply?.kind === 'reply') (reply.blocks as number[]).push(index);
      else seat.push({ kind: 'reply', blocks: [index] });
    });
  }
  // Native display heads have no transcript cursor. Their positively observed
  // Attempt membership survives canonical commitment until the read domain
  // catches up; independent/unknown heads still close the preceding group.
  for (const head of heads) {
    const key = head.owner && head.message.role === 'assistant' ? attemptOwnerKey(head.owner) : undefined;
    if (head.independentBefore || !key || owner !== key) flush();
    owner = key; ownerAttempt = head.owner?.attempt_id ?? undefined;
    if (!key || head.message.role !== 'assistant') continue;
    const seat: StepPiece[] = [];
    pieces.set(headStepSeatKey(head.message.id), seat);
    const blocks = head.blocks ?? head.message.content;
    blocks.forEach((block, index) => {
      const blockIndex = 'block_index' in block ? block.block_index : index;
      if (block.type === 'reasoning' || block.type === 'tool_call') {
        if (block.type === 'reasoning' && !block.text?.trim() && !hasDeferredText(head.message.id, blockIndex, block.text ?? '')) return;
        pending ??= { members: [], calls: new Map(), seat };
        const last = pending.members.at(-1);
        if (last && 'head' in last && last.head === head) (last.blocks as number[]).push(blockIndex);
        else pending.members.push({ head, blocks: [blockIndex] });
        if (block.type === 'tool_call') {
          const id = 'block_index' in block ? block.call_id : block.id;
          if (id && block.name) pending.calls.set(id, stepActivity(block.name));
        }
      } else {
        if (block.type === 'text' && !block.text.trim() && !hasDeferredText(head.message.id, blockIndex, block.text)) return;
        flush();
        const previous = seat.at(-1);
        if (previous?.kind === 'reply') (previous.blocks as number[]).push(blockIndex);
        else seat.push({ kind: 'reply', blocks: [blockIndex] });
      }
    });
  }
  flush();
  return pieces;
}
