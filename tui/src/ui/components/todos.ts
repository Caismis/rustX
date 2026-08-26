/**
 * The task panel: the model's plan, kept on screen.
 *
 * ```text
 * ● Todos (2/5)
 * ├─ ✓ Define the canonical snapshot
 * ├─ ◐ Register the native tool (registering the native tool)
 * ├─ ○ Derive the panel from the transcript   ⛓ #2
 * └─ ○ Document the capability
 * ```
 *
 * The panel sits above the editor because it answers a question the reader
 * has *while typing the next message* — what is this agent doing, and what is
 * still queued. It is drawn from {@link selectTodos}, so it holds no state of
 * its own and disappears when the list is empty.
 *
 * # The budget is a floor on what stays visible, not a cap on the list
 *
 * A long list must not push the conversation off the screen, so the panel is
 * bounded. What it drops is chosen so the *unfinished* work is the last thing
 * to go: completed rows go first, newest first, and only then is the tail of
 * the remaining list trimmed. Whatever is hidden is always named on a final
 * `+N more` row, so the reader is never shown a silently truncated plan —
 * and `/todos` prints the complete list, unbounded, at any time.
 */

import {
  type TodoSnapshot,
  type TodoStatus,
  type TodoTask,
  progress,
  visibleTasks,
} from "../../presentation/todos.ts";
import { role } from "../theme.ts";
import { clipText } from "./tool-renderers.ts";

/** The content rows the panel may draw, heading and `+N more` included. */
export const TODO_PANEL_ROWS = 12;

/** The minimum sensible budget: a heading, one task, and a summary row. */
const MINIMUM_ROWS = 3;

const GLYPH: Record<TodoStatus, string> = {
  pending: "○",
  in_progress: "◐",
  completed: "✓",
  deleted: "✗",
};

export interface TodoPanelLayout {
  /** Terminal width available to the panel. */
  columns: number;
  /** The content-row budget. Defaults to {@link TODO_PANEL_ROWS}. */
  rows?: number;
}

/**
 * The panel, or an empty string when there is nothing to show.
 *
 * An empty string means "draw nothing at all": a conversation that never
 * tracked tasks, and one whose list was cleared, both leave the area above
 * the editor exactly as it was before the tool existed.
 */
export function renderTodoPanel(
  snapshot: TodoSnapshot | undefined,
  layout: TodoPanelLayout,
): string {
  if (snapshot === undefined) {
    return "";
  }
  const tasks = visibleTasks(snapshot);
  if (tasks.length === 0) {
    return "";
  }
  const { done, total } = progress(snapshot);
  const complete = done === total;
  const heading = `${complete ? role.meta("○") : role.accent("●")} ${
    complete ? role.meta(`Todos (${done}/${total})`) : role.strong(`Todos (${done}/${total})`)
  }`;

  const budget = Math.max(layout.rows ?? TODO_PANEL_ROWS, MINIMUM_ROWS);
  const { shown, hiddenCompleted, hiddenUnfinished } = fit(tasks, budget);
  const hidden = hiddenCompleted + hiddenUnfinished;
  // Ids are drawn only when something points at one: without a `⛓ #N`
  // anywhere in view, a per-row id names nothing the reader can use.
  const withIds = shown.some((task) => (task.blocked_by ?? []).length > 0);
  const width = Math.max(layout.columns - 4, 20);

  const rows = shown.map((task, index) => {
    const last = index === shown.length - 1 && hidden === 0;
    const prefix = role.chrome(last ? "└─" : "├─");
    return `${prefix} ${clipText(renderRow(task, withIds), width)}`;
  });
  if (hidden > 0) {
    rows.push(`${role.chrome("└─")} ${role.meta(summarizeHidden(hiddenCompleted, hiddenUnfinished))}`);
  }
  return [heading, ...rows].join("\n");
}

