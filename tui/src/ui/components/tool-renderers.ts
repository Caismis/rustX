/**
 * Tool *presentation* renderers.
 *
 * The rule this module exists to enforce, stated once:
 *
 * > Tool identity may select a presentation renderer.
 * > Tool identity may never select or infer execution semantics.
 *
 * A renderer receives facts that are already authoritative — the runtime's
 * published arguments and its normalized result — and decides how to *show*
 * them. Whether a call is running, succeeded, was denied, cancelled, timed
 * out, or settled with an unknown outcome is decided before a renderer is
 * ever consulted, in Rust, and is rendered by the card shell in
 * {@link ./tool-card.ts}. No
 * function here receives the lifecycle, so none of them can express an
 * opinion about it.
 *
 * A renderer may:
 *   format a title, the published arguments, the normalized result content,
 *   and a deterministic diff derivable from those arguments.
 *
 * A renderer does **not** decide how much of that is shown. Every renderer
 * here is context-free: it separates what must always be visible (a title, a
 * one-line subject, a runtime-published summary) from verbose *detail*, and
 * the card shell in {@link ./tool-card.ts} applies the collapse budget to the
 * detail. That keeps progressive disclosure in one place instead of asking
 * every present and future renderer to remember to truncate itself.
 *
 * A renderer may not:
 *   execute anything, touch the filesystem, shell out, make a network
 *   request, decide a status from output text, mutate arguments or results,
 *   create messages, or change runtime behaviour in any way. There is no I/O
 *   surface in this module and no argument through which one could be
 *   supplied.
 *
 * Arguments arrive as opaque JSON text. Parsing it here is a *formatting*
 * step: a renderer that cannot make sense of a shape returns `undefined` and
 * the generic renderer takes over, so a malformed or unexpected argument
 * object degrades instead of crashing.
 */

import type { ToolExecutionResult, ToolId } from "../../protocol/types.ts";
import type { PreviewBudget } from "../preferences.ts";
import { role, style, plainText, plainWidth } from "../theme.ts";

/**
 * What a renderer says about the call itself.
 *
 * `subject` is the one-line identity of the call and is always drawn.
 * `detail` is everything verbose — argument JSON, a diff, the continuation
 * lines of a multiline command — and is what the card shell bounds.
 */
export interface ToolCallPresentation {
  /** The display title, e.g. `Bash`. */
  title: string;
  /** The one-line subject, e.g. `$ cargo test --all`. Always visible. */
  subject?: string;
  /** Verbose call detail, e.g. a deterministic diff. Bounded when collapsed. */
  detail?: string[];
}

/**
 * What a renderer says about the settled result.
 *
 * `summary` is runtime-published shape a reader needs even when collapsed —
 * `2 matches`, `applied 1 replacement`, `(no output)`. `detail` is the body.
 */
export interface ToolResultPresentation {
  /** Short runtime-published summary lines. Always visible. */
  summary?: string[];
  /** The verbose result body. Bounded when collapsed. */
  detail?: string[];
}

/**
 * How much of a band a collapsed card shows.
 *
 * `budget` is the *detail* budget — the always-visible bands carry their own
 * fixed budgets from {@link ../preferences.ts}, because a band a reader
 * cannot expand away is not the reader's line-count preference to spend.
 */
export interface ToolRenderContext {
  expanded: boolean;
  budget: PreviewBudget;
}

/**
 * One tool's presentation adapter.
 *
 * Both methods are total over "shapes I understand" and return `undefined`
 * for everything else, which is what makes the generic fallback mandatory
 * rather than aspirational. Neither receives a {@link ToolRenderContext}: a
 * renderer cannot see whether its card is expanded, so it cannot participate
 * in — or forget to participate in — progressive disclosure.
 */
export interface ToolPresentationRenderer {
  renderCall(args: unknown): ToolCallPresentation | undefined;
  renderResult?(
    result: ToolExecutionResult,
    args: unknown,
  ): ToolResultPresentation | undefined;
}

