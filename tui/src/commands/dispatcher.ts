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
 * The dispatcher produces *typed presentation intents* — an inspection view,
 * transient feedback, an overlay to open, or a quit request — rather than
 * touching the terminal itself. That keeps it testable without a real
 * terminal and keeps Pi at the outermost layer.
 *
 * What it must never do: read `models.jsonc`, resolve a credential, execute a
 * tool, read a `SKILL.md`, compose an Agent Status, drain a mailbox, or reach
 * a provider. Every one of those is Rust-owned, and several are reachable
 * only through operations this file calls.
 */

import type { RuntimeClientAttachment, SessionSwitch } from "../runtime/attachment.ts";
import { RuntimeRequestError } from "../runtime/connection.ts";
import {
  activeBackground,
  capabilitySummary,
  describeConfiguredReasoning,
  describeReasoning,
  inactiveToolsByOrigin,
  originLabel,
  outcomeLabel,
  sessionLabel,
  skills,
  toolsByOrigin,
  unavailableInputModalities,
} from "../presentation/selectors.ts";
import { selectTodos } from "../presentation/todos.ts";
import type { PresentationState } from "../presentation/state.ts";
import { renderTodoInspection } from "../ui/components/todos.ts";
import { COMMANDS, parseCommandLine } from "./registry.ts";
import type {
  ApprovalDecision,
  ApprovalMode,
  CatalogModelView,
  InteractionId,
  InteractionResponse,
  SessionNodeView,
  SessionSummaryView,
  SessionUserMessageBoundaryView,
  SessionView,
  ToolCallId,
  ToolExecutionId,
} from "../protocol/types.ts";

/** What the dispatcher wants the UI to do next. */
export type CommandOutcome =
  | { kind: "none" }
  | { kind: "inspect"; title: string; body: string }
  | { kind: "transient"; level: "info" | "error"; text: string }
  | { kind: "choose_model"; models: CatalogModelView[] }
  | {
      kind: "choose_session";
      sessions: SessionSummaryView[];
      nextOffset?: number;
      query: string;
    }
  | {
      kind: "choose_fork";
      boundaries: SessionUserMessageBoundaryView[];
      nextOffset?: number;
    }
  | {
      kind: "choose_tree";
      session: SessionView;
      nodes: SessionNodeView[];
      nextNodeOffset?: number;
      boundaries: SessionUserMessageBoundaryView[];
      nextHistoryOffset?: number;
    }
  | { kind: "session_switch"; change: SessionSwitch }
  | { kind: "replacement_required"; message: string }
  | {
      /** A client display preference. Never a runtime request. */
      kind: "preference";
      preference: PreferenceChange;
    }
  | { kind: "quit" };

/**
 * One change to a client presentation preference.
 *
 * These never reach the runtime. `reasoning` here is *display* of reasoning
 * content, which is a different thing from the `reasoningProfile` /
 * `reasoningEnabled` model request configuration `/model` shows.
 */
export type PreferenceChange =
  | { type: "reasoning"; visible?: boolean }
  | { type: "expand"; target: ExpandTarget }
  /** One foreground card, addressed by its `ToolCallId`. */
  | { type: "expand_call"; callId: ToolCallId }
  /** One background card, addressed by its `ToolExecutionId`. */
  | { type: "expand_background"; executionId: ToolExecutionId }
  /** One pending approval card, addressed by its `InteractionId`. */
  | { type: "expand_interaction"; interactionId: InteractionId };

/**
 * A bulk expansion target.
 *
 * `all` and `none` mean every identity domain — every renderable tool card,
 * every renderable background card, and every pending interaction card —
 * because that is what the words say. `latest` deliberately does not: it stays
 * the latest *tool call*, because "the latest" across three unrelated identity
 * domains would name whichever entity a rule picked rather than the one the
 * reader is looking at.
 */
export type ExpandTarget = "all" | "none" | "latest";

export interface DispatcherContext {
  session: RuntimeClientAttachment;
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
  #context: DispatcherContext;

  constructor(context: DispatcherContext) {
    this.#context = context;
  }

