import { isAttemptActive } from "../presentation/state.ts";
import { archiveDestination } from "../app-server/archive.ts";
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
 * What it must never do: read `rustx.toml`, resolve a credential, execute a
 * tool, read a `SKILL.md`, compose an Agent Status, drain a mailbox, or reach
 * a provider. Every one of those is Rust-owned, and several are reachable
 * only through operations this file calls.
 */

import type { AppServerHost } from "../app-server/host.ts";
import type { AppServerSession } from "../app-server/session.ts";
import {
  AppServerRequestError,
  UncertainOutcomeError,
} from "../app-server/client.ts";
import {
  activeBackground,
  capabilitySummary,
  describeConfiguredReasoning,
  describeReasoning,
  inactiveToolsByOrigin,
  latestAgentStatus,
  originLabel,
  outcomeLabel,
  sessionLabel,
  skills,
  toolsByOrigin,
  unavailableInputModalities,
} from "../presentation/selectors.ts";
import { isTodoComposed, selectTodos } from "../presentation/todos.ts";
import type { PresentationState } from "../presentation/state.ts";
import { renderAgentStatusDetail } from "../ui/components/agent-status.ts";
import { renderTodoInspection } from "../ui/components/todos.ts";
import { COMMANDS, parseCommandLine } from "./registry.ts";
import type {
  CatalogModelView,
  InteractionRef,
  SessionNodeView,
  SessionSettings,
  SessionSummaryView,
  SessionUserMessageBoundaryView,
  SessionView,
  ToolCallId,
  ToolExecutionId,
  UserInputBlock,
} from "../protocol/app-server.ts";

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
  /**
   * Show a different Session.
   *
   * This is client focus and nothing else. The App Server keeps every other
   * Session exactly as it was: loaded, attached, and running.
   */
  | {
      kind: "focus_session";
      sessionId: string;
      nodeId?: string;
      /** Fork/tree content selected before publication; never history. */
      editorContent?: UserInputBlock[];
      /** A bounded note about how this Session came to exist. */
      notice?: string;
    }
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
  /** One pending interaction card, addressed by its full routed identity. */
  | { type: "expand_interaction"; interaction: InteractionRef };

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
  /** The connection and its durable Session catalog. */
  host: AppServerHost;
  /** The Session currently in focus. */
  session: AppServerSession | undefined;
  /** `session/create` inputs for Sessions this client creates. */
  sessionSettings: SessionSettings;
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
  sessionId?: string;
  attachmentId?: string;
  conversationId?: string;
  runtimeIncarnation?: string;
  cursor?: string;
  /** How this client reaches the App Server, and who owns that process. */
  connection: string;
  ownership: string;
  connectionState: string;
  attachedSessions: number;
  childStatus: string;
  stderrTail: string;
  stderrTruncatedBytes: number;
  pendingRequests: number;
  resyncCount: number;
}

export class CommandDispatcher {
  #context: DispatcherContext;
  #inspected = new Map<string, import('../../../protocol/app-server/v20.ts').AvailableConfiguration>();

  constructor(context: DispatcherContext) {
    this.#context = context;
  }

  /**
   * Rebinds admission routing after the visible Session changed.
   *
   * This only affects invocations admitted after the rebind. Every public
   * operation captures the attachment at its start and passes that exact
   * attachment through all awaited phases, so an admitted command cannot
   * retarget a newer attachment.
   */
  setHost(host: AppServerHost): void {
    this.#context.host = host;
  }

