/**
 * The task list, derived from canonical `todo` results.
 *
 * The runtime publishes the complete list as the structured content of every
 * settled `todo` result, so the panel is a *pure derivation* of canonical
 * conversation content — not a second copy of runtime state:
 *
 * ```text
 * canonical tool result (tool_id: tool-todo)  ->  TodoSnapshot  ->  panel
 * ```
 *
 * The derivation runs in two places over the same fact, and that is
 * deliberate:
 *
 * - the **runtime** derives it over the whole Ledger and carries the result
 *   in every snapshot. That is the list a fresh attach, a resume, or a
 *   post-compaction reload opens on;
 * - this client folds each newly committed `todo` result into the same
 *   value, so a live change lands without a snapshot round trip.
 *
 * Scanning the loaded transcript instead would be wrong, and was: a client
 * holds only a bounded newest page of it, so a conversation that committed a
 * page or more of messages after its last `todo` result would show no list
 * at all until the reader happened to page far enough back — while the
 * runtime, reading the whole Ledger, still had one.
 *
 * The client stores nothing of its own either way. A fresh authoritative
 * snapshot reproduces the panel exactly, and no local edit, expiry, or
 * replay of tool calls is involved.
 *
 * Selection is keyed by the runtime's own {@link ToolId}, never by a tool
 * name and never by the shape of the JSON. A differently identified tool that
 * happens to publish similar structure is not this list.
 */

import type {
  MessageBlock,
  TodoSnapshot,
  TodoStatus,
  TodoTask,
  ToolExecutionResult,
} from "../protocol/types.ts";
import type { PresentationState } from "./state.ts";

export type { TodoSnapshot, TodoStatus, TodoTask };

/** The runtime identity of the native `todo` tool. */
export const TODO_TOOL_ID = "tool-todo";

const STATUSES: ReadonlySet<string> = new Set([
  "pending",
  "in_progress",
  "completed",
  "deleted",
]);

/**
 * The conversation's list, or `undefined` when the projection has none.
 *
 * `undefined` means "this client has not been told yet", a state that only
 * exists before the first snapshot arrives. A conversation that never called
 * `todo`, and one whose list was cleared, both carry an *empty* list, which
 * is a fact rather than an absence.
 */
export function selectTodos(
  state: PresentationState | undefined,
): TodoSnapshot | undefined {
  return state?.todos;
}

/** Tasks that are neither completed nor tombstoned. */
export function openTasks(snapshot: TodoSnapshot): TodoTask[] {
  return snapshot.tasks.filter(
    (task) => task.status !== "completed" && task.status !== "deleted",
  );
}

/** Tasks the panel shows: everything the runtime still considers live. */
export function visibleTasks(snapshot: TodoSnapshot): TodoTask[] {
  return snapshot.tasks.filter((task) => task.status !== "deleted");
}

/** The `done/total` counters, which never count tombstones. */
export function progress(snapshot: TodoSnapshot): {
  done: number;
  total: number;
} {
  const visible = visibleTasks(snapshot);
  return {
    done: visible.filter((task) => task.status === "completed").length,
    total: visible.length,
  };
}

/**
 * The list one canonical message published, or `undefined`.
 *
 * A message from another tool, and a rejected `todo` call — which publishes
 * nothing — both leave the list alone, which matches the runtime, where a
 * rejected call leaves the list untouched.
 */
export function publishedTodos(
  message: MessageBlock,
): TodoSnapshot | undefined {
  if (message.role !== "tool" || message.tool_id !== TODO_TOOL_ID) {
    return undefined;
  }
  return publishedSnapshot(message.result);
}

/**
 * The snapshot a settled result published, or `undefined`.
 *
 * The value is validated structurally rather than trusted: this client
 * renders it, so a malformed payload must be ignored rather than drawn.
 */
function publishedSnapshot(
  result: ToolExecutionResult,
): TodoSnapshot | undefined {
  if (result.status.type !== "success") {
    return undefined;
  }
  for (const content of result.content ?? []) {
    if (content.type !== "json") {
      continue;
    }
    const snapshot = parseSnapshot(content.value);
    if (snapshot !== undefined) {
      return snapshot;
    }
  }
  return undefined;
}

