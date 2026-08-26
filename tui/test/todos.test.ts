/**
 * The task list, derived and drawn.
 *
 * The panel is a pure function of the runtime's own derivation of canonical
 * `todo` results — the snapshot carries it, and a committed result moves it.
 * Every case here drives one of those two paths and asserts what the reader
 * sees. Nothing in the client stores a task, and nothing here simulates one.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { COMMANDS } from "../src/commands/registry.ts";
import {
  emptyPresentationState,
  reduce,
  replaceFromSnapshot,
} from "../src/presentation/projection.ts";
import type { PresentationState } from "../src/presentation/state.ts";
import {
  TODO_TOOL_ID,
  type TodoSnapshot,
  type TodoTask,
  progress,
  selectTodos,
} from "../src/presentation/todos.ts";
import { sanitizeData, sanitizeField, sanitizeLine } from "../src/sanitize.ts";
import type { MessageBlock, ToolExecutionResult } from "../src/protocol/types.ts";
import { plainText } from "../src/ui/theme.ts";
import { rendererFor } from "../src/ui/components/tool-renderers.ts";
import { renderToolCard } from "../src/ui/components/tool-card.ts";
import {
  DEFAULT_PREVIEW_CHARS,
  DEFAULT_PREVIEW_LINES,
} from "../src/ui/preferences.ts";
import {
  renderTodoInspection,
  renderTodoPanel,
} from "../src/ui/components/todos.ts";
import {
  assistantMessage,
  runtimeCursor,
  sessionModel,
  snapshot as clientSnapshot,
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

/** The state a client holds after attaching to `todos`, with `messages` loaded. */
function attached(
  todos: TodoSnapshot | undefined,
  messages: MessageBlock[] = [],
): PresentationState {
  return replaceFromSnapshot(
    clientSnapshot({ messages, todos }),
    runtimeCursor(1),
  );
}

/** One committed message, folded through the real reducer. */
function committed(
  state: PresentationState,
  message: MessageBlock,
  cursor: number,
): PresentationState {
  return reduce(state, {
    cursor: runtimeCursor(cursor),
    event: {
      type: "message_committed",
      message,
      transcript_cursor: transcriptCursor(cursor),
    },
  });
}

describe("the derived task list", () => {
  it("is the list the runtime derived, whatever the loaded transcript holds", () => {
    const latest = snapshotOf([
      task(1, "Write the parser", { status: "completed" }),
      task(2, "Write the tests", { status: "in_progress" }),
    ]);
    assert.deepEqual(selectTodos(attached(latest)), latest);
    assert.deepEqual(progress(latest), { done: 1, total: 2 });
  });

  /**
   * The regression the transcript scan could not survive.
   *
   * A client is seeded with a bounded newest page of transcript. Once enough
   * messages commit after the last `todo` result, that result is no longer on
   * the page — and a client that derived the list by scanning what it holds
   * showed no list at all, while the runtime still had one.
   */
  it("survives a fresh attach whose transcript page has scrolled past the result", () => {
    const list = snapshotOf([task(1, "Write the parser", { status: "in_progress" })]);
    const since = Array.from({ length: 65 }, (_, index) =>
      assistantMessage(`message-${index}`, `after ${index}`),
    );
    const state = attached(list, since);
    assert.ok(
      !state.transcript.some(
        (entry) => entry.kind === "committed" && entry.message.role === "tool",
      ),
      "the page really does not contain the todo result any more",
    );
    assert.deepEqual(selectTodos(state), list, "the runtime's list is still the list");
  });

  it("follows a committed todo result live", () => {
    const first = snapshotOf([task(1, "Write the parser")]);
    const latest = snapshotOf([
      task(1, "Write the parser", { status: "completed" }),
      task(2, "Write the tests"),
    ]);
    let state = attached(first);
    state = committed(
      state,
      toolMessage("m2", "c2", TODO_TOOL_ID, todoResult(latest)),
      2,
    );
    assert.deepEqual(selectTodos(state), latest);
  });

  it("ignores results of other tools and rejected todo calls", () => {
    const published = snapshotOf([task(1, "Write the parser")]);
    let state = attached(published);
    state = committed(
      state,
      toolMessage(
        "m2",
        "c2",
        "tool-bash",
        todoResult(snapshotOf([task(7, "Not a task list")])),
      ),
      2,
    );
    state = committed(
      state,
      toolMessage(
        "m3",
        "c3",
        TODO_TOOL_ID,
        toolResult({ status: { type: "failed", error: "#9 not found" }, content: [] }),
      ),
      3,
    );
    assert.deepEqual(
      selectTodos(state),
      published,
      "a rejected call changes nothing, and another tool's JSON is not this list",
    );
  });

  it("keeps the good list when a payload is malformed", () => {
    const published = snapshotOf([task(1, "Write the parser")]);
    let state = attached(published);
    state = committed(
      state,
      toolMessage(
        "m2",
        "c2",
        TODO_TOOL_ID,
        toolResult({
          content: [{ type: "json", value: { tasks: [{ id: "one" }], next_id: 2 } }],
        }),
      ),
      2,
    );
    assert.deepEqual(
      selectTodos(state),
      published,
      "an undrawable payload is ignored, never promoted over a good list",
    );
    assert.deepEqual(
      selectTodos(attached(undefined)),
      { tasks: [], next_id: 1 },
      "a snapshot that carries no list at all is the empty list, not a guess",
    );
  });

  it("is empty, not absent, when the conversation never used the tool", () => {
    assert.deepEqual(selectTodos(attached({ tasks: [], next_id: 1 })), {
      tasks: [],
      next_id: 1,
    });
    assert.equal(
      selectTodos(emptyPresentationState(sessionModel("alpha/model-a"))),
      undefined,
      "only a client that has not attached yet has no list",
    );
    assert.equal(selectTodos(undefined), undefined);
  });
});