  setSession(session: AppServerSession | undefined): void {
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
    const globalCommand = parseCommandLine(line);
    if (globalCommand?.name === "/settings") {
      try { return await this.#settings(globalCommand.argument); } catch (error) { return failure(error); }
    }
    const session = this.#context.session;
    if (session === undefined) return transient("info", "choose a Session before submitting commands");
    const command = parseCommandLine(line);
    if (command === undefined) {
      const text = line.trim();
      if (text.length === 0) {
        return { kind: "none" };
      }
      try {
        if (isAttemptActive(session.state)) await session.steer([{ type: "text", text }]);
        else await session.submitInbound([{ type: "text", text }]);
        return { kind: "none" };
      } catch (error) {
        return failure(error);
      }
    }
    return this.#dispatch(session, command.name, command.argument);
  }

  async #dispatch(
    session: AppServerSession,
    name: string,
    argument: string,
  ): Promise<CommandOutcome> {
    const state = session.state;

    try {
      switch (name) {
        case "/export": {
          const destination = archiveDestination(session.sessionId, argument);
          const path = await this.#context.host.exportSession(session.sessionId, destination);
          return { kind: "transient", level: "info", text: `Session archive saved to ${path}` };
        }
        case "/help":
          return inspect("Help", renderHelp());
        case "/model":
          return await this.#model(session, state, argument);
        case "/new":
          return await this.newSession();
        case "/resume":
          return await this.#resume(session, argument);
        case "/session":
          return argument ? await this.#configuration(session, argument) : await this.#sessionInfo(session);
        case "/name":
          return await this.#name(session, argument);
        case "/clone":
          return await this.#clone(session);
        case "/fork":
          return await this.#fork(session);
        case "/tree":
          return await this.#tree(session);
        case "/tools":
          return inspect("Tools", renderTools(state));
        case "/skills":
          return inspect("Skills", renderSkills(state));
        case "/todos":
          return inspect(
            "Todos",
            renderTodoInspection(selectTodos(state), isTodoComposed(state)),
          );
        case "/goal":
          return await this.#goal(session, argument);
        case "/status":
          return inspect("Agent Status", renderStatus(state));
        case "/compact":
          return await this.#compact(session, argument);
        case "/debug":
          return inspect("Client diagnostics", renderDebug(state, this.#context.diagnostics()));
        case "/show-reasoning":
          return reasoningPreference(argument);
        case "/expand":
          return expandPreference(argument);
        case "/cancel":
          return await this.#cancel(session, argument);
        case "/quit":
          return { kind: "quit" };
        default:
          return transient("error", `unknown command ${name}. Try /help.`);
      }
    } catch (error) {
      return failure(error);
    }
  }

  /**
   * Selection seams used by the native-data overlays.
   *
   * Every one of them ends in a focus change. None of them replaces a process,
   * detaches another Session, or asks the server to stop anything.
   */
  selectSession(sessionId: string): CommandOutcome {
    return { kind: "focus_session", sessionId };
  }

  selectTreeNode(sessionId: string, nodeId: string): CommandOutcome {
    return { kind: "focus_session", sessionId, nodeId };
  }

  /** Creates an independent Session from an exact historical boundary. */
  async forkAt(
    boundary: SessionUserMessageBoundaryView,
  ): Promise<CommandOutcome> {
    const session = this.#context.session;
    if (session === undefined) return transient("info", "choose a Session before submitting commands");
    try {
      const forked = await this.#context.host.forkSession(
        session.sessionId,
        boundary.surface_revision,
        boundary.message.id,
        session.nodeId,
      );
      return focusTransition(forked, "forked");
    } catch (error) {
      return failure(error);
    }
  }

  /** Creates a branch node inside the same Session graph. */
  async branchAt(
    boundary: SessionUserMessageBoundaryView,
  ): Promise<CommandOutcome> {
    const session = this.#context.session;
    if (session === undefined) return transient("info", "choose a Session before submitting commands");
    try {
      const current = await this.#context.host.readSession(session.sessionId);
      const branched = await this.#context.host.branchSession(
        session.sessionId,
        session.nodeId ?? current.active_node,
        boundary.surface_revision,
        boundary.message.id,
      );
      return focusTransition(branched, "branched");
    } catch (error) {
      return failure(error);
    }
  }

