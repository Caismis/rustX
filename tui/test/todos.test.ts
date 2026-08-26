/**
 * The task list, derived and drawn.
 *
 * The panel is a pure function of the transcript, so every case here builds
 * canonical tool results and asserts what the reader sees. Nothing in the
 * client stores a task, and nothing here simulates one.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { COMMANDS } from "../src/commands/registry.ts";
import { emptyPresentationState } from "../src/presentation/projection.ts";
import type { PresentationState } from "../src/presentation/state.ts";
import {
  TODO_TOOL_ID,
  type TodoSnapshot,
  type TodoTask,
  progress,
  selectTodos,
} from "../src/presentation/todos.ts";
import type { MessageBlock, ToolExecutionResult } from "../src/protocol/types.ts";
import { plainText } from "../src/ui/theme.ts";
import { rendererFor } from "../src/ui/components/tool-renderers.ts";
import {
  renderTodoInspection,
  renderTodoPanel,
} from "../src/ui/components/todos.ts";
import {
  sessionModel,
  toolMessage,
  toolResult,
  transcriptCursor,
} from "./support/fixtures.ts";

function task(id: number, subject: string, overrides: Partial<TodoTask> = {}): TodoTask {
  return { id, subject, status: "pending", ...overrides };
}

function snapshotOf(tasks: TodoTask[]): TodoSnapshot {
  return { tasks, next_id: tasks.length + 1 };
}

function todoResult(snapshot: TodoSnapshot, summary = "Updated #1"): ToolExecutionResult {
  return toolResult({
    content: [
      { type: "text", text: summary },
      { type: "json", value: snapshot },
    ],
  });
}

function stateWith(...messages: MessageBlock[]): PresentationState {
  const state = emptyPresentationState(sessionModel("alpha/model-a"));
  state.transcript = messages.map((message, index) => ({
    kind: "committed" as const,
    key: `entry-${index}`,
    messageId: message.id,
    cursor: transcriptCursor(index),
    message,
  }));
  return state;
}

describe("the derived task list", () => {
  it("is the newest list any settled todo result published", () => {
    const first = snapshotOf([task(1, "Write the parser")]);
    const latest = snapshotOf([
      task(1, "Write the parser", { status: "completed" }),
      task(2, "Write the tests", { status: "in_progress" }),
    ]);
    const state = stateWith(
      toolMessage("m1", "c1", TODO_TOOL_ID, todoResult(first)),
      toolMessage("m2", "c2", TODO_TOOL_ID, todoResult(latest)),
    );
    assert.deepEqual(selectTodos(state), latest);
    assert.deepEqual(progress(latest), { done: 1, total: 2 });
  });

  it("ignores results of other tools and rejected todo calls", () => {
    const published = snapshotOf([task(1, "Write the parser")]);
    const rejected = toolMessage(
      "m3",
      "c3",
      TODO_TOOL_ID,
      toolResult({
        status: { type: "failed", error: "#9 not found" },
        content: [],
      }),
    );
    const otherTool = toolMessage(
      "m2",
      "c2",
      "tool-bash",
      todoResult(snapshotOf([task(7, "Not a task list")])),
    );
    const state = stateWith(
      toolMessage("m1", "c1", TODO_TOOL_ID, todoResult(published)),
      otherTool,
      rejected,
    );
    assert.deepEqual(
      selectTodos(state),
      published,
      "a rejected call changes nothing, and another tool's JSON is not this list",
    );
  });

  it("ignores a malformed payload instead of drawing it", () => {
    const state = stateWith(
      toolMessage(
        "m1",
        "c1",
        TODO_TOOL_ID,
        toolResult({
          content: [{ type: "json", value: { tasks: [{ id: "one" }], next_id: 2 } }],
        }),
      ),
    );
    assert.equal(selectTodos(state), undefined);
  });

  it("is absent, not empty, when the conversation never used the tool", () => {
    assert.equal(selectTodos(stateWith()), undefined);
    assert.equal(selectTodos(undefined), undefined);
  });
});

describe("the task panel", () => {
  it("draws nothing when there is nothing to show", () => {
    assert.equal(renderTodoPanel(undefined, { columns: 80 }), "");
    assert.equal(renderTodoPanel(snapshotOf([]), { columns: 80 }), "");
    assert.equal(
      renderTodoPanel(snapshotOf([task(1, "Gone", { status: "deleted" })]), {
        columns: 80,
      }),
      "",
      "a list of tombstones is an empty list",
    );
  });

  it("shows progress, the active task, and its dependencies", () => {
    const panel = plainText(
      renderTodoPanel(
        snapshotOf([
          task(1, "Write the parser", { status: "completed" }),
          task(2, "Write the tests", {
            status: "in_progress",
            active_form: "writing the tests",
          }),
          task(3, "Ship", { blocked_by: [1, 2] }),
        ]),
        { columns: 80 },
      ),
    );
    assert.equal(
      panel,
      [
        "● Todos (1/3)",
        "├─ ✓ #1 Write the parser",
        "├─ ◐ #2 Write the tests (writing the tests)",
        "└─ ○ #3 Ship ⛓ #1,#2",
      ].join("\n"),
    );
  });

  it("omits per-row ids when nothing points at one", () => {
    const panel = plainText(
      renderTodoPanel(snapshotOf([task(1, "Write the parser")]), { columns: 80 }),
    );
    assert.equal(panel, ["● Todos (0/1)", "└─ ○ Write the parser"].join("\n"));
  });

  it("dims the heading once every task is complete", () => {
    const panel = plainText(
      renderTodoPanel(
        snapshotOf([task(1, "Write the parser", { status: "completed" })]),
        { columns: 80 },
      ),
    );
    assert.match(panel, /^○ Todos \(1\/1\)/);
  });

  it("drops completed rows before unfinished ones and names what it hid", () => {
    const tasks = [
      task(1, "Completed first", { status: "completed" }),
      task(2, "Completed second", { status: "completed" }),
      task(3, "Completed third", { status: "completed" }),
      task(4, "Pending first"),
      task(5, "Pending second"),
    ];
    assert.equal(
      plainText(renderTodoPanel(snapshotOf(tasks), { columns: 80, rows: 5 })),
      [
        "● Todos (3/5)",
        "├─ ✓ Completed first",
        "├─ ○ Pending first",
        "├─ ○ Pending second",
        "└─ +2 more (2 completed)",
      ].join("\n"),
      "the oldest completed row is the last completed row to go",
    );
    assert.equal(
      plainText(renderTodoPanel(snapshotOf(tasks), { columns: 80, rows: 4 })),
      [
        "● Todos (3/5)",
        "├─ ○ Pending first",
        "├─ ○ Pending second",
        "└─ +3 more (3 completed)",
      ].join("\n"),
      "unfinished work is the last thing to disappear",
    );
    assert.equal(
      plainText(renderTodoPanel(snapshotOf(tasks), { columns: 80, rows: 3 })),
      [
        "● Todos (3/5)",
        "├─ ○ Pending first",
        "└─ +4 more (3 completed, 1 unfinished)",
      ].join("\n"),
      "past the floor the unfinished tail is trimmed, and named",
    );
  });

  it("clips a row rather than wrapping the panel", () => {
    const panel = plainText(
      renderTodoPanel(snapshotOf([task(1, "x".repeat(200))]), { columns: 40 }),
    );
    for (const line of panel.split("\n")) {
      assert.ok(line.length <= 40, `${line.length} > 40: ${line}`);
    }
    assert.match(panel, /…$/);
  });
});

describe("the todo tool card", () => {
  const renderer = rendererFor(TODO_TOOL_ID);

  it("names the action, the task, and the requested status", () => {
    const call = renderer.renderCall({
      action: "update",
      id: 2,
      status: "in_progress",
    });
    assert.equal(call?.title, "Todo");
    assert.equal(plainText(call?.subject ?? ""), "update #2 → in_progress");
  });

  it("shows the runtime's summary and never redraws the whole list", () => {
    const result = renderer.renderResult?.(
      todoResult(snapshotOf([task(1, "Write the parser")]), "Created #1: Write the parser (pending)"),
      { action: "create" },
    );
    assert.deepEqual(result?.summary, ["Created #1: Write the parser (pending)"]);
    assert.deepEqual(
      result?.detail,
      [],
      "the published list belongs to the panel, not to a second copy in the transcript",
    );
  });
});

describe("/todos", () => {
  it("is part of the bounded command surface", () => {
    assert.ok(COMMANDS.some((command) => command.name === "/todos"));
  });

  it("prints the complete list grouped by status", () => {
    const body = plainText(
      renderTodoInspection(
        snapshotOf([
          task(1, "Write the parser", { status: "completed" }),
          task(2, "Write the tests", {
            status: "in_progress",
            active_form: "writing the tests",
          }),
          task(3, "Ship", { blocked_by: [2] }),
          task(4, "Gone", { status: "deleted" }),
        ]),
      ),
    );
    assert.equal(
      body,
      [
        "1/3 completed · 1 in progress · 1 pending",
        "",
        "In progress",
        "  ◐ #2 Write the tests (writing the tests)",
        "",
        "Pending",
        "  ○ #3 Ship ⛓ #2",
        "",
        "Completed",
        "  ✓ #1 Write the parser",
      ].join("\n"),
      "tombstones are never listed",
    );
  });

  it("says so when there is no list yet", () => {
    assert.match(plainText(renderTodoInspection(undefined)), /^No tasks yet/);
  });
});
