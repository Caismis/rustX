import type { AgentStatusView, MessageBlock, RuntimeClientSnapshot, RuntimeClientStatusSection, RuntimeClientTodoStatusTask } from '../../../protocol/app-server/v25';

/** Agent Status is historical, request-scoped model context. Nothing here is a
 * current-state authority: current Todo is `snapshot.todos`, current Goal is
 * `snapshot.goal`, current Queue is `snapshot.inbound`. This module only decides
 * *where* one composition belongs and *what* its typed sections say. */

/** Where one composed Agent Status belongs, per the runtime's own published facts.
 *
 * `FreshInbound` wins unconditionally when both opportunities exist: an exact
 * message identity is a stronger fact than a transcript position, and choosing it
 * without looking at what is loaded is what keeps a doubly-eligible composition
 * from being drawn twice — or drawn in the wrong place while its target is off-page. */
export type AgentStatusAnchor =
  | { kind: 'inbound_message'; messageId: string }
  | { kind: 'transcript_position'; cursor: string }
  | { kind: 'unplaced' };

export function agentStatusAnchor(status: AgentStatusView): AgentStatusAnchor {
  const fresh = status.opportunities.fresh_inbound;
  if (fresh) return { kind: 'inbound_message', messageId: fresh.target_message_id };
  const anchor = status.opportunities.post_tool_batch?.transcript_anchor;
  if (anchor != null) return { kind: 'transcript_position', cursor: anchor };
  return { kind: 'unplaced' };
}

/** One composition at its anchor, carrying the runtime's composition ordinal.
 * `order` orders annotations that resolve to one transcript position; identity
 * is always `status.status_message_id`, never this number and never an index. */
export interface AnchoredStatus { status: AgentStatusView; order: number }
export interface AgentStatusPlacement {
  byMessageId: Map<string, AnchoredStatus[]>;
  byCursor: Map<string, AnchoredStatus[]>;
}

/** Groups the authoritative bounded window by single anchor, in composition order.
 *
 * Repeated observation of one `status_message_id` collapses to the first: a
 * composition is one fact however many times a snapshot or a folded event
 * restates it, and a render key alone would not prevent a second annotation.
 * An unplaced composition enters no bucket, so it is never drawn in ordinary
 * transcript UI; the runtime still owns it and the Inspector still shows it. */
export function agentStatusPlacement(statuses?: readonly AgentStatusView[]): AgentStatusPlacement {
  const placement: AgentStatusPlacement = { byMessageId: new Map(), byCursor: new Map() };
  const seen = new Set<string>();
  let order = 0;
  for (const status of statuses ?? []) {
    if (seen.has(status.status_message_id)) continue;
    seen.add(status.status_message_id);
    const anchor = agentStatusAnchor(status);
    const anchored = { status, order: order++ };
    if (anchor.kind === 'inbound_message') append(placement.byMessageId, anchor.messageId, anchored);
    else if (anchor.kind === 'transcript_position') append(placement.byCursor, anchor.cursor, anchored);
  }
  return placement;
}

function append(index: Map<string, AnchoredStatus[]>, key: string, anchored: AnchoredStatus) {
  const bucket = index.get(key);
  if (bucket) bucket.push(anchored); else index.set(key, [anchored]);
}

/** The compositions drawn after one transcript entry, in runtime composition order.
 * A message-anchored and a position-anchored composition can resolve to the same
 * entry; both orders come from one window, so they interleave correctly. */
export function statusesAt(placement: AgentStatusPlacement, entry: { messageId?: string; cursor: string }): AgentStatusView[] {
  const attached = (entry.messageId != null ? placement.byMessageId.get(entry.messageId) : undefined) ?? [];
  const standalone = placement.byCursor.get(entry.cursor) ?? [];
  if (!attached.length && !standalone.length) return [];
  return [...attached, ...standalone].sort((a, b) => a.order - b.order).map(anchored => anchored.status);
}

/** The canonical Agent Status Context message, by typed kind — never by text.
 * It is request-scoped model history with its own annotation presentation, so it
 * must not reappear through generic Context / "Current context" chrome as a second
 * ordinary-chat representation or as purported live state. */
export function isAgentStatusContext(message: MessageBlock): boolean {
  if (message.role !== 'user') return false;
  const kind = message.kind;
  return !!kind && typeof kind === 'object' && 'context' in kind
    && typeof kind.context === 'object' && kind.context !== null && 'agent_status' in kind.context;
}

/** One typed section reduced to presentation values. `kind` mirrors the runtime's
 * section identity so a caller styles by section without re-deciding which it is. */
