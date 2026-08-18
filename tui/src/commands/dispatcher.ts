/**
 * The command/input dispatcher.
 *
 * ```text
 * editor submission
 *   -> parseCommandLine
 *        |
 *        +-- a command  -> render projection state, or invoke ONE canonical
 *        |                 Runtime Client operation
 *        +-- plain text -> submit_inbound
 * ```
 *
 * The dispatcher produces *outcomes* — text to show, an overlay to open, a
 * quit request — rather than touching the terminal itself. That keeps it
 * testable without a real terminal and keeps Pi at the outermost layer.
 *
 * What it must never do: read `models.json`, resolve a credential, execute a
 * tool, read a `SKILL.md`, compose an Agent Status, drain a mailbox, or reach
 * a provider. Every one of those is Rust-owned, and several are reachable
 * only through operations this file calls.
 */

import type { RuntimeClientSession } from "../runtime/session.ts";
import { RuntimeRequestError } from "../runtime/connection.ts";
import {
  activeBackground,
  capabilitySummary,
  catalogRows,
  describeReasoning,
  originLabel,
  outcomeLabel,
  skills,
  toolsByOrigin,
  unavailableInputModalities,
} from "../presentation/selectors.ts";
import type { PresentationState } from "../presentation/state.ts";
import { COMMANDS, parseCommandLine } from "./registry.ts";
import type { CatalogModelView } from "../protocol/types.ts";

/** What the dispatcher wants the UI to do next. */
export type CommandOutcome =
  | { kind: "none" }
  | { kind: "message"; level: "info" | "error"; text: string }
  | {
      kind: "choose_model";
      models: CatalogModelView[];
      rows: Array<{ value: string; label: string; description: string }>;
    }
  | { kind: "quit" };

export interface DispatcherContext {
  session: RuntimeClientSession;
  /** Bounded diagnostics the UI owns, surfaced by `/debug`. */
  diagnostics: () => DebugDiagnostics;
}

/**
 * Bounded client-side diagnostics.
 *
 * Presentation and protocol facts only. No credential value ever appears
 * here, and no field is composed from anything but observed state.
 */
export interface DebugDiagnostics {
  attachmentId?: string;
  conversationId?: string;
  agentId?: string;
  cursor?: number;
  connectionState: string;
  childStatus: string;
  stderrTail: string;
  stderrTruncatedBytes: number;
  pendingRequests: number;
  resyncCount: number;
}

export class CommandDispatcher {
  readonly #context: DispatcherContext;

  constructor(context: DispatcherContext) {
    this.#context = context;
  }

  /**
   * Handles one editor submission.
   *
   * Plain text becomes one `submit_inbound`. The runtime owns the resulting
   * message id, inbound sequence, timestamp, and provenance; this client
   * never fabricates any of them.
   */
  async submit(line: string): Promise<CommandOutcome> {
    const command = parseCommandLine(line);
    if (command === undefined) {
      const text = line.trim();
      if (text.length === 0) {
        return { kind: "none" };
      }
      try {
        await this.#context.session.submitInbound([{ type: "text", text }]);
        return { kind: "none" };
      } catch (error) {
        return failure(error);
      }
    }
    return this.#dispatch(command.name, command.argument);
  }

  async #dispatch(name: string, argument: string): Promise<CommandOutcome> {
    const state = this.#context.session.state;
    if (state === undefined) {
      return { kind: "message", level: "error", text: "not attached yet" };
    }