describe("task text a terminal must not be handed", () => {
  const ESC = String.fromCharCode(27);

  it("keeps every field to one line, whatever the payload says", () => {
    const state = attached({
      tasks: [
        {
          id: 1,
          subject: `safe${String.fromCharCode(10)}spoofed`,
          status: "in_progress",
          active_form: `writing${String.fromCharCode(9)}fast`,
          owner: `me${String.fromCharCode(13)}`,
        },
      ],
      next_id: 2,
    });
    const list = selectTodos(state);
    const subject = list?.tasks[0]?.subject ?? "";
    assert.ok(!/[\n\r\t]/.test(subject), subject);
    assert.ok(!/[\n\r\t]/.test(list?.tasks[0]?.active_form ?? ""));
    assert.ok(!/[\n\r\t]/.test(list?.tasks[0]?.owner ?? ""));
  });

  it("bounds the panel in physical rows, not in tasks", () => {
    const tasks = Array.from({ length: 6 }, (_, index) =>
      task(index + 1, `line${String.fromCharCode(10)}break ${index}`),
    );
    const state = attached({ tasks, next_id: 7 });
    const panel = renderTodoPanel(selectTodos(state), { columns: 80, rows: 3 });
    assert.equal(
      plainText(panel).split(String.fromCharCode(10)).length,
      3,
      "a newline in a subject cannot buy a task a second row",
    );
  });

  it("never lets an escape sequence through to the terminal", () => {
    const state = attached({
      tasks: [
        {
          id: 1,
          subject: `${ESC}[31mred${ESC}[0m${ESC}]0;retitled${String.fromCharCode(7)}`,
          status: "pending",
        },
      ],
      next_id: 2,
    });
    const panel = renderTodoPanel(selectTodos(state), { columns: 80 });
    assert.ok(!plainText(panel).includes(ESC), "no ESC survives sanitization");
    assert.equal(
      sanitizeField(`a${ESC}b`),
      `a${String.fromCharCode(0xfffd)}b`,
      "a removed character leaves a visible mark rather than vanishing",
    );
  });

  /**
   * Unicode's `Bidi_Control` property is twelve code points, and the twelfth
   * is the one that gets left out: `U+061C` ARABIC LETTER MARK sits in the
   * Arabic block rather than beside the other eleven, and it is `Cf` rather
   * than a control character, so it survives every rule written from the
   * LRM/RLM, embedding, and isolate families — while reversing reading order
   * exactly as they do. The list is asserted whole so the next one added is
   * added here too.
   */
  it("strips every bidi control that would reverse what the reader sees", () => {
    const MARK = String.fromCharCode(0xfffd);
    for (const code of [
      0x061c, 0x200e, 0x200f, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2066, 0x2067, 0x2068,
      0x2069,
    ]) {
      const control = String.fromCharCode(code);
      assert.equal(
        sanitizeField(`ship${control}dangerous`),
        `ship${MARK}dangerous`,
        `U+${code.toString(16).toUpperCase().padStart(4, "0")} still reaches the row`,
      );
      assert.equal(
        sanitizeField(`ship${control}dangerous`, true),
        `ship${MARK}dangerous`,
        "the long-form field keeps line breaks, not bidi controls",
      );
      assert.deepEqual(
        sanitizeData({ [`own${control}er`]: [`me${control}`, 3] }),
        { owner: ["me", 3] },
        "nor does a key or a nested value keep one",
      );
    }
  });

  it("keeps line breaks in the one long-form field", () => {
    const paragraph = `first${String.fromCharCode(10)}second`;
    assert.equal(sanitizeField(paragraph, true), paragraph);
  });
});