  /**
   * Rebinds admission routing after a native process-boundary session switch.
   *
   * This only affects invocations admitted after the rebind. Every public
   * operation captures the attachment at its start and passes that exact
   * attachment through all awaited phases, so an admitted command cannot
   * retarget a newer attachment.
   */
  setSession(session: RuntimeClientAttachment): void {
    this.#context.session = session;
  }

  /**
   * Handles one editor submission.
   *
   * Plain text is always an ordinary inbound message. Questionnaire answers
   * are sent only by the focused questionnaire surface, which constructs the
   * typed whole-questionnaire response.
   */
  async submit(line: string): Promise<CommandOutcome> {
    const session = this.#context.session;
    const command = parseCommandLine(line);
    if (command === undefined) {
      const text = line.trim();
      if (text.length === 0) {
        return { kind: "none" };
      }
      try {
        await session.submitInbound([{ type: "text", text }]);
        return { kind: "none" };
      } catch (error) {
        return failure(error);
      }
    }
    return this.#dispatch(session, command.name, command.argument);
  }

  async #dispatch(
    session: RuntimeClientAttachment,
    name: string,
    argument: string,
  ): Promise<CommandOutcome> {
    const state = session.state;
    if (state === undefined) {
      return transient("error", "not attached yet");
    }

    try {
      switch (name) {
        case "/help":
          return inspect("Help", renderHelp());
        case "/model":
          return await this.#model(session, state, argument);
        case "/new":
          return { kind: "session_switch", change: await session.newSession() };
        case "/resume":
          return await this.#resume(session, argument);
        case "/session":
          return await this.#sessionInfo(session);
        case "/name":
          return await this.#name(session, argument);
        case "/clone":
          return { kind: "session_switch", change: await session.cloneSession() };
        case "/fork":
          return await this.#fork(session);
        case "/tree":
          return await this.#tree(session);
        case "/tools":
          return inspect("Tools", renderTools(state));
        case "/skills":
          return inspect("Skills", renderSkills(state));
        case "/todos":
          return inspect("Todos", renderTodoInspection(selectTodos(state)));
        case "/status":
          return inspect("Runtime status", renderStatus(state));
        case "/compact":
          return await this.#compact(session, argument);
        case "/reload":
          return await this.#reload(session, argument);
        case "/debug":
          return inspect("Client diagnostics", renderDebug(state, this.#context.diagnostics()));
        case "/reasoning":
          return reasoningPreference(argument);
        case "/expand":
          return expandPreference(argument);
        case "/cancel":
          return await this.#cancel(session, argument);
        case "/approve":
          return await this.#approve(session, argument);
        case "/approval":
          return await this.#approvalMode(session, argument);
        case "/quit":
          return { kind: "quit" };
        default:
          return transient("error", `unknown command ${name}. Try /help.`);
      }
    } catch (error) {
      return failure(error);
    }
  }

  /** Selection seams used by the native-data overlays. */
  async selectSession(sessionId: string): Promise<CommandOutcome> {
    const session = this.#context.session;
    return this.#selectSession(session, sessionId);
  }

  async #selectSession(
    session: RuntimeClientAttachment,
    sessionId: string,
  ): Promise<CommandOutcome> {
    try {
      return {
        kind: "session_switch",
        change: await session.selectSession(sessionId),
      };
    } catch (error) {
      return failure(error);
    }
  }

  async selectTreeNode(sessionId: string, nodeId: string): Promise<CommandOutcome> {
    const session = this.#context.session;
    return this.#selectTreeNode(session, sessionId, nodeId);
  }

  async #selectTreeNode(
    session: RuntimeClientAttachment,
    sessionId: string,
    nodeId: string,
  ): Promise<CommandOutcome> {
    try {
      return {
        kind: "session_switch",
        change: await session.selectSession(sessionId, nodeId),
      };
    } catch (error) {
      return failure(error);
    }
  }

  async forkAt(boundary: SessionUserMessageBoundaryView): Promise<CommandOutcome> {
    const session = this.#context.session;
    return this.#forkAt(session, boundary);
  }

  async #forkAt(
    session: RuntimeClientAttachment,
    boundary: SessionUserMessageBoundaryView,
  ): Promise<CommandOutcome> {
    try {
      return {
        kind: "session_switch",
        change: await session.forkSession(
          boundary.surface_revision,
          boundary.message.id,
        ),
      };
    } catch (error) {
      return failure(error);
    }
  }

  async branchAt(boundary: SessionUserMessageBoundaryView): Promise<CommandOutcome> {
    const session = this.#context.session;
    return this.#branchAt(session, boundary);
  }

  async #branchAt(
    session: RuntimeClientAttachment,
    boundary: SessionUserMessageBoundaryView,
  ): Promise<CommandOutcome> {
    try {
      return {
        kind: "session_switch",
        change: await session.branchTree(
          boundary.surface_revision,
          boundary.message.id,
        ),
      };
    } catch (error) {
      return failure(error);
    }
  }

  async #resume(
    session: RuntimeClientAttachment,
    argument: string,
  ): Promise<CommandOutcome> {
    if (argument.length > 0) return this.#selectSession(session, argument);
    const page = await session.listSessions();
    return {
      kind: "choose_session",
      sessions: page.sessions,
      nextOffset: page.nextOffset,
      query: "",
    };
  }

  async #sessionInfo(session: RuntimeClientAttachment): Promise<CommandOutcome> {
    const refreshed = await session.refreshSession();
    return inspect(
      "Session",
      [
        // An unnamed Session has no name line rather than a placeholder one:
        // the identity below is what it is actually known by.
        ...(refreshed.name === undefined ? [] : [`name ${refreshed.name}`]),
        `session ${refreshed.id}`,
        `active node ${refreshed.active_node}`,
        `conversation ${refreshed.active_conversation_id}`,
        `nodes ${refreshed.node_count}`,
      ].join("\n"),
    );
  }

  async #compact(
    session: RuntimeClientAttachment,
    argument: string,
  ): Promise<CommandOutcome> {
    if (argument.length > 0) {
      return transient("error", "usage: /compact");
    }
    const context = await session.compactContext();
    const latest = context.latest_compaction;
    if (latest === undefined) {
      return transient("info", "context compacted");
    }
    return transient(
      "info",
      `context compacted to generation ${latest.generation}: ${latest.tokens_before.input_tokens} → ${latest.estimated_tokens_after} tokens`,
    );
  }

  async #reload(
    session: RuntimeClientAttachment,
    argument: string,
  ): Promise<CommandOutcome> {
    if (argument.length > 0) {
      return transient("error", "usage: /reload");
    }
    const reloaded = await session.reloadResources();
    return transient(
      "info",
      `runtime resources reloaded to generation ${reloaded.resourceRevision} (capabilities ${reloaded.capabilityRevision})`,
    );
  }

  /**
   * `/name` shows the active Session's name, and `/name <text>` sets it.
   *
   * Reporting the current name is the useful answer to a bare `/name`: a
   * Session is unnamed until someone names it, so "this one has no name" is
   * the fact the user is asking about, not a syntax mistake to correct.
   */
  async #name(
    session: RuntimeClientAttachment,
    argument: string,
  ): Promise<CommandOutcome> {
    if (argument.trim().length === 0) {
      const current = await session.refreshSession();
      return transient(
        "info",
        current.name === undefined
          ? `session ${current.id} is unnamed; use /name <text> to name it`
          : `session name: ${current.name}`,
      );
    }
    const named = await session.nameSession(argument);
    return transient("info", `session named ${sessionLabel(named)}`);
  }

  async #fork(session: RuntimeClientAttachment): Promise<CommandOutcome> {
    const tree = await session.sessionTree();
    return {
      kind: "choose_fork",
      boundaries: tree.branchableMessages,
      nextOffset: tree.nextHistoryOffset,
    };
  }

  async #tree(session: RuntimeClientAttachment): Promise<CommandOutcome> {
    const tree = await session.sessionTree();
    return {
      kind: "choose_tree",
      session: tree.session,
      nodes: tree.nodes,
      nextNodeOffset: tree.nextNodeOffset,
      boundaries: tree.branchableMessages,
      nextHistoryOffset: tree.nextHistoryOffset,
    };
  }

  /**
   * `/model` — a presentation over the runtime's authoritative model
   * operations.
   *
   * With no argument it opens the searchable selector over the runtime's
   * `model_catalog_get` result. With `show` it renders the projection's own
   * model view. With a model reference it reads the catalog and replaces the
   * whole runtime configuration through `model_set`. It never parses a
   * provider catalog file, never touches a provider SDK, and never resolves
   * an API key.
   */
  async #model(
    session: RuntimeClientAttachment,
    state: PresentationState,
    argument: string,
  ): Promise<CommandOutcome> {
    // `show` is answered from the projection alone; every other spelling
    // needs the runtime's authoritative catalog.
    if (argument === "show") {
      return inspect("Model", renderModel(state));
    }
    const catalog = await session.modelCatalog();
    const models = catalog.models ?? [];
    if (argument.length === 0 || argument === "list") {
      return { kind: "choose_model", models };
    }

    const chosen = models.find((model) => model.model === argument);
    if (chosen === undefined) {
      const known = models.map((model) => model.model).join(", ");
      return transient(
        "error",
        `${argument} is not in the runtime's catalog. Selectable: ${known || "none"}`,
      );
    }
    return this.#selectModel(session, chosen);
  }

  /**
   * Applies one catalog selection as a whole-state replacement.
   *
   * The update affects future admissions only. An already-admitted attempt
   * keeps the model it froze — the runtime enforces that, and the client
   * simply reports both facts truthfully.
   */
  async selectModel(model: CatalogModelView): Promise<CommandOutcome> {
    const session = this.#context.session;
    return this.#selectModel(session, model);
  }

  async #selectModel(
    session: RuntimeClientAttachment,
    model: CatalogModelView,
  ): Promise<CommandOutcome> {
    try {
      const current = session.state?.sessionModel.configured;
      if (current === undefined) {
        return transient("error", "not attached yet");
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
      const updated = await session.modelSet({
        ...replacement,
      });

      const attempt = session.state?.attempt;
      const attemptNote =
        attempt !== undefined && attempt.phase.type === "running"
          ? `current attempt remains ${attempt.model.primary.model}`
          : "";
      const modelFeedback = attemptNote.length > 0
        ? [
            `session model -> ${updated.configured.model}; ${attemptNote}`,
            "change applies to next attempt; primary overrides reset; summary policy preserved",
            `capabilities: ${capabilitySummary(updated.effective)}`,
          ].join("\n")
        : [
            `session model -> ${updated.configured.model}; primary overrides reset; summary policy preserved`,
            `capabilities: ${capabilitySummary(updated.effective)}`,
          ].join("\n");
      return transient(
        "info",
        modelFeedback,
      );
    } catch (error) {
      return failure(error);
    }
  }

  async #cancel(
    session: RuntimeClientAttachment,
    argument: string,
  ): Promise<CommandOutcome> {
    if (argument.length > 0) {
      // Cancellation of one background execution is a request. Acceptance is
      // not settlement: the terminal fact arrives later, from the runtime.
      const accepted = await session.cancelBackground(argument);
      return transient(
        "info",
        `cancellation requested for ${accepted.execution_id} (registry state: ${accepted.state})\nThis is acceptance, not settlement.`,
      );
    }
    const attemptId = await session.cancelCurrentAttempt();
    return transient(
      "info",
      `cancellation requested for attempt ${attemptId}\nThis is acceptance; the runtime owns the terminal settlement.`,
    );
  }

  async #approve(
    session: RuntimeClientAttachment,
    argument: string,
  ): Promise<CommandOutcome> {
    const parts = argument.split(/\s+/).filter((part) => part.length > 0);
    if (parts.length < 2) {
      return transient("error", "usage: /approve <interaction-id> <allow|deny> [reason]");
    }
    const interactionId = parts[0]!;
    const decision = parts[1]!;
    const reasonParts = parts.slice(2);
    let approval: ApprovalDecision;
    if (decision === "allow") {
      if (reasonParts.length > 0) {
        return transient("error", "allow does not accept a replacement argument or reason");
      }
      approval = { type: "allow" };
    } else if (decision === "deny") {
      approval = {
        type: "deny",
        reason: reasonParts.join(" ") || "denied by Runtime Client",
      };
    } else {
      return transient("error", "usage: /approve <interaction-id> <allow|deny> [reason]");
    }
    const response: InteractionResponse = { type: "approval", decision: approval };
    await session.respondInteraction(interactionId, response);
    return transient("info", `response accepted for interaction ${interactionId}`);
  }

  async #approvalMode(
    session: RuntimeClientAttachment,
    argument: string,
  ): Promise<CommandOutcome> {
    let mode: ApprovalMode;
    if (argument === "policy") {
      mode = "policy";
    } else if (argument === "full_access") {
      mode = "full_access";
    } else {
      return transient("error", "usage: /approval <policy|full_access>");
    }
    const result = await session.approvalModeSet(mode);
    if (result.pendingApprovalMode !== undefined) {
      return transient(
        "info",
        `ApprovalMode request accepted: effective ${result.effectiveApprovalMode}, pending ${result.pendingApprovalMode}`,
      );
    }
    return transient("info", `ApprovalMode is now ${result.effectiveApprovalMode}`);
  }
}