    try {
      switch (name) {
        case "/help":
          return info(renderHelp());
        case "/model":
          return await this.#model(state, argument);
        case "/tools":
          return info(renderTools(state));
        case "/skills":
          return info(renderSkills(state));
        case "/status":
          return info(renderStatus(state));
        case "/debug":
          return info(renderDebug(state, this.#context.diagnostics()));
        case "/cancel":
          return await this.#cancel(argument);
        case "/quit":
          return { kind: "quit" };
        default:
          return {
            kind: "message",
            level: "error",
            text: `unknown command ${name}. Try /help.`,
          };
      }
    } catch (error) {
      return failure(error);
    }
  }

  /**
   * `/model` — a presentation over the runtime's authoritative model
   * operations.
   *
   * With no argument it renders `model_get`. With one it reads the catalog
   * through `model_catalog_get`, then replaces the whole session
   * configuration through `model_set`. It never parses a provider catalog
   * file, never touches a provider SDK, and never resolves an API key.
   */
  async #model(
    state: PresentationState,
    argument: string,
  ): Promise<CommandOutcome> {
    if (argument.length === 0) {
      return info(renderModel(state));
    }

    const catalog = await this.#context.session.modelCatalog();
    const models = catalog.models ?? [];
    if (argument === "list") {
      return { kind: "choose_model", models, rows: catalogRows(models) };
    }

    const chosen = models.find((model) => model.model === argument);
    if (chosen === undefined) {
      const known = models.map((model) => model.model).join(", ");
      return {
        kind: "message",
        level: "error",
        text: `${argument} is not in the runtime's catalog. Selectable: ${known || "none"}`,
      };
    }
    return this.selectModel(chosen);
  }

  /**
   * Applies one catalog selection as a whole-state replacement.
   *
   * The update affects future admissions only. An already-admitted attempt
   * keeps the model it froze — the runtime enforces that, and the client
   * simply reports both facts truthfully.
   */
  async selectModel(model: CatalogModelView): Promise<CommandOutcome> {
    const current = this.#context.session.state?.sessionModel.configured;
    if (current === undefined) {
      return { kind: "message", level: "error", text: "not attached yet" };
    }

    // `/model X` is a deliberate whole-state replacement: the selected
    // primary model gets its own runtime defaults, while the independently
    // configured summary policy is copied from the authoritative current
    // configuration unchanged.
    const replacement = {
      model: model.model,
      reasoningProfile: model.defaultReasoningProfile,
      requestParams: {},
      summaryModel: current.summaryModel,
    };
    const updated = await this.#context.session.modelSet({
      ...replacement,
    });

    const attempt = this.#context.session.state?.attempt;
    const note =
      attempt !== undefined && attempt.phase.type === "running"
        ? `\nThe running attempt stays on ${attempt.model.primary.model}; the change applies to the next attempt.`
        : "";
    return info(
      `session model is now ${updated.configured.model}\nprimary overrides reset to the selected model defaults; summary model policy preserved\n${capabilitySummary(updated.effective)}${note}`,
    );
  }

  async #cancel(argument: string): Promise<CommandOutcome> {
    if (argument.length > 0) {
      // Cancellation of one background execution is a request. Acceptance is
      // not settlement: the terminal fact arrives later, from the runtime.
      const accepted = await this.#context.session.cancelBackground(argument);
      return info(
        `cancellation requested for ${accepted.execution_id} (registry state: ${accepted.state})\nThis is acceptance, not settlement.`,
      );
    }
    const attemptId = await this.#context.session.cancelCurrentAttempt();
    return info(
      `cancellation requested for attempt ${attemptId}\nThis is acceptance; the runtime owns the terminal settlement.`,
    );
  }
}

function info(text: string): CommandOutcome {
  return { kind: "message", level: "info", text };
}

function failure(error: unknown): CommandOutcome {
  if (error instanceof RuntimeRequestError) {
    return { kind: "message", level: "error", text: error.message };
  }
  return {
    kind: "message",
    level: "error",
    text: (error as Error).message ?? String(error),
  };
}

// ---------------------------------------------------------------------------
// Renderers — Markdown text built purely from projection state
// ---------------------------------------------------------------------------

