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
 * out, or was interrupted is decided before a renderer is ever consulted, in
 * Rust, and is rendered by the card shell in {@link ./tool-card.ts}. No
 * function here receives the lifecycle, so none of them can express an
 * opinion about it.
 *
 * A renderer may:
 *   format a title, the published arguments, the normalized result content,
 *   a deterministic diff derivable from those arguments, and a bounded
 *   preview of any of it.
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
import { role, style } from "../theme.ts";

/** What a renderer says about the call itself. */
export interface ToolCallPresentation {
  /** The display title, e.g. `Bash`. */
  title: string;
  /** The one-line subject, e.g. `$ cargo test --all`. */
  subject?: string;
  /** Further lines describing the call, e.g. a deterministic diff. */
  lines?: string[];
}

/** How much of a long body a collapsed card shows. */
export interface ToolRenderContext {
  expanded: boolean;
  previewLines: number;
}

/**
 * One tool's presentation adapter.
 *
 * Both methods are total over "shapes I understand" and return `undefined`
 * for everything else, which is what makes the generic fallback mandatory
 * rather than aspirational.
 */
export interface ToolPresentationRenderer {
  renderCall(args: unknown): ToolCallPresentation | undefined;
  renderResult?(
    result: ToolExecutionResult,
    args: unknown,
    context: ToolRenderContext,
  ): string[] | undefined;
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
 * Bounds a body for a collapsed card.
 *
 * This is a *client visual collapse* and says nothing about the runtime's own
 * `TruncationState`, which the card reports separately and unconditionally.
 */
export function preview(
  lines: string[],
  context: ToolRenderContext,
): string[] {
  if (context.expanded || lines.length <= context.previewLines) {
    return lines;
  }
  const hidden = lines.length - context.previewLines;
  return [
    ...lines.slice(0, context.previewLines),
    role.meta(`… ${hidden} more ${hidden === 1 ? "line" : "lines"} · ctrl+o to expand`),
  ];
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
 * rustX already has heterogeneous tool sources — native, MCP, Python, and
 * whatever a future extension registers — so this is not a courtesy path. It
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
      return { title: "", lines: formatJson(args) };
    }
    const keys = Object.keys(fields);
    if (keys.length === 0) {
      return { title: "" };
    }
    return { title: "", lines: formatJson(fields) };
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
  context: ToolRenderContext,
): string[] {
  const lines = resultText(result);
  const json = resultJson(result);
  if (json !== undefined) {
    lines.push(...formatJson(json));
  }
  for (const content of result.content ?? []) {
    if (content.type === "file") {
      lines.push(role.meta("(file)"));
    }
    if (content.type === "image") {
      lines.push(role.meta("(image)"));
    }
  }
  return preview(lines, context);
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
      lines: rest.map((line) => `  ${line}`),
    };
  },
  renderResult(result, _args, context) {
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
      return [role.meta("(no output)")];
    }
    return preview(body, context);
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
      lines: window(offset, limit),
    };
  },
  renderResult(result, _args, context) {
    const body = resultText(result);
    if (body.length === 0) {
      return undefined;
    }
    return preview(body, context);
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
      lines: scope.length === 0 ? [] : [role.meta(scope.join(" · "))],
    };
  },
  renderResult(result, _args, context) {
    const payload = record(resultJson(result));
    const matches = payload?.["matches"];
    if (!Array.isArray(matches)) {
      return undefined;
    }
    const summary = role.meta(
      `${matches.length} ${matches.length === 1 ? "match" : "matches"}`,
    );
    const rows = matches.map((match) => {
      const entry = record(match);
      const path = text(entry?.["path"]) ?? "";
      const line = count(entry?.["line"]);
      const body = text(entry?.["text"]) ?? "";
      return `${role.accent(path)}${line === undefined ? "" : role.chrome(`:${line}`)} ${body.trim()}`;
    });
    return [summary, ...preview(rows, context)];
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
      lines: path === undefined ? [] : [role.meta(path)],
    };
  },
  renderResult(result, _args, context) {
    const payload = record(resultJson(result));
    const results = payload?.["results"];
    if (!Array.isArray(results)) {
      return undefined;
    }
    const summary = role.meta(
      `${results.length} ${results.length === 1 ? "path" : "paths"}`,
    );
    return [
      summary,
      ...preview(
        results.map((entry) => text(entry) ?? String(entry)),
        context,
      ),
    ];
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
    const lines: string[] = [
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
      lines.push(...toLines(oldText).map((line) => style.red(`- ${line}`)));
      lines.push(...toLines(newText).map((line) => style.green(`+ ${line}`)));
    }
    return { title: "Edit", subject: path, lines };
  },
  renderResult(result, _args) {
    const payload = record(resultJson(result));
    const replacements = count(payload?.["replacements"]);
    if (replacements === undefined) {
      return undefined;
    }
    return [
      role.meta(
        `applied ${replacements} ${replacements === 1 ? "replacement" : "replacements"}`,
      ),
    ];
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
      lines: [
        role.meta(`${lineCount} ${lineCount === 1 ? "line" : "lines"}`),
      ],
    };
  },
  renderResult(result, _args) {
    const payload = record(resultJson(result));
    const bytes = count(payload?.["bytes_written"]);
    const path = text(payload?.["path"]);
    if (bytes === undefined) {
      return undefined;
    }
    return [role.meta(`wrote ${bytes} bytes${path === undefined ? "" : ` to ${path}`}`)];
  },
};

/**
 * The renderer registry.
 *
 * Small on purpose. A tool without an entry is not degraded — it renders
 * through {@link genericRenderer}, which is a first-class presentation, not a
 * placeholder. Python, MCP, and future tool sources are expected to live
 * here forever.
 */
const RENDERERS: ReadonlyMap<ToolId, ToolPresentationRenderer> = new Map([
  ["tool-bash", bashRenderer],
  ["tool-read", readRenderer],
  ["tool-grep", grepRenderer],
  ["tool-glob", globRenderer],
  ["tool-edit", editRenderer],
  ["tool-write", writeRenderer],
]);

/** The renderer for one tool identity, or the generic one. */
export function rendererFor(toolId: ToolId): ToolPresentationRenderer {
  return RENDERERS.get(toolId) ?? genericRenderer;
}

/** Whether a tool identity has a specialized renderer. Presentation only. */
export function hasSpecializedRenderer(toolId: ToolId): boolean {
  return RENDERERS.has(toolId);
}