/**
 * `/reasoning [on|off]` — a display preference, applied by the UI.
 *
 * It changes what is drawn and nothing else. The model's reasoning request
 * configuration lives in `SessionModelConfig.reasoningProfile` and is only
 * changeable through `model_set`.
 */
function reasoningPreference(argument: string): CommandOutcome {
  switch (argument) {
    case "":
      return { kind: "preference", preference: { type: "reasoning" } };
    case "on":
      return {
        kind: "preference",
        preference: { type: "reasoning", visible: true },
      };
    case "off":
      return {
        kind: "preference",
        preference: { type: "reasoning", visible: false },
      };
    default:
      return transient("error", "usage: /reasoning [on|off]");
  }
}

/**
 * `/expand` — a visual collapse preference over all three identity domains.
 *
 * ```text
 * /expand                          toggle the latest tool call
 * /expand latest                   the same
 * /expand all                      expand every tool, background, and
 *                                  interaction card
 * /expand none                     collapse all three domains
 * /expand <tool-call-id>           toggle one foreground card
 * /expand background <exec-id>     toggle one background card
 * /expand interaction <interaction-id>  toggle one pending approval card
 * ```
 *
 * A bare id addresses the `ToolCallId` domain, always. There is no search
 * across the namespaces and no "first match wins": the three domains are
 * distinct rustX identities, so addressing a background execution or a pending
 * interaction says so.
 *
 * Expanding shows more of a call, a result, or a pending approval request the
 * client already holds. It never re-executes a tool, never re-reads anything,
 * never re-queries the runtime, and never undoes the runtime's own result
 * truncation, which is a separate fact the card always reports.
 */