export function renderHelp(): string {
  const rows = COMMANDS.map((command) => {
    const spelling =
      command.argumentHint === undefined
        ? command.name
        : `${command.name} ${command.argumentHint}`;
    return `- \`${spelling}\` — ${command.description}`;
  });
  return [
    "### Commands",
    ...rows,
    "",
    "Anything else is submitted to the runtime as an inbound message.",
  ].join("\n");
}

/**
 * `/model` — the authoritative session model, and the running attempt's
 * frozen model when they differ.
 */
export function renderModel(state: PresentationState): string {
  const session = state.sessionModel;
  const lines = [
    "### Session model",
    `- configured: \`${session.configured.model}\``,
    `- effective: \`${session.effective.model}\` via ${session.effective.protocol}`,
    `- context window: ${session.effective.contextWindow}`,
    `- max output tokens: ${session.effective.maxOutputTokens} (model maximum ${session.effective.modelMaxOutputTokens})`,
    `- reasoning: ${describeReasoning(session.effective)}`,
    `- capabilities: ${capabilitySummary(session.effective)}`,
  ];

  const unavailable = unavailableInputModalities(session.effective);
  if (unavailable.length > 0) {
    // Only effective capability is advertised; the declaration explains why
    // something the catalog claims is not offered.
    lines.push(
      `- declared but not usable today: input ${unavailable.join(", ")}`,
    );
  }

  const params = session.effective.requestParams ?? {};
  if (Object.keys(params).length > 0) {
    // Opaque provider-owned configuration: displayed, never interpreted.
    lines.push(
      "- request parameters (provider-owned, opaque):",
      "```json",
      JSON.stringify(params, null, 2),
      "```",
    );
  }

  lines.push(
    `- summary model: ${
      session.summary.mode === "session"
        ? "follows the attempt's primary model"
        : `\`${session.summary.model}\``
    }`,
  );

  const attempt = state.attempt;
  if (attempt !== undefined) {
    lines.push(
      "",
      "### Active attempt model (frozen at admission)",
      `- attempt: \`${attempt.attemptId}\` (${attempt.phase.type})`,
      `- model: \`${attempt.model.primary.model}\``,
    );
    if (attempt.model.primary.model !== session.effective.model) {
      lines.push(
        `- the session moved to \`${session.effective.model}\`; this attempt keeps the model it froze.`,
      );
    }
  }

  lines.push("", "Use `/model <provider/model>` or `/model list` to change it.");
  return lines.join("\n");
}

/** `/tools` — the capability projection's tool catalog, generically. */
export function renderTools(state: PresentationState): string {
  const groups = toolsByOrigin(state);
  if (groups.length === 0) {
    return "No tools in the active capability set.";
  }
  const lines = [`### Tools (capability revision ${state.capabilities.revision})`];
  for (const group of groups) {
    lines.push("", `**${group.origin}**`);
    for (const tool of group.tools) {
      lines.push(
        `- \`${tool.name}\` — ${tool.description}`,
        `  - execution: ${tool.execution_policy}, concurrency: ${tool.concurrency_policy}, replay: ${tool.replay_policy}`,
        `  - origin: ${originLabel(tool.origin)}`,
      );
    }
  }
  return lines.join("\n");
}

/** `/skills` — the runtime's Skill projection. No SKILL.md is ever read. */
export function renderSkills(state: PresentationState): string {
  const catalog = skills(state);
  if (catalog.length === 0) {
    return "No Skills in the active capability set.";
  }
  return [
    `### Skills (capability revision ${state.capabilities.revision})`,
    ...catalog.map(
      (skill) =>
        `- \`${skill.name}\` (${skill.version_id}) — ${skill.description}`,
    ),
  ].join("\n");
}

/**
 * `/status` — the runtime-composed Agent Status plus runtime diagnostics.
 *
 * The rendering is the runtime's own; this client never composes an Agent
 * Status and never parses the rendered text to recover structure.
 */