export interface AgentStatusFacet {
  kind: RuntimeClientStatusSection['type'];
  /** The one-line compact form: `10:42 CST`, `todo 3`, `background 2`. */
  compact: string;
  label: string;
  /** Detail lines, already bounded. Never empty. */
  values: string[];
}

const VALUE_LIMIT = 64;
const DETAIL_LIMIT = 160;
const DETAIL_ENTRY_LIMIT = 6;

/** The typed facets of one composition, in the runtime's own section order.
 *
 * Read from `sections` only. `AgentStatusView.rendered` is the exact text the
 * model saw and stays diagnostics: recovering structure from a model-facing
 * string would make this client a second interpreter of a composition it already
 * receives structurally. A section the runtime published with nothing to say
 * contributes no facet rather than an `0` nobody asked about. */
export function agentStatusFacets(status: AgentStatusView): AgentStatusFacet[] {
  return status.sections.flatMap(section => { const facet = facetOf(section); return facet ? [facet] : []; });
}

function facetOf(section: RuntimeClientStatusSection): AgentStatusFacet | undefined {
  if (section.type === 'temporal') {
    const time = formatStatusTime(section.current_time, section.timezone ?? undefined);
    return { kind: 'temporal', compact: time, label: 'Time', values: [time] };
  }
  if (section.type === 'background_executions') {
    const executions = section.executions ?? [];
    const total = executions.length + section.omitted_count;
    if (!total) return undefined;
    const values = executions.slice(0, DETAIL_ENTRY_LIMIT).map(execution => clip(`${execution.tool_name} · ${execution.state}`, DETAIL_LIMIT));
    const hidden = section.omitted_count + Math.max(0, executions.length - DETAIL_ENTRY_LIMIT);
    if (hidden > 0) values.push(`… and ${hidden} more`);
    return { kind: 'background_executions', compact: `background ${total}`, label: 'Background', values };
  }
  if (!section.active_count) return undefined;
  const values: string[] = [];
  if (section.current) values.push(clip(todoSubject(section.current), DETAIL_LIMIT));
  // Counts restate the runtime's committed totals; they are never recomputed from
  // the bounded task list, which may omit entries.
  const counts = [`${section.active_count} active`];
  if (section.blocked_count > 0) counts.push(`${section.blocked_count} blocked`);
  if (section.completed_count > 0) counts.push(`${section.completed_count} completed`);
  values.push(counts.join(' · '));
  return { kind: 'todo', compact: `todo ${section.active_count}`, label: 'Todo', values };
}

/** The compact one-line annotation body, without styling. */
export function agentStatusSummary(status: AgentStatusView): string {
  return agentStatusFacets(status).map(facet => clip(facet.compact, VALUE_LIMIT)).join(' · ');
}

const todoSubject = (task: RuntimeClientTodoStatusTask) => task.status === 'in_progress' && task.active_form ? task.active_form : task.subject;

/** The sampled runtime clock, in the timezone the runtime configured. Both the
 * instant and the zone are runtime-published, so one composition reads the same
 * on every machine; an unparseable instant is shown verbatim rather than guessed. */
export function formatStatusTime(currentTime: string, timezone?: string): string {
  const instant = new Date(currentTime);
  if (Number.isNaN(instant.getTime())) return clip(currentTime, VALUE_LIMIT);
  return zonedParts(instant, timezone ?? 'UTC') ?? zonedParts(instant, 'UTC') ?? instant.toISOString();
}

function zonedParts(instant: Date, zone: string): string | undefined {
  let parts;
  try { parts = new Intl.DateTimeFormat('en-US', { timeZone: zone, hour: '2-digit', minute: '2-digit', hour12: false, timeZoneName: 'short' }).formatToParts(instant); }
  catch { return undefined; } // An unknown IANA zone is a runtime fact this client cannot render, not a render failure.
  const value = (type: string) => parts.find(part => part.type === type)?.value;
  const hour = value('hour'), minute = value('minute'), name = value('timeZoneName');
  if (hour === undefined || minute === undefined) return undefined;
  return name === undefined ? `${hour}:${minute}` : `${hour}:${minute} ${name}`;
}

/** Bounds one externally derived value without rewriting it into a lie. */
function clip(text: string, limit: number): string {
  const flat = text.replace(/\s+/gu, ' ').trim();
  const characters = [...flat];
  return characters.length <= limit ? flat : `${characters.slice(0, Math.max(0, limit - 1)).join('')}…`;
}

/** The authoritative bounded window, oldest first. Never a browser-owned history. */
export const agentStatusHistory = (snapshot?: RuntimeClientSnapshot): readonly AgentStatusView[] => snapshot?.statuses ?? [];