function expandPreference(argument: string): CommandOutcome {
  if (argument === "" || argument === "latest") {
    return { kind: "preference", preference: { type: "expand", target: "latest" } };
  }
  if (argument === "all" || argument === "none") {
    return { kind: "preference", preference: { type: "expand", target: argument } };
  }
  const [head, ...rest] = argument.split(/\s+/);
  if (head === "background" || head === "bg") {
    const executionId = rest.join(" ");
    if (executionId.length === 0) {
      return usage("/expand background <execution-id>");
    }
    return {
      kind: "preference",
      preference: { type: "expand_background", executionId },
    };
  }
  if (head === "interaction") {
    const interactionId = rest.join(" ");
    if (interactionId.length === 0) {
      return usage("/expand interaction <interaction-id>");
    }
    return {
      kind: "preference",
      preference: { type: "expand_interaction", interactionId },
    };
  }
  return { kind: "preference", preference: { type: "expand_call", callId: argument } };
}

function usage(spelling: string): CommandOutcome {
  return transient("error", `usage: ${spelling}`);
}

function inspect(title: string, body: string): CommandOutcome {
  return { kind: "inspect", title, body };
}

function transient(
  level: "info" | "error",
  text: string,
): CommandOutcome {
  return { kind: "transient", level, text };
}

