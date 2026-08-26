/**
 * The task list, derived from the transcript the client already holds.
 *
 * The runtime publishes the complete list as the structured content of every
 * settled `todo` result, so the panel is a *pure derivation* of canonical
 * conversation content — not a second copy of runtime state:
 *
 * ```text
 * canonical tool result (tool_id: tool-todo)  ->  TodoSnapshot  ->  panel
 * ```
 *
 * Two consequences follow, and both are the point:
 *
 * - the client stores nothing. A fresh authoritative snapshot reproduces the
 *   panel exactly, and no local edit, expiry, or replay of tool calls is
 *   involved;
 * - the list survives a reload, a resume, and a compaction for the same
 *   reason the transcript does, because it *is* the transcript.
 *
 * Selection is keyed by the runtime's own {@link ToolId}, never by a tool
 * name and never by the shape of the JSON. A differently identified tool that
 * happens to publish similar structure is not this list.
 */

import type { MessageBlock, ToolExecutionResult } from "../protocol/types.ts";
import type { PresentationState } from "./state.ts";

/** The runtime identity of the native `todo` tool. */
export const TODO_TOOL_ID = "tool-todo";

export type TodoStatus = "pending" | "in_progress" | "completed" | "deleted";

/** One task, exactly as the runtime published it. */
export interface TodoTask {
  id: number;
  subject: string;
  description?: string;
  active_form?: string;
  status: TodoStatus;
  blocked_by?: number[];
  owner?: string;
  metadata?: Record<string, unknown>;
}

/** The complete list of one conversation. */
export interface TodoSnapshot {
  tasks: TodoTask[];
  next_id: number;
}

const STATUSES: ReadonlySet<string> = new Set([
  "pending",
  "in_progress",
  "completed",
  "deleted",
]);

/**
 * The list as of the newest settled `todo` result in the loaded transcript.
 *
 * Returns `undefined` when the conversation has no such result — which is
 * also what an unloaded older transcript page looks like, so an absent list
 * is never rendered as an empty one.
 */
export function selectTodos(
  state: PresentationState | undefined,
): TodoSnapshot | undefined {
  if (state === undefined) {
    return undefined;
  }
  for (let index = state.transcript.length - 1; index >= 0; index -= 1) {
    const entry = state.transcript[index];
    if (entry?.kind !== "committed") {
      continue;
    }
    const snapshot = snapshotOf(entry.message);
    if (snapshot !== undefined) {
      return snapshot;
    }
  }
  return undefined;
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

function snapshotOf(message: MessageBlock): TodoSnapshot | undefined {
  if (message.role !== "tool" || message.tool_id !== TODO_TOOL_ID) {
    return undefined;
  }
  return publishedSnapshot(message.result);
}

/**
 * The snapshot a settled result published, or `undefined`.
 *
 * A rejected call publishes nothing, so a failed result never replaces the
 * list — which matches the runtime, where a rejected call leaves the list
 * untouched. The value is validated structurally rather than trusted: this
 * client renders it, so a malformed payload must be ignored rather than
 * drawn.
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

function parseSnapshot(value: unknown): TodoSnapshot | undefined {
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
    subject: candidate.subject,
    status: candidate.status as TodoStatus,
  };
  const description = optionalString(candidate.description);
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

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}
