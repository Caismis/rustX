import { activeBackground, capabilitySummary, describeConfiguredProfile, describeProfile, describeReasoning, inactiveToolsByOrigin, latestAgentStatus, originLabel, outcomeLabel, skills, toolsByOrigin, unavailableInputModalities } from "../presentation/selectors.ts";
import type { PresentationState } from "../presentation/state.ts";
import { renderAgentStatusDetail } from "../ui/components/agent-status.ts";
import { COMMANDS } from "./registry.ts";


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

/** The product Goal surface, derived from durable `GoalPhase` alone.
 *
 * `Active` means rustX is authorized to continue pursuing the objective
 * whenever the runtime reaches an eligible idle boundary — there is no
 * separate arm/play/start step to report, and no inactive-but-active state
 * to render. The exact revision is kept for the CAS token users pass back. */
export function goalSummary(view: import("../protocol/app-server.ts").RuntimeClientGoalView): string {
  const goal = view.current;
  if (!goal) return "No current Goal.";
  const status = goal.phase[0]!.toUpperCase() + goal.phase.slice(1);
  const lines = [
    `Goal: ${goal.objective_deferred?"Objective deferred; inspect its native Goal fields with /observation goal":goal.objective}`,
    `Status: ${status}`,
    `Progress: ${goal.autonomous_rounds_consumed}/${goal.autonomous_round_budget} autonomous rounds`,
    `Revision: ${goal.reference.id} r${goal.reference.revision}`,
  ];
  if (goal.phase === "blocked" && goal.blocked_reason_deferred) lines.splice(2,0,'Blocked reason deferred; read the selected native Goal field.');
  else if (goal.phase === "blocked" && goal.blocked_reason) lines.splice(2, 0, `Blocked: ${goal.blocked_reason}`);
  return lines.join("\n");
}

/* Renderers — Markdown text built purely from projection state. */

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
 * Three model identities, two profile facts and the reasoning state, each
 * named for what it is:
 *
 * ```text
 * configured            SessionModelView.configured.model
 * effective             SessionModelView.effective.model
 * attempt               AttemptModelView.primary.model
 * configured profile    SessionModelConfig.profile
 * effective profile     ModelInvocationView.profile
 * effective reasoning   ModelInvocationView.reasoningEnabled
 * ```
 *
 * They are always all printed, even when they coincide, because `/model show`
 * is the place a user goes to find out whether they do.
 */
export function renderModel(state: PresentationState): string {
  const session = state.sessionModel;
  if (session === null && state.modelStatus?.body_deferred) {
    return `Session model: ${state.modelStatus.available ? "available" : "unavailable"}; configuration content is deferred. Read /observation model at the current checkpoint. Model and Profile catalog choices remain available through /model-position.`;
  }
  if (session === null) {
    return "Historical evidence is partial. Active Session model, approval, resources, and launch sources are unavailable. Consult retained Request Snapshots for request-specific evidence.";
  }
  // A selection the published catalog no longer admits has no effective
  // model for new work; the last resolved one is not offered as one.
  const effective = session.unavailable == null ? session.effective : undefined;
  const lines = [
    `### ${state.settingsEvidence === "frozen_child" ? "Parent-provided child" : "Session"} model (next independent admission)`,
    `- configured: \`${session.configured.model}\``,
  ];
  if (effective == null) {
    lines.push(
      `- effective: unavailable — ${session.unavailable ?? "the published catalog does not admit the configured selection"}`,
      "- nothing new is admitted until a Model or Profile the catalog publishes is selected",
      `- configured profile: ${describeConfiguredProfile(session.configured)}`,
    );
  } else {
    lines.push(
      `- effective: \`${effective.model}\` via ${effective.protocol}`,
      `- context window: ${effective.contextWindow}`,
      `- max output tokens: ${effective.maxOutputTokens} (model maximum ${effective.modelMaxOutputTokens})`,
      // Configured and effective profiles are separate facts: the session asks,
      // the runtime resolves, and a catalog default is neither of them.
      `- configured profile: ${describeConfiguredProfile(session.configured)}`,
      `- effective profile: ${describeProfile(effective)}`,
      `- reasoning: ${describeReasoning(effective)}`,
      `- capabilities: ${capabilitySummary(effective)}`,
    );

    const unavailable = unavailableInputModalities(effective);
    if (unavailable.length > 0) {
      // Only effective capability is advertised; the declaration explains why
      // something the catalog claims is not offered.
      lines.push(
        `- declared but not usable today: input ${unavailable.join(", ")}`,
      );
    }

    const params = effective.requestParams ?? {};
    if (Object.keys(params).length > 0) {
      // Opaque provider-owned configuration: displayed, never interpreted.
      lines.push(
        "- request parameters (provider-owned, opaque):",
        "```json",
        JSON.stringify(params, null, 2),
        "```",
      );
    }
  }

  const summary = session.summary ?? session.configured.summaryModel;
  lines.push(
    `- summary model: ${
      summary == null || summary.mode === "session"
        ? "follows the attempt's primary model"
        : `\`${summary.model}\``
    }`,
  );

  const title = session.title ?? session.configured.titleModel;
  lines.push(`- title model: ${title == null || title.mode === "session"
    ? "follows the Session's primary model at title admission"
    : `\`${title.model}\``}`);

  const attempt = state.attempt;
  if (attempt !== undefined) {
    lines.push(
      "",
      `### Active attempt model (frozen at admission)`,
      `- attempt: \`${attempt.attemptId}\` (${attempt.phase.type})`,
      `- model: \`${attempt.model?.primary.model ?? "unavailable"}\``,
      `- profile: ${(attempt.model ? describeProfile(attempt.model.primary) : "unavailable")}`,
      `- reasoning: ${(attempt.model ? describeReasoning(attempt.model.primary) : "unavailable")}`,
      `- max output tokens: ${attempt.model?.primary.maxOutputTokens ?? "unavailable"}`,
      `- effective capabilities: ${attempt.model ? capabilitySummary(attempt.model.primary) : "unavailable"}`,
      `- read_image active: ${attempt.executionSettings?.read_image_active ?? "unavailable"}`,
    );
    if (attempt.model) {
      lines.push("- frozen request parameters (provider-owned, opaque):", "```json", JSON.stringify(attempt.model.primary.requestParams ?? {}, null, 2), "```",
        "- frozen summary model:", "```json", JSON.stringify(attempt.model.summary, null, 2), "```");
    }
    if (attempt.model && effective != null) {
      lines.push(
        `- Session selection applies to future independent attempts; this attempt keeps its frozen model, profile, parameters and summary policy.`,
      );
    }
    if (effective != null && session.configured.model !== effective.model) {
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
    "Active tools below describe the selected configuration for the next eligible admission. An existing Attempt keeps its frozen catalog; `/model show` reports its admitted image capability. Available but inactive tools cannot be invoked.",
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
    `- Jobs: ${state.jobs.length} (${activeBackground(state).length} active)`,
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
    `- context compactions: ${context.compaction_count} (latest generation ${latest.generation}, projection activation ${latest.activation_id})`,
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
    "/settings edits User/Workspace sources; /settings repairs shows their automatic repairs. /session settings inspects Session state; /session adopt explicitly adopts the inspected candidate.",
  ].join("\n");
}