function failure(error: unknown): CommandOutcome {
  if (error instanceof RuntimeRequestError) {
    if (error.error.type === "session_restart_required") {
      return { kind: "replacement_required", message: error.error.message };
    }
    return transient("error", compactDiagnostic(error.message));
  }
  return transient("error", compactDiagnostic(error));
}

/** Transient errors keep their distinguishing identity near the front. */
function compactDiagnostic(value: unknown): string {
  const text = value instanceof Error ? value.message : String(value);
  return text.replace(/\s*\r?\n\s*/g, " · ").trim();
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
    "Plain text answers the focused runtime interaction when one is pending; otherwise it is submitted as an inbound message.",
  ].join("\n");
}

/**
 * `/model` — the authoritative session model, and the running attempt's
 * frozen model.
 *
 * Three model identities and two reasoning facts, each named for what it is:
 *
 * ```text
 * configured            SessionModelView.configured.model
 * effective             SessionModelView.effective.model
 * attempt               AttemptModelView.primary.model
 * configured reasoning  SessionModelConfig.reasoningProfile
 * effective reasoning   ModelInvocationView.reasoningProfile/reasoningEnabled
 * ```
 *
 * They are always all printed, even when they coincide, because `/model show`
 * is the place a user goes to find out whether they do.
 */