/**
 * One list value, validated and made safe to draw.
 *
 * Every string a task carries was written by a model, and this client writes
 * them into a terminal. The runtime rejects control characters where the task
 * is created, so a well-behaved runtime never sends one; this is the second
 * half of that boundary, on the side that actually holds the terminal.
 * Whatever arrives, {@link sanitize} guarantees the two properties the
 * panel's layout depends on:
 *
 * - one task is one physical row, because no single-line field can contain a
 *   line break or a tab;
 * - the only escape sequences on screen are the ones this client emitted,
 *   because `ESC`, the C1 range, and the bidi controls never survive here.
 */
export function parseSnapshot(value: unknown): TodoSnapshot | undefined {
  if (typeof value !== "object" || value === null) {
    return undefined;
  }
  const candidate = value as { tasks?: unknown; next_id?: unknown };
  if (typeof candidate.next_id !== "number") {
    return undefined;
  }
  const rawTasks = candidate.tasks ?? [];
  if (!Array.isArray(rawTasks)) {
    return undefined;
  }
  const tasks: TodoTask[] = [];
  for (const raw of rawTasks) {
    const task = parseTask(raw);
    if (task === undefined) {
      return undefined;
    }
    tasks.push(task);
  }
  return { tasks, next_id: candidate.next_id };
}

function parseTask(value: unknown): TodoTask | undefined {
  if (typeof value !== "object" || value === null) {
    return undefined;
  }
  const candidate = value as Record<string, unknown>;
  if (
    typeof candidate.id !== "number" ||
    typeof candidate.subject !== "string" ||
    typeof candidate.status !== "string" ||
    !STATUSES.has(candidate.status)
  ) {
    return undefined;
  }
  const blockedBy = candidate.blocked_by;
  if (
    blockedBy !== undefined &&
    (!Array.isArray(blockedBy) ||
      blockedBy.some((entry) => typeof entry !== "number"))
  ) {
    return undefined;
  }
  // Absent stays absent: an optional field the runtime omitted must not
  // reappear here as an explicit `undefined`, or the derived list would stop
  // comparing equal to the one the runtime published.
  const task: TodoTask = {
    id: candidate.id,
    subject: sanitize(candidate.subject),
    status: candidate.status as TodoStatus,
  };
  // `description` is long-form prose that no bounded row ever draws, so it
  // keeps its line breaks — exactly the field rule the runtime enforces.
  const description = optionalString(candidate.description, true);
  if (description !== undefined) task.description = description;
  const activeForm = optionalString(candidate.active_form);
  if (activeForm !== undefined) task.active_form = activeForm;
  const owner = optionalString(candidate.owner);
  if (owner !== undefined) task.owner = owner;
  if (blockedBy !== undefined) task.blocked_by = blockedBy as number[];
  if (typeof candidate.metadata === "object" && candidate.metadata !== null) {
    task.metadata = candidate.metadata as Record<string, unknown>;
  }
  return task;
}

/**
 * The characters no rendered task field may carry: the C0 range (line
 * breaks, tabs, and `ESC`, which introduces every CSI and OSC sequence),
 * `DEL`, the C1 range, and the Unicode bidi controls.
 */
const FORBIDDEN = /[\u0000-\u001f\u007f-\u009f\u200e\u200f\u202a-\u202e\u2066-\u2069]/g;

/** The same set minus the line break, for the one long-form field. */
const FORBIDDEN_MULTILINE = /[\u0000-\u0009\u000b-\u001f\u007f-\u009f\u200e\u200f\u202a-\u202e\u2066-\u2069]/g;

/**
 * One field, reduced to text a terminal row can hold.
 *
 * Each offending character becomes U+FFFD rather than disappearing, so a
 * reader sees that something was removed instead of silently reading
 * doctored text.
 */
export function sanitize(value: string, multiline = false): string {
  return value.replace(multiline ? FORBIDDEN_MULTILINE : FORBIDDEN, "\ufffd");
}

function optionalString(value: unknown, multiline = false): string | undefined {
  return typeof value === "string" ? sanitize(value, multiline) : undefined;
}