// ---------------------------------------------------------------------------
// Shared formatting helpers
// ---------------------------------------------------------------------------

/** Parses published argument text for display. Never for meaning. */
export function parseArguments(argumentsText: string): unknown {
  if (argumentsText.trim().length === 0) {
    return undefined;
  }
  try {
    return JSON.parse(argumentsText);
  } catch {
    // A partially streamed argument fragment is not valid JSON yet. That is
    // an ordinary intermediate state, not an error.
    return undefined;
  }
}

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function text(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function count(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

/** Splits a body into lines, dropping a single trailing newline. */
export function toLines(body: string): string[] {
  const normalized = body.endsWith("\n") ? body.slice(0, -1) : body;
  return normalized.length === 0 ? [] : normalized.split("\n");
}

/**
 * Bounds one band of a collapsed card, in height *and* in content length.
 *
 * This is the one disclosure policy of the whole card, and the reason it
 * takes both dimensions is that either one alone is not a bound. Eight lines
 * of pretty-printed JSON can carry a 100 kB string; a thousand characters can
 * arrive as a thousand one-character lines. A band is finite only when both
 * are capped, so both are, here, once, for every band.
 *
 * This is a *client visual collapse* and says nothing about the runtime's own
 * `TruncationState`, which the card reports separately and unconditionally.
 * `noun` names what was hidden so a card carrying both a long call detail and
 * a long result says which is which.
 *
 * Expanding returns the lines untouched. Those lines are already in hand —
 * they were built from the published call and the committed result — so
 * expansion re-renders, and never re-requests, re-executes, or reads.
 */
export function bounded(
  lines: string[],
  budget: PreviewBudget,
  context: ToolRenderContext,
  noun = "line",
): string[] {
  if (context.expanded) {
    return lines;
  }
  const kept: string[] = [];
  let used = 0;
  let consumed = 0;
  let clipped = 0;
  for (const line of lines) {
    if (kept.length >= budget.maxLines || used >= budget.maxChars) {
      break;
    }
    const width = plainWidth(line);
    const room = budget.maxChars - used;
    if (width <= room) {
      kept.push(line);
      used += width;
      consumed += 1;
      continue;
    }
    // The line itself is over budget, so this band's bulk is width rather
    // than height. Keep the part that fits and stop.
    if (room > 0) {
      kept.push(clip(line, room));
      clipped = width - room;
      used = budget.maxChars;
      consumed += 1;
    }
    break;
  }

  const hiddenLines = lines.length - consumed;
  if (hiddenLines === 0 && clipped === 0) {
    return kept;
  }
  // Both dimensions are reported, because hiding either silently would let a
  // collapsed card look complete when it is not.
  const parts: string[] = [];
  if (hiddenLines > 0) {
    parts.push(`${hiddenLines} more ${hiddenLines === 1 ? noun : `${noun}s`}`);
  }
  if (clipped > 0) {
    parts.push(`${clipped} more character${clipped === 1 ? "" : "s"}`);
  }
  return [...kept, role.meta(`… ${parts.join(" · ")} · ctrl+o to expand`)];
}

/**
 * Bounds one always-visible line, keeping the band exactly one line.
 *
 * The subject and the header are one line by contract, so their elision
 * marker is inline rather than a second line. Used for bands a reader cannot
 * scroll past: they must stay finite whatever the runtime published.
 */
export function boundedLine(
  value: string,
  budget: PreviewBudget,
  context: ToolRenderContext,
): string {
  if (context.expanded || plainWidth(value) <= budget.maxChars) {
    return value;
  }
  const hidden = plainWidth(value) - budget.maxChars;
  return `${clip(value, budget.maxChars)}${role.meta(`… ${hidden} more characters`)}`;
}

/**
 * Bounds one always-visible fragment that has no expanded form at all.
 *
 * Header chrome — a tool title, a runtime progress message — is drawn the
 * same whether the card is open or shut, so it is clipped unconditionally.
 */
export function clipText(value: string, maxChars: number): string {
  return plainWidth(value) <= maxChars ? value : `${clip(value, maxChars)}…`;
}

/**
 * Cuts one styled line to a visible-character budget.
 *
 * Styling is dropped rather than sliced: slicing through an SGR sequence
 * would leak escape bytes into the terminal. The band's meaning is in its
 * text, so losing the colour of a clipped line costs nothing.
 */
function clip(line: string, room: number): string {
  return [...plainText(line)].slice(0, Math.max(0, room)).join("");
}

/**
 * Bounds one verbose detail section with the reader's own detail budget.
 *
 * The convenience form of {@link bounded} for the two expandable bands.
 */
export function preview(
  lines: string[],
  context: ToolRenderContext,
  noun = "line",
): string[] {
  return bounded(lines, context.budget, context, noun);
}

/** The text of every textual result block, in publication order. */
export function resultText(result: ToolExecutionResult): string[] {
  const lines: string[] = [];
  for (const content of result.content ?? []) {
    if (content.type === "text") {
      lines.push(...toLines(content.text));
    }
  }
  return lines;
}

/** The first JSON result block, when the runtime published one. */
export function resultJson(result: ToolExecutionResult): unknown {
  for (const content of result.content ?? []) {
    if (content.type === "json") {
      return content.value;
    }
  }
  return undefined;
}

/** A stable, readable rendering of any published JSON value. */
export function formatJson(value: unknown): string[] {
  try {
    return toLines(JSON.stringify(value, null, 2) ?? String(value));
  } catch {
    return [String(value)];
  }
}

// ---------------------------------------------------------------------------
// The generic renderer
// ---------------------------------------------------------------------------

/**
 * The renderer every unknown tool gets, and the fallback for every known one.
 *
 * rustX already has heterogeneous tool sources — native and MCP (managed
 * Python tool packages surface as MCP-origin tools), plus whatever a future
 * extension registers — so this is not a courtesy path. It
 * is the normal path, and it must stay good: a readable title, deterministic
 * argument formatting, and a bounded result preview.
 */
export const genericRenderer: ToolPresentationRenderer = {
  renderCall(args: unknown): ToolCallPresentation | undefined {
    if (args === undefined) {
      // Not JSON (yet). The card shows the raw fragment instead, so a call
      // whose arguments are still streaming is never blank.
      return undefined;
    }
    const fields = record(args);
    if (fields === undefined) {
      return { title: "", detail: formatJson(args) };
    }
    const keys = Object.keys(fields);
    if (keys.length === 0) {
      return { title: "" };
    }
    // Arbitrarily large argument objects are normal for MCP tools (including
    // managed Python tool packages, which surface as MCP-origin).
    // The whole object is formatted here and bounded by the card shell, so a
    // few hundred lines of JSON never dominate a collapsed card.
    return { title: "", detail: formatJson(fields) };
  },
};

/**
 * The generic result body: published text first, then published JSON.
 *
 * Used whenever a specialized renderer declines a result shape, so a
 * specialized renderer can never make a result *less* visible than it would
 * have been without one.
 */
export function genericResultLines(
  result: ToolExecutionResult,
): ToolResultPresentation {
  const detail = resultText(result);
  const json = resultJson(result);
  if (json !== undefined) {
    detail.push(...formatJson(json));
  }
  // Non-textual content markers are body, not summary: a result carrying a
  // hundred file blocks is a hundred lines of detail and is bounded as such.
  for (const content of result.content ?? []) {
    if (content.type === "file") {
      detail.push(role.meta("(file)"));
    }
    if (content.type === "image") {
      detail.push(role.meta("(image)"));
    }
  }
  return { detail };
}

// ---------------------------------------------------------------------------
// Native tool renderers
//
// Keyed by the canonical `ToolId` the runtime publishes, never by the
// model-facing name: the name is what a model was told to type, the id is
// rustX's own identity for the capability.
// ---------------------------------------------------------------------------

const bashRenderer: ToolPresentationRenderer = {
  renderCall(args) {
    const fields = record(args);
    const command = text(fields?.["command"]);
    if (command === undefined) {
      return undefined;
    }
    const timeout = count(fields?.["timeout"]);
    const [first, ...rest] = toLines(command);
    return {
      title: "Bash",
      subject: `${role.chrome("$")} ${first ?? ""}${
        timeout === undefined ? "" : ` ${role.meta(`(timeout ${timeout}s)`)}`
      }`,
      // The remaining lines of a multiline command are call *detail*: the
      // first line identifies the call, the rest is bounded by the shell.
      detail: rest.map((line) => `  ${line}`),
    };
  },
  renderResult(result, _args) {
    // Bash publishes `{exit_code, stdout, stderr, combined}`. The combined
    // stream is what a person reads; the exit code is shown by the card
    // shell, from the runtime's own `exit_code` field.
    const payload = record(resultJson(result));
    if (payload === undefined) {
      return undefined;
    }
    const combined = text(payload["combined"]);
    const stdout = text(payload["stdout"]);
    const stderr = text(payload["stderr"]);
    const body =
      combined !== undefined && combined.length > 0
        ? toLines(combined)
        : [...toLines(stdout ?? ""), ...toLines(stderr ?? "")];
    if (body.length === 0) {
      return { summary: [role.meta("(no output)")] };
    }
    return { detail: body };
  },
};

const readRenderer: ToolPresentationRenderer = {
  renderCall(args) {
    const fields = record(args);
    const path = text(fields?.["path"]);
    if (path === undefined) {
      return undefined;
    }
    const offset = count(fields?.["offset"]);
    const limit = count(fields?.["limit"]);
    return {
      title: "Read",
      subject: path,
      // Only the window the model actually asked for is stated. The runtime's
      // own defaults are not restated here, because a default is a runtime
      // decision and this client is not its second owner.
      detail: window(offset, limit),
    };
  },
  renderResult(result, _args) {
    const body = resultText(result);
    if (body.length === 0) {
      return undefined;
    }
    return { detail: body };
  },
};

function window(offset: number | undefined, limit: number | undefined): string[] {
  if (offset !== undefined && limit !== undefined) {
    return [role.meta(`lines ${offset}–${offset + limit - 1}`)];
  }
  if (offset !== undefined) {
    return [role.meta(`from line ${offset}`)];
  }
  if (limit !== undefined) {
    return [role.meta(`first ${limit} lines`)];
  }
  return [];
}

const grepRenderer: ToolPresentationRenderer = {
  renderCall(args) {
    const fields = record(args);
    const pattern = text(fields?.["pattern"]);
    if (pattern === undefined) {
      return undefined;
    }
    const scope: string[] = [];
    const path = text(fields?.["path"]);
    if (path !== undefined) {
      scope.push(path);
    }
    const glob = text(fields?.["glob"]);
    if (glob !== undefined) {
      scope.push(glob);
    }
    if (fields?.["literal"] === true) {
      scope.push("literal");
    }
    if (fields?.["ignoreCase"] === true) {
      scope.push("ignore case");
    }
    return {
      title: "Grep",
      subject: style.yellow(JSON.stringify(pattern)),
      detail: scope.length === 0 ? [] : [role.meta(scope.join(" · "))],
    };
  },
  renderResult(result, _args) {
    const body = resultText(result);
    if (body.length === 0) {
      return undefined;
    }
    return { detail: body };
  },
};

const globRenderer: ToolPresentationRenderer = {
  renderCall(args) {
    const fields = record(args);
    const pattern = text(fields?.["pattern"]);
    if (pattern === undefined) {
      return undefined;
    }
    const path = text(fields?.["path"]);
    return {
      title: "Glob",
      subject: style.yellow(pattern),
      detail: path === undefined ? [] : [role.meta(path)],
    };
  },
  renderResult(result, _args) {
    const body = resultText(result);
    if (body.length === 0) {
      return undefined;
    }
    return { detail: body };
  },
};

const editRenderer: ToolPresentationRenderer = {
  renderCall(args) {
    const fields = record(args);
    const path = text(fields?.["path"]);
    const edits = fields?.["edits"];
    if (path === undefined || !Array.isArray(edits)) {
      return undefined;
    }
    // The diff is built only from what the model published. The workspace is
    // never read to reconstruct surrounding context — the TUI has no
    // filesystem access and must not acquire one to draw a nicer card.
    const detail: string[] = [
      role.meta(
        `${edits.length} ${edits.length === 1 ? "replacement" : "replacements"}`,
      ),
    ];
    for (const edit of edits) {
      const replacement = record(edit);
      const oldText = text(replacement?.["oldText"]);
      const newText = text(replacement?.["newText"]);
      if (oldText === undefined || newText === undefined) {
        continue;
      }
      detail.push(...toLines(oldText).map((line) => style.red(`- ${line}`)));
      detail.push(...toLines(newText).map((line) => style.green(`+ ${line}`)));
    }
    // A large replacement produces a large diff. It is call detail, so the
    // card shell bounds it: a collapsed Edit card never dumps a whole file.
    return { title: "Edit", subject: path, detail };
  },
  renderResult(result, _args) {
    const body = resultText(result);
    if (body.length === 0) {
      return undefined;
    }
    return { detail: body };
  },
};

const writeRenderer: ToolPresentationRenderer = {
  renderCall(args) {
    const fields = record(args);
    const path = text(fields?.["path"]);
    const content = text(fields?.["content"]);
    if (path === undefined || content === undefined) {
      return undefined;
    }
    const lineCount = toLines(content).length;
    return {
      title: "Write",
      subject: path,
      detail: [
        role.meta(`${lineCount} ${lineCount === 1 ? "line" : "lines"}`),
      ],
    };
  },
  renderResult(result, _args) {
    const body = resultText(result);
    if (body.length === 0) {
      return undefined;
    }
    return { detail: body };
  },
};

/**
 * The task list.
 *
 * Every settled `todo` result carries the complete list as structured JSON so
 * the panel above the editor can be derived from it. Drawing that JSON in the
 * transcript as well would print the whole plan twice on every call, so the
 * card shows the runtime's own one-line summary of what the call did and
 * leaves the list to the panel.
 */
const todoRenderer: ToolPresentationRenderer = {
  renderCall(args) {
    const fields = record(args);
    const action = text(fields?.["action"]);
    if (action === undefined) {
      return undefined;
    }
    const id = count(fields?.["id"]);
    const subject = text(fields?.["subject"]);
    const status = text(fields?.["status"]);
    const parts = [
      id === undefined ? undefined : `#${id}`,
      subject,
      status === undefined ? undefined : `→ ${status}`,
    ].filter((part): part is string => part !== undefined);
    return {
      title: "Todo",
      subject: `${style.yellow(action)}${parts.length === 0 ? "" : ` ${parts.join(" ")}`}`,
      detail: [],
    };
  },
  renderResult(result, _args) {
    const body = resultText(result);
    return body.length === 0 ? undefined : { summary: body, detail: [] };
  },
};

/**
 * The renderer registry.
 *
 * Small on purpose. A tool without an entry is not degraded — it renders
 * through {@link genericRenderer}, which is a first-class presentation, not a
 * placeholder. MCP tools — including managed Python tool packages, which
 * surface as MCP-origin — and future tool sources are expected to live here
 * forever.
 */
const RENDERERS: ReadonlyMap<ToolId, ToolPresentationRenderer> = new Map([
  ["tool-bash", bashRenderer],
  ["tool-read", readRenderer],
  ["tool-grep", grepRenderer],
  ["tool-glob", globRenderer],
  ["tool-edit", editRenderer],
  ["tool-write", writeRenderer],
  ["tool-todo", todoRenderer],
]);

/** The renderer for one tool identity, or the generic one. */
export function rendererFor(toolId: ToolId): ToolPresentationRenderer {
  return RENDERERS.get(toolId) ?? genericRenderer;
}

/** Whether a tool identity has a specialized renderer. Presentation only. */
export function hasSpecializedRenderer(toolId: ToolId): boolean {
  return RENDERERS.has(toolId);
}