/** The complete list, grouped by status, for `/todos`. */
export function renderTodoInspection(snapshot: TodoSnapshot | undefined): string {
  if (snapshot === undefined || visibleTasks(snapshot).length === 0) {
    return "No tasks yet. The agent creates them with the todo tool as it plans multi-step work.";
  }
  const tasks = visibleTasks(snapshot);
  const { done, total } = progress(snapshot);
  const inProgress = tasks.filter((task) => task.status === "in_progress");
  const pending = tasks.filter((task) => task.status === "pending");
  const completed = tasks.filter((task) => task.status === "completed");

  const counts = [
    `${done}/${total} completed`,
    inProgress.length > 0 ? `${inProgress.length} in progress` : undefined,
    pending.length > 0 ? `${pending.length} pending` : undefined,
  ].filter((count): count is string => count !== undefined);

  const sections: string[] = [counts.join(" · ")];
  for (const [title, group] of [
    ["In progress", inProgress],
    ["Pending", pending],
    ["Completed", completed],
  ] as const) {
    if (group.length === 0) {
      continue;
    }
    sections.push(
      [role.strong(title), ...group.map((task) => `  ${renderRow(task, true)}`)].join("\n"),
    );
  }
  return sections.join("\n\n");
}

/** One task row: glyph, optional id, subject, activity, dependencies. */
function renderRow(task: TodoTask, withIds: boolean): string {
  const glyph = statusStyle(task.status)(GLYPH[task.status]);
  const identity = withIds ? `${role.meta(`#${task.id}`)} ` : "";
  const subject =
    task.status === "completed" || task.status === "deleted"
      ? role.meta(task.subject)
      : task.subject;
  const activity =
    task.status === "in_progress" && task.active_form !== undefined
      ? ` ${role.meta(`(${task.active_form})`)}`
      : "";
  const blockedBy = task.blocked_by ?? [];
  const dependencies =
    blockedBy.length > 0
      ? ` ${role.meta(`⛓ ${blockedBy.map((id) => `#${id}`).join(",")}`)}`
      : "";
  return `${glyph} ${identity}${subject}${activity}${dependencies}`;
}

function statusStyle(status: TodoStatus): (value: string) => string {
  switch (status) {
    case "in_progress":
      return role.pending;
    case "completed":
      return role.success;
    case "deleted":
      return role.meta;
    default:
      return role.chrome;
  }
}

/**
 * Chooses the rows to draw within the budget.
 *
 * Completed rows are dropped newest-first, so the oldest finished work — the
 * part a reader scrolls back to for context — is the last completed row to
 * go. Only when the unfinished tasks alone overflow is their tail trimmed.
 */
function fit(
  tasks: TodoTask[],
  budget: number,
): { shown: TodoTask[]; hiddenCompleted: number; hiddenUnfinished: number } {
  const forRows = budget - 1; // the heading always costs one row
  if (tasks.length <= forRows) {
    return { shown: tasks, hiddenCompleted: 0, hiddenUnfinished: 0 };
  }
  const room = forRows - 1; // the `+N more` row costs one more
  const completedIndices = tasks
    .map((task, index) => ({ task, index }))
    .filter((entry) => entry.task.status === "completed")
    .map((entry) => entry.index);

  const dropped = new Set<number>();
  let hiddenCompleted = 0;
  for (
    let cursor = completedIndices.length - 1;
    cursor >= 0 && tasks.length - dropped.size > room;
    cursor -= 1
  ) {
    const index = completedIndices[cursor];
    if (index === undefined) continue;
    dropped.add(index);
    hiddenCompleted += 1;
  }

  const kept = tasks.filter((_, index) => !dropped.has(index));
  const shown = kept.slice(0, Math.max(room, 0));
  const hiddenUnfinished = kept.length - shown.length;
  return { shown, hiddenCompleted, hiddenUnfinished };
}

function summarizeHidden(completed: number, unfinished: number): string {
  const parts = [
    completed > 0 ? `${completed} completed` : undefined,
    unfinished > 0 ? `${unfinished} unfinished` : undefined,
  ].filter((part): part is string => part !== undefined);
  return `+${completed + unfinished} more (${parts.join(", ")})`;
}