export function renderModel(state: PresentationState): string {
  const session = state.sessionModel;
  const lines = [
    "### Session model",
    `- configured: \`${session.configured.model}\``,
    `- effective: \`${session.effective.model}\` via ${session.effective.protocol}`,
    `- context window: ${session.effective.contextWindow}`,
    `- max output tokens: ${session.effective.maxOutputTokens} (model maximum ${session.effective.modelMaxOutputTokens})`,
    // Configured and effective reasoning are separate facts: the session asks,
    // the runtime resolves, and a catalog default is neither of them.
    `- configured reasoning: ${describeConfiguredReasoning(session.configured)}`,
    `- effective reasoning: ${describeReasoning(session.effective)}`,
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
      `- reasoning: ${describeReasoning(attempt.model.primary)}`,
    );
    if (attempt.model.primary.model !== session.effective.model) {
      lines.push(
        `- the session's effective model is \`${session.effective.model}\`; this attempt keeps the model it froze.`,
      );
    }
    if (session.configured.model !== session.effective.model) {
      lines.push(
        `- the session is configured for \`${session.configured.model}\`, which is not what it would use today.`,
      );
    }
  }

  lines.push(
    "",
    "Use `/model` for the searchable selector, or `/model <provider/model>` to select directly.",
  );
  return lines.join("\n");
}

/** `/tools` — the capability projection's tool catalog, generically. */
export function renderTools(state: PresentationState): string {
  const activeGroups = toolsByOrigin(state);
  const inactiveGroups = inactiveToolsByOrigin(state);
  if (activeGroups.length === 0 && inactiveGroups.length === 0) {
    return "No available tools in the capability set.";
  }
  const lines = [`### Tools (capability revision ${state.capabilities.revision})`];
  appendToolGroups(lines, "Active tools", activeGroups);
  appendToolGroups(lines, "Available but inactive", inactiveGroups);
  return lines.join("\n");
}

function appendToolGroups(
  lines: string[],
  heading: string,
  groups: Array<{ origin: string; tools: import("../protocol/types.ts").RuntimeClientTool[] }>,
): void {
  lines.push("", `### ${heading}`);
  if (groups.length === 0) {
    lines.push("- none");
    return;
  }
  for (const group of groups) {
    lines.push("", `**${group.origin}**`);
    for (const tool of group.tools) {
      lines.push(
        `- \`${tool.name}\` — ${tool.description}`,
        `  - execution: ${tool.execution_policy}, concurrency: ${tool.concurrency_policy}, approval: ${tool.approval_policy}, replay: ${tool.replay_policy}`,
        `  - origin: ${originLabel(tool.origin)}`,
      );
    }
  }
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
        `- \`${skill.name}\` (${skill.version_id}) — ${skill.description}\n  - location: \`${skill.location}\``,
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