export function renderStatus(state: PresentationState): string {
  const lines = ["### Runtime"];
  lines.push(`- conversation: \`${state.conversationId}\``);
  lines.push(`- capability revision: ${state.capabilities.revision}`);
  lines.push(
    `- session model: \`${state.sessionModel.configured.model}\``,
  );
  lines.push(...contextDiagnosticsLines(state));

  const attempt = state.attempt;
  lines.push(
    attempt === undefined
      ? "- attempt: none"
      : `- attempt: \`${attempt.attemptId}\` ${
          attempt.phase.type === "settled"
            ? outcomeLabel(attempt.phase.outcome)
            : attempt.phase.type
        } (turn ${attempt.turn}, model \`${attempt.model.primary.model}\`)`,
  );

  const pending = state.inbound.pending ?? [];
  lines.push(`- inbound pending: ${pending.length}`);
  if (state.inbound.last_drain !== undefined) {
    lines.push(
      `- last drain: watermark ${state.inbound.last_drain.watermark}, ${state.inbound.last_drain.count} item(s)`,
    );
  }
  const active = activeBackground(state);
  lines.push(
    `- background: ${active.length} active of ${state.background.length} known`,
  );
  if (state.runtimeShutdown) {
    lines.push("- runtime is draining; conversation-owned work is settling");
  }

  if (state.status === undefined) {
    lines.push("", "No Agent Status has been composed yet.");
    return lines.join("\n");
  }

  lines.push(
    "",
    `### Agent Status (attempt \`${state.status.attempt_id}\`, turn ${state.status.turn})`,
    "```",
    state.status.rendered,
    "```",
  );
  return lines.join("\n");
}

/** `/debug` — bounded presentation and protocol diagnostics. */
export function renderDebug(
  state: PresentationState,
  diagnostics: DebugDiagnostics,
): string {
  const lines = [
    "### Client diagnostics",
    `- attachment: \`${diagnostics.attachmentId ?? "none"}\``,
    `- conversation: \`${diagnostics.conversationId ?? "none"}\``,
    `- agent: \`${diagnostics.agentId ?? "none"}\``,
    `- cursor: ${diagnostics.cursor ?? state.cursor}`,
    `- connection: ${diagnostics.connectionState}`,
    `- child: ${diagnostics.childStatus}`,
    `- pending requests: ${diagnostics.pendingRequests}`,
    `- authoritative repairs (resync): ${diagnostics.resyncCount}`,
    "",
    "### Runtime projection",
    `- desired session model: \`${state.sessionModel.configured.model}\``,
    `- active attempt model: \`${state.attempt?.model.primary.model ?? "none"}\``,
    `- capability revision: ${state.capabilities.revision}`,
    ...contextDiagnosticsLines(state),
    `- inbound pending: ${(state.inbound.pending ?? []).length}`,
    `- background executions: ${state.background.length} (${activeBackground(state).length} active)`,
    `- transcript entries: ${state.transcript.length}`,
    `- unacknowledged local submissions: ${state.pendingSubmissions.length}`,
  ];

  if (diagnostics.stderrTail.length > 0) {
    // A bounded tail. Startup diagnostics from rustX never carry a credential,
    // and this client adds none.
    lines.push(
      "",
      `### Runtime stderr (last ${diagnostics.stderrTail.length} bytes, ${diagnostics.stderrTruncatedBytes} dropped)`,
      "```",
      diagnostics.stderrTail,
      "```",
    );
  }
  return lines.join("\n");
}

function contextDiagnosticsLines(state: PresentationState): string[] {
  const context = state.context;
  const latest = context.latest_compaction;
  if (latest === undefined) {
    return [`- context compactions: ${context.compaction_count}`];
  }
  return [
    `- context compactions: ${context.compaction_count} (latest generation ${latest.generation}, surface revision ${latest.surface_revision})`,
    `- latest context measurement: ${latest.tokens_before.input_tokens} tokens before (${latest.tokens_before.source}), ${latest.estimated_tokens_after} estimated after`,
  ];
}