  /** Creates a fresh Session from this client's Session settings. */
  async newSession(): Promise<CommandOutcome> {
    try {
      const created = await this.#context.host.createSession(
        this.#context.sessionSettings,
      );
      return focusTransition(created, "created");
    } catch (error) {
      return failure(error);
    }
  }

  /**
   * Clones the exact committed current canonical head.
   *
   * The head revision is read authoritatively first, so the copy means one
   * exact conversation cut even if the Session commits again immediately
   * afterwards.
   */
  async #clone(session: AppServerSession): Promise<CommandOutcome> {
    const head = await session.boundaries(0, 1);
    const cloned = await this.#context.host.forkSession(
      session.sessionId,
      head.surfaceRevision,
      undefined,
      session.nodeId,
    );
    return focusTransition(cloned, "cloned");
  }

  async #resume(
    session: AppServerSession,
    argument: string,
  ): Promise<CommandOutcome> {
    if (argument.length > 0) return this.selectSession(argument);
    const page = await this.#context.host.listSessions();
    return {
      kind: "choose_session",
      sessions: page.sessions,
      nextOffset: page.nextOffset,
      query: "",
    };
  }

  async #sessionInfo(session: AppServerSession): Promise<CommandOutcome> {
    const refreshed = await this.#context.host.readSession(session.sessionId);
    return inspect(
      "Session",
      [
        // An unnamed Session has no name line rather than a placeholder one:
        // the identity below is what it is actually known by.
        ...(refreshed.name == null ? [] : [`name ${refreshed.name}`]),
        `session ${refreshed.id}`,
        `active node ${refreshed.active_node}`,
        `conversation ${refreshed.active_conversation_id}`,
        `nodes ${refreshed.node_count}`,
      ].join("\n"),
    );
  }

  async #compact(
    session: AppServerSession,
    argument: string,
  ): Promise<CommandOutcome> {
    if (argument.length > 0) {
      return transient("error", "usage: /compact");
    }
    const context = await session.compactContext();
    const latest = context.latest_compaction;
    if (latest == null) {
      return transient("info", "context compacted");
    }
    return transient(
      "info",
      `context compacted to generation ${latest.generation}: ${latest.tokens_before.input_tokens} → ${latest.estimated_tokens_after} tokens`,
    );
  }

  async #goal(session: AppServerSession, argument: string): Promise<CommandOutcome> {
    const [action = "show", ...words] = argument.trim().split(/\s+/);
    const text = words.join(" ");
    if (action === "create") {
      if (!text) return transient("error", "usage: /goal create <objective>");
      // A typed native Goal control. Creating a Goal never sends a model
      // request, and nothing follows it: an Active Goal continues on its own
      // whenever the runtime reaches an eligible idle boundary.
      return inspect("Goal", goalSummary(await session.goal({ action: "create", objective: text, budget: 10 })));
    }
    const view = await session.goal({ action: "show" });
    if (!action || action === "show") return inspect("Goal", goalSummary(view));
    if (!view.current) return transient("error", "No current Goal. Use /goal create <objective>.");
    let mutation: import("../protocol/app-server.ts").GoalMutation;
    if ((action === "pause" || action === "resume") && !text) mutation = { action };
    else if (action === "edit" && text) mutation = { action: "edit", objective: text };
    else if (action === "budget" && /^\d+$/.test(text)) mutation = { action: "budget", rounds: Number(text) };
    else return transient("error", "usage: /goal [show | create <objective> | pause | resume | edit <objective> | budget <rounds>]");
    const updated = await session.goal({ action: "mutate", expected: view.current.reference, mutation });
    return inspect("Goal", goalSummary(updated));
  }

  async #settings(argument: string): Promise<CommandOutcome> {
    const words = argument.match(/"(?:[^"\\]|\\.)*"|\S+/g)?.map(word => word.startsWith('"') ? JSON.parse(word) as string : word) ?? [];
    const owner = words.shift() ?? "user";
    let target: import("../../../protocol/app-server/v20.ts").SourceTarget;
    if (owner === "user") target = { kind: "user" };
    else if (owner === "workspace" && words[0]) target = { kind: "workspace", directory: words.shift()! };
    else return transient("error", 'usage: /settings [user | workspace "<canonical absolute path>"] [rescan | approval policy|full_access|inherit]');
    const action = words.shift();
    const client = this.#context.host.client;
    let source = (await client.call("configuration/sourcesRead", { target }, "source_settings")).projection;
    if (action === "rescan") await client.call("configuration/reconcile", { target }, "configuration_application");
    else if (action === "approval") {
      const mode = words.shift();
      if (mode !== "policy" && mode !== "full_access" && mode !== "inherit") return transient("error", "approval expects policy, full_access, or inherit");
      await client.call("configuration/sourceWrite", { target, expected_revision: source[target.kind]!.revision,
        mutation: { kind: "config", mutation: { unit: "approval", authored: mode === "inherit" ? null : mode } } }, "source_settings");
    } else if (action) return transient("error", "Unknown Settings action");
    if (action) source = (await client.call("configuration/sourcesRead", { target }, "source_settings")).projection;
    return inspect(target.kind === "user" ? "User Settings" : "Workspace Settings", JSON.stringify(source, null, 2));
  }

  async #configuration(session: AppServerSession, argument: string): Promise<CommandOutcome> {
    if (argument === "adopt") {
      const candidate = this.#inspected.get(session.sessionId);
      if (!candidate) return transient("error", "Inspect /session settings before adopting.");
      try { await session.adoptConfiguration(candidate); }
      catch (error) {
        const authority = await session.readConfiguration();
        if (authority?.candidate) this.#inspected.set(session.sessionId, authority.candidate);
        else this.#inspected.delete(session.sessionId);
        return transient("error", String(error) + "\nNot replayed. Native authority: " + JSON.stringify(authority));
      }
    } else if (argument !== "settings") return transient("error", "usage: /session [settings | adopt]");
    const application = await session.readConfiguration();
    if (application?.candidate) this.#inspected.set(session.sessionId, application.candidate);
    else this.#inspected.delete(session.sessionId);
    return inspect("Session settings", [
      renderSettings(session.state, await session.configuration()),
      JSON.stringify(application, null, 2),
      application?.candidate ? "Inspect the exact candidate above; /session adopt explicitly adopts it." : "No pending candidate reported by native authority.",
    ].join("\n"));
  }

  /**
   * `/name` shows the active Session's name, and `/name <text>` sets it.
   *
   * Reporting the current name is the useful answer to a bare `/name`: a
   * Session is unnamed until someone names it, so "this one has no name" is
   * the fact the user is asking about, not a syntax mistake to correct.
   */
  async #name(
    session: AppServerSession,
    argument: string,
  ): Promise<CommandOutcome> {
    const host = this.#context.host;
    if (argument.trim().length === 0) {
      const current = await host.readSession(session.sessionId);
      return transient(
        "info",
        current.name == null
          ? `session ${current.id} is unnamed; use /name <text> to name it`
          : `session name: ${current.name}`,
      );
    }
    const named = await host.renameSession(session.sessionId, argument);
    return transient("info", `session named ${sessionLabel(named)}`);
  }

  async #fork(session: AppServerSession): Promise<CommandOutcome> {
    const page = await session.boundaries();
    return {
      kind: "choose_fork",
      boundaries: page.boundaries,
      nextOffset: page.nextOffset,
    };
  }

  async #tree(session: AppServerSession): Promise<CommandOutcome> {
    // The graph is a durable catalog read and the boundaries are a live Surface
    // read: two owners, two requests, and no third place that could disagree.
    const host = this.#context.host;
    const [current, tree, page] = await Promise.all([
      host.readSession(session.sessionId),
      host.sessionTree(session.sessionId),
      session.boundaries(),
    ]);
    return {
      kind: "choose_tree",
      session: current,
      nodes: tree.nodes,
      nextNodeOffset: tree.nextOffset,
      boundaries: page.boundaries,
      nextHistoryOffset: page.nextOffset,
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
    session: AppServerSession,
    state: PresentationState,
    argument: string,
  ): Promise<CommandOutcome> {
    // `show` is answered from the projection alone; every other spelling
    // needs the runtime's authoritative catalog.
    if (/^profile(?:\s|$)/.test(argument)) {
      const parts = argument.split(/\s+/);
      const clear = parts.length === 2 && parts[1] === "clear";
      const profile = parts.length === 3 && parts[1] === "set" ? parts[2] : undefined;
      if (!clear && !profile) return transient("error", "usage: /model profile set <id> | /model profile clear");
      const current = await session.modelGet();
      const configured = { ...current.configured };
      if (clear) delete configured.reasoningProfile;
      else if (profile !== undefined) configured.reasoningProfile = profile;
      const updated = await session.modelSet(configured);
      return transient("info", `Session reasoning profile -> ${updated.configured.reasoningProfile ?? "model default"}; next eligible admission. Defaults unchanged.`);
    }
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
    if (session === undefined) return transient("info", "choose a Session before submitting commands");
    return this.#selectModel(session, model);
  }

  async #selectModel(
    session: AppServerSession,
    model: CatalogModelView,
  ): Promise<CommandOutcome> {
    try {
      const current = session.state?.sessionModel?.configured;
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
          ? `current attempt remains ${attempt.model?.primary.model ?? "unavailable"}`
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
    session: AppServerSession,
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


}

/**
 * `/show-reasoning [on|off]` — a display preference, applied by the UI.
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
      return transient("error", "usage: /show-reasoning [on|off]");
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
 * /expand interaction <conversation-id>::<interaction-id>  toggle one pending card
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
    const interaction = parseInteractionRef(rest.join(" "));
    if (interaction === undefined) {
      return usage("/expand interaction <conversation-id>::<interaction-id>");
    }
    return {
      kind: "preference",
      preference: { type: "expand_interaction", interaction },
    };
  }
  return { kind: "preference", preference: { type: "expand_call", callId: argument } };
}

function parseInteractionRef(value: string): InteractionRef | undefined {
  const separator = value.indexOf("::");
  if (separator <= 0 || separator === value.length - 2) {
    return undefined;
  }
  return {
    conversation_id: value.slice(0, separator),
    interaction_id: value.slice(separator + 2),
  };
}

function usage(spelling: string): CommandOutcome {
  return transient("error", `usage: ${spelling}`);
}

/** The product Goal surface, derived from durable `GoalPhase` alone.
 *
 * `Active` means rustX is authorized to continue pursuing the objective
 * whenever the runtime reaches an eligible idle boundary — there is no
 * separate arm/play/start step to report, and no inactive-but-active state
 * to render. The exact revision is kept for the CAS token users pass back. */
export function goalSummary(view: import("../protocol/app-server.ts").GoalView): string {
  const goal = view.current;
  if (!goal) return "No current Goal.";
  const status = goal.phase[0]!.toUpperCase() + goal.phase.slice(1);
  const lines = [
    `Goal: ${goal.objective}`,
    `Status: ${status}`,
    `Progress: ${goal.autonomous_rounds_consumed}/${goal.autonomous_round_budget} autonomous rounds`,
    `Revision: ${goal.reference.id} r${goal.reference.revision}`,
  ];
  if (goal.phase === "blocked" && goal.blocked_reason) lines.splice(2, 0, `Blocked: ${goal.blocked_reason}`);
  return lines.join("\n");
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

/** Turns one committed durable transition into a focus change. */
function focusTransition(
  transition: import("../app-server/host.ts").SessionTransition,
  verb: string,
): CommandOutcome {
  const label = sessionLabel(transition.session);
  return {
    kind: "focus_session",
    sessionId: transition.session.id,
    nodeId: transition.session.active_node,
    editorContent: transition.editorContent,
    notice:
      transition.durabilityDiagnostic === undefined
        ? `${verb} session ${label}`
        : `${verb} session ${label}, but its durability became uncertain: ${compactDiagnostic(transition.durabilityDiagnostic)}`,
  };
}

function failure(error: unknown): CommandOutcome {
  if (error instanceof UncertainOutcomeError) {
    // The response was lost, which is not evidence that the server refused
    // the request. Saying "failed" here would be a claim this client cannot
    // support, and resending would risk doing the work twice.
    return transient(
      "error",
      `${compactDiagnostic(error.message)} · reconnect and check the authoritative state before retrying`,
    );
  }
  if (error instanceof AppServerRequestError) {
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
    "Pending approvals and questionnaires are answered in the human-input surface, which opens automatically while an interaction is pending (Ctrl+G reopens it after a dismissal). Plain text is always submitted as an inbound message, never as an answer.",
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
  if (session === null) {
    return "Historical evidence is partial. Active Session model, approval, resources, and launch sources are unavailable. Consult retained Request Snapshots for request-specific evidence.";
  }
  const lines = [
    `### ${state.settingsEvidence === "frozen_child" ? "Parent-provided child" : "Session"} model (next eligible admission)`,
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
      `### Active attempt model (frozen at admission)`,
      `- attempt: \`${attempt.attemptId}\` (${attempt.phase.type})`,
      `- model: \`${attempt.model?.primary.model ?? "unavailable"}\``,
      `- reasoning: ${(attempt.model ? describeReasoning(attempt.model.primary) : "unavailable")}`,
    );
    if (attempt.model && attempt.model.primary.model !== session.effective.model) {
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
  const lines = [
    `### Tools (capability revision ${state.capabilities.revision})`,
    "Active tools are the exact model authority. Available but inactive tools cannot be invoked by this model.",
    "Definitions are inert. Agent/Workflow selection creates finite admitted demand before source preparation.",
  ];
  appendToolGroups(lines, "Active tools", activeGroups);
  appendToolGroups(lines, "Available but inactive", inactiveGroups);
  return lines.join("\n");
}

function appendToolGroups(
  lines: string[],
  heading: string,
  groups: Array<{ origin: string; tools: import("../protocol/app-server.ts").RuntimeClientTool[] }>,
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
 * `/status` — the latest runtime-composed Agent Status, as Agent Status.
 *
 * This surface answers one question: *what context is the agent currently
 * carrying?* It renders the runtime's typed sections through the same facets
 * the transcript annotation uses, so the compact `◇ status · …` line beside a
 * turn and the expanded view here can never describe one composition
 * differently.
 *
 * It never parses `AgentStatusView.rendered`. That string is the model-facing
 * body, kept for diagnostics; recovering section semantics from it would make
 * this client a second interpreter of a composition it already receives
 * structurally.
 *
 * Generic runtime and client diagnostics — cursors, attempts, capability
 * revisions, mailbox counters — belong to `/debug`, which is the one
 * diagnostic surface. The provenance line below is the only runtime metadata
 * kept here, and it is metadata *about this composition*, separated from it.
 */
export function renderStatus(state: PresentationState): string {
  const status = latestAgentStatus(state);
  if (status === undefined) {
    return [
      "### Agent Status",
      "",
      "No Agent Status has been composed yet. The runtime composes one when a",
      "delivery opportunity makes it eligible; `/debug` shows runtime and",
      "client diagnostics meanwhile.",
    ].join("\n");
  }
  return [
    "### Agent Status",
    ...renderAgentStatusDetail(status),
    "",
    `Composed by attempt \`${status.attempt_id}\` on turn ${status.turn}. Runtime and client diagnostics are in \`/debug\`.`,
  ].join("\n");
}

/** `/debug` — bounded presentation and protocol diagnostics. */
export function renderDebug(
  state: PresentationState,
  diagnostics: DebugDiagnostics,
): string {
  const lines = [
    "### Client diagnostics",
    `- App Server: ${diagnostics.connection}`,
    `- process ownership: ${diagnostics.ownership}`,
    `- session: \`${diagnostics.sessionId ?? "none"}\``,
    `- attachment: \`${diagnostics.attachmentId ?? "none"}\``,
    `- conversation: \`${diagnostics.conversationId ?? "none"}\``,
    `- runtime incarnation: \`${diagnostics.runtimeIncarnation ?? "none"}\``,
    `- cursor: ${diagnostics.cursor ?? state.cursor}`,
    `- attached sessions: ${diagnostics.attachedSessions}`,
    `- connection: ${diagnostics.connectionState}`,
    `- child: ${diagnostics.childStatus}`,
    `- pending requests: ${diagnostics.pendingRequests}`,
    `- authoritative repairs (resync): ${diagnostics.resyncCount}`,
    "",
    "### Runtime projection",
    `- desired session model: \`${state.sessionModel?.configured.model ?? "unavailable"}\``,
    `- active attempt model: \`${state.attempt?.model?.primary.model ?? "none"}\``,
    `- capability revision: ${state.capabilities.revision}`,
    ...contextDiagnosticsLines(state),
    `- inbound pending: ${(state.inbound.pending ?? []).length}`,
    ...lastDrainLines(state),
    `- background executions: ${state.background.length} (${activeBackground(state).length} active)`,
    `- transcript entries: ${state.transcript.length}`,
    `- composed Agent Statuses: ${state.statuses.length}`,
    ...attemptDiagnosticsLines(state),
    ...(state.runtimeShutdown
      ? ["- runtime is draining; conversation-owned work is settling"]
      : []),
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

/** The current/latest attempt, as the runtime published it. */
function attemptDiagnosticsLines(state: PresentationState): string[] {
  const attempt = state.attempt;
  if (attempt === undefined) {
    return ["- attempt: none"];
  }
  const phase =
    attempt.phase.type === "settled"
      ? outcomeLabel(attempt.phase.outcome)
      : attempt.phase.type;
  return [`- attempt: \`${attempt.attemptId}\` ${phase} (turn ${attempt.turn})`];
}

function lastDrainLines(state: PresentationState): string[] {
  const drain = state.inbound.last_drain;
  return drain == null
    ? []
    : [`- last drain: watermark ${drain.watermark}, ${drain.count} item(s)`];
}

function contextDiagnosticsLines(state: PresentationState): string[] {
  const context = state.context;
  const latest = context.latest_compaction;
  if (latest == null) {
    return [`- context compactions: ${context.compaction_count}`];
  }
  return [
    `- context compactions: ${context.compaction_count} (latest generation ${latest.generation}, surface revision ${latest.surface_revision})`,
    `- latest context measurement: ${latest.tokens_before.input_tokens} tokens before (${latest.tokens_before.source}), ${latest.estimated_tokens_after} estimated after`,
  ];
}

/** Render native publication facts without parsing or resolving configuration. */
export function renderSettings(state: PresentationState, configuration?: import("../protocol/app-server.ts").EffectiveConfiguration): string {
  return [
    `Published generation: ${configuration?.generation ?? state.resources.revision}`,
    renderModel(state),
    `Approval policy: ${state.effectiveApprovalMode}`,
    `Plugins: ${JSON.stringify(state.effectivePlugins)}`,
    ...(configuration ? [
      `Root default model: ${configuration.root_agent.model?.model ?? "none"}`,
      `Session explicit model: ${configuration.session_model?.model ?? "none"}`,
      `Admitted Attempt: ${JSON.stringify(configuration.admitted_attempt ?? null)}`,
      "Effective configuration and provenance:", JSON.stringify(configuration, null, 2),
    ] : []),
    "/settings edits User/Workspace sources. /session settings inspects Session state; /session adopt explicitly adopts the inspected candidate.",
  ].join("\n");
}