describe("what the tool card may hand a terminal", () => {
  const ESC = String.fromCharCode(27);
  const BEL = String.fromCharCode(7);
  const LF = String.fromCharCode(10);
  const RLO = String.fromCharCode(0x202e);
  const cardContext = {
    expanded: false,
    budget: { maxLines: DEFAULT_PREVIEW_LINES, maxChars: DEFAULT_PREVIEW_CHARS },
  };

  function todoCard(args: unknown, result?: ToolExecutionResult): string {
    return renderToolCard(
      {
        callId: "call-todo",
        toolId: TODO_TOOL_ID,
        name: "todo",
        argumentsText: JSON.stringify(args),
        lifecycle:
          result === undefined ? { type: "assembled" } : { type: "settled", result },
        committed: result !== undefined,
      },
      cardContext,
    );
  }

  /**
   * The call band is drawn from the model's own arguments while the
   * assistant message is still streaming — before any executor, and
   * therefore before any rejection, has seen them. Input validation cannot
   * unprint what was already printed, so the card is the boundary.
   *
   * Asserted on the card itself, never on `plainText(card)`: `plainText`
   * strips every SGR sequence, including the ones an argument smuggled in,
   * so an assertion made through it passes on a card that is handing the
   * terminal exactly what this test exists to catch.
   */
  it("draws a rejected call's arguments without handing them to the terminal", () => {
    const card = todoCard({
      action: `create${LF}spoofed`,
      subject: `${ESC}[2J${ESC}]0;owned${BEL}safe${LF}second line`,
      status: `pending${RLO}gnidnep`,
    });
    assert.ok(!card.includes(`${ESC}[2J`), "no screen clear reaches the terminal");
    assert.ok(!card.includes(`${ESC}]0;`), "no window retitle reaches the terminal");
    assert.ok(!card.includes(BEL), "no OSC terminator reaches the terminal");
    assert.ok(!card.includes(RLO), "no bidi override reaches the terminal");
    assert.equal(
      card.split(LF).length,
      1,
      "a newline in an argument cannot buy the call band a second row",
    );
  });

  /**
   * `ESC[8m` is *conceal*, and it is an SGR sequence — the same shape the
   * theme itself emits. That is the whole difficulty: a card is assembled
   * out of styled fragments, so a filter applied to the finished line cannot
   * tell the model's `ESC[8m` from the client's `ESC[2m`, and one that
   * spares "our own colours" spares this too. The reduction therefore
   * happens before styling, where provenance is still known.
   *
   * The assertion is an equality rather than a search: the dangerous card
   * must render identically to the card of the same arguments with the
   * escapes already removed — no hidden text, no extra styling, nothing.
   */
  it("keeps model-written SGR out of a card it cannot distinguish it in", () => {
    const dangerous = todoCard({
      action: "create",
      subject: `${ESC}[8mhidden${ESC}[0m visible`,
    });
    assert.ok(
      !dangerous.includes(`${ESC}[8m`),
      "a concealed run would hide text the reader is being shown",
    );
    assert.equal(
      dangerous,
      todoCard({ action: "create", subject: "[8mhidden[0m visible" }),
      "the payload draws as the inert text it should have been",
    );
    assert.ok(
      plainText(dangerous).includes("[8mhidden[0m visible"),
      "and the reader can see what the model actually wrote",
    );
  });

  /**
   * The card draws the same characters the runtime refuses, and it draws
   * them before the runtime has seen them. `U+061C` is the one that would
   * have survived both halves of that boundary at once: the runtime's rule
   * was `is_control` plus a hand-listed bidi set, and the client's was that
   * same set written as a character class.
   */
  it("draws no bidi control at all, including the one that is not a control character", () => {
    const ALM = String.fromCharCode(0x061c);
    const settled = (subject: string): ToolExecutionResult => ({
      status: { type: "success" },
      content: [
        { type: "text", text: `[pending] #1 ${subject}` },
        { type: "json", value: { tasks: [{ subject }] } },
      ],
      duration_ms: 0,
      artifacts: [],
    });
    const card = todoCard(
      { action: "create", subject: `ship${ALM}dangerous`, owner: `me${ALM}` },
      settled(`ship${ALM}dangerous`),
    );
    assert.ok(!card.includes(ALM), "no reading-order reversal reaches the terminal");
    // The two bands neutralize it differently, and deliberately: an argument
    // is short model-written text where a mark tells the reader something
    // was removed, and a result body is arbitrary tool output where one mark
    // per stray byte would be noise. So the dangerous card is exactly the
    // card of the marked arguments and the plain body.
    assert.equal(
      card,
      todoCard(
        {
          action: "create",
          subject: `ship${String.fromCharCode(0xfffd)}dangerous`,
          owner: `me${String.fromCharCode(0xfffd)}`,
        },
        settled("shipdangerous"),
      ),
      "the card is exactly the card of the same text with it already gone",
    );
  });

  /**
   * A published argument is JSON *text*: `"\u001b"` in it is six harmless
   * characters until `JSON.parse` turns them into one `ESC`. Reducing the
   * text alone would leave every renderer that reads a parsed field exposed.
   */
  it("reduces the arguments a renderer reads, not just the text they arrived in", () => {
    const card = renderToolCard(
      {
        callId: "call-todo",
        toolId: TODO_TOOL_ID,
        name: "todo",
        argumentsText: '{"action":"create","subject":"\\u001b[8mhidden"}',
        lifecycle: { type: "assembled" },
        committed: false,
      },
      cardContext,
    );
    assert.ok(!card.includes(`${ESC}[8m`), "the decoded escape never reaches the card");
    assert.ok(plainText(card).includes("[8mhidden"));
  });

  /**
   * Metadata is nested, and `get` renders `metadata.<key>: <value>`, so a
   * key is drawn as literally as a subject is. The runtime rejects a key
   * that carries a control character; the card is the second half of that
   * boundary, on the side that holds the terminal.
   */
  it("draws a result summary without handing it to the terminal", () => {
    const card = todoCard(
      { action: "get", id: 1 },
      {
        status: { type: "success" },
        content: [
          {
            type: "text",
            text: `[pending] #1 Ship${LF}metadata.${ESC}]0;owned${BEL}key: ${RLO}value`,
          },
          { type: "json", value: { subject: `${ESC}[8mhidden`, owner: `me${RLO}` } },
        ],
        duration_ms: 0,
        exit_code: undefined,
        artifacts: [],
      },
    );
    assert.ok(!card.includes(`${ESC}]0;`), "no window retitle reaches the terminal");
    assert.ok(!card.includes(`${ESC}[8m`), "nor a concealed run out of the result JSON");
    assert.ok(!card.includes(BEL));
    assert.ok(!card.includes(RLO));
  });

  /**
   * A failure reason is runtime-published prose drawn into an always-visible
   * band, and it is the one string on a card that a *tool* — not the model —
   * chooses. It goes through the same reduction.
   */
  it("draws a rejection reason without handing it to the terminal", () => {
    const card = todoCard(
      { action: "create", subject: "Ship" },
      {
        status: { type: "failed", error: `${ESC}[8mrejected${ESC}]0;owned${BEL}` },
        content: [],
        duration_ms: 0,
        artifacts: [],
      },
    );
    assert.ok(!card.includes(`${ESC}[8m`));
    assert.ok(!card.includes(`${ESC}]0;`));
    assert.ok(!card.includes(BEL));
  });

  it("keeps the styling this client emitted", () => {
    const card = todoCard({ action: "create", subject: "Write the parser" });
    assert.notEqual(card, plainText(card), "the card is still styled");
    assert.match(plainText(card), /Todo\s+create Write the parser/);
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
