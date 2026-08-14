/**
 * View models derived from presentation state.
 *
 * Components read these instead of subscribing to protocol events directly.
 * That keeps the rendering layer replaceable: a future non-Pi client consumes
 * the same selectors, and swapping the terminal library touches nothing
 * below this file.
 *
 * Selectors are pure derivations. They label and group; they never decide a
 * semantic fact. In particular a tool's origin or name may pick an icon and
 * nothing else — execution semantics stay Rust-owned.
 */

import type {
  BackgroundLifecycle,
  CatalogModelView,
  ModelInvocationView,
  RuntimeClientBackgroundExecution,
  RuntimeClientOutcome,
  RuntimeClientSkill,
  RuntimeClientTool,
  ToolOrigin,
} from "../protocol/types.ts";
import { BACKGROUND_TERMINAL_STATES } from "../protocol/types.ts";
import type { PresentationState } from "./state.ts";

/** A short human label for a tool origin. Cosmetic only. */
export function originLabel(origin: ToolOrigin): string {
  if (origin === "builtin") {
    return "native";
  }
  if ("mcp" in origin) {
    return `mcp:${origin.mcp.server_id}`;
  }
  return "python";
}

/** A short human label for an attempt settlement. */
export function outcomeLabel(outcome: RuntimeClientOutcome): string {
  switch (outcome.type) {
    case "completed":
      return outcome.finish_reason.type === "other"
        ? `completed (${outcome.finish_reason.reason})`
        : `completed (${outcome.finish_reason.type})`;
    case "cancelled":
      return `cancelled (${outcome.reason})`;
    case "timed_out":
      return "timed out";
    case "limit_exceeded":
      return `limit exceeded (${outcome.limit})`;
    case "failed":
      return outcome.error.type === "model"
        ? `failed (${outcome.error.kind}): ${outcome.error.message}`
        : `failed: ${outcome.error.error.type}`;
    default:
      return "settled";
  }
}

/** The one-line working indicator, or undefined when idle. */
export function workingLabel(state: PresentationState): string | undefined {
  const attempt = state.attempt;
  if (attempt === undefined) {
    return undefined;
  }
  if (attempt.phase.type === "admitted") {
    return "admitted";
  }
  if (attempt.phase.type !== "running") {
    return undefined;
  }
  const running = attempt.foreground.filter(
    (execution) => execution.state.type === "running",
  );
  if (running.length > 0) {
    const names = running.map((execution) => execution.name || execution.tool_id);
    return `running ${names.join(", ")}`;
  }
  return `turn ${attempt.turn}`;
}

/** Background executions the runtime still considers active. */
export function activeBackground(
  state: PresentationState,
): RuntimeClientBackgroundExecution[] {
  return state.background.filter(
    (execution) => !BACKGROUND_TERMINAL_STATES.has(execution.state),
  );
}

/** Whether a background lifecycle state is terminal, per the runtime. */
export function isBackgroundTerminal(state: BackgroundLifecycle): boolean {
  return BACKGROUND_TERMINAL_STATES.has(state);
}

/** The tool catalog, grouped by origin for display. */
export function toolsByOrigin(
  state: PresentationState,
): Array<{ origin: string; tools: RuntimeClientTool[] }> {
  const groups = new Map<string, RuntimeClientTool[]>();
  for (const tool of state.capabilities.tools ?? []) {
    const label = originLabel(tool.origin);
    const bucket = groups.get(label);
    if (bucket === undefined) {
      groups.set(label, [tool]);
    } else {
      bucket.push(tool);
    }
  }
  return [...groups.entries()]
    .map(([origin, tools]) => ({ origin, tools }))
    .sort((left, right) => left.origin.localeCompare(right.origin));
}

/** The Skill catalog as the runtime published it. */
export function skills(state: PresentationState): RuntimeClientSkill[] {
  return state.capabilities.skills ?? [];
}

/** A compact capability summary line for one resolved invocation. */
export function capabilitySummary(invocation: ModelInvocationView): string {
  const parts = [
    `in: ${invocation.capabilities.inputModalities.join("/") || "none"}`,
    `out: ${invocation.capabilities.outputModalities.join("/") || "none"}`,
    `tools: ${invocation.capabilities.toolCalls ? "yes" : "no"}`,
    `reasoning: ${describeReasoning(invocation)}`,
  ];
  return parts.join("  ");
}

/**
 * How reasoning stands for one invocation.
 *
 * Reports exactly what rustX published. A reasoning-capable model with no
 * selectable profile is *not* rendered as a profile list: there is no
 * universal off/low/medium/high, and inventing one would make this client a
 * second model-configuration authority.
 */
export function describeReasoning(invocation: ModelInvocationView): string {
  if (!invocation.capabilities.reasoning) {
    return "unsupported";
  }
  if (invocation.reasoningProfile !== undefined) {
    return `${invocation.reasoningEnabled ? "on" : "off"} (profile ${invocation.reasoningProfile})`;
  }
  return invocation.reasoningEnabled
    ? "on (runtime default, no selectable profile)"
    : "off (runtime default, no selectable profile)";
}

/**
 * Modalities the catalog claims but the runtime cannot deliver today.
 *
 * Displaying only effective capability is the rule; this exists so a user can
 * be told *why* something is absent without the client ever advertising an
 * unusable capability as available.
 */
export function unavailableInputModalities(
  invocation: ModelInvocationView,
): string[] {
  const effective = new Set(invocation.capabilities.inputModalities);
  return invocation.declaredCapabilities.inputModalities.filter(
    (modality) => !effective.has(modality),
  );
}

/** Catalog entries as select-list rows: reference plus effective capability. */
export function catalogRows(
  models: CatalogModelView[],
): Array<{ value: string; label: string; description: string }> {
  return models.map((model) => ({
    value: model.model,
    label: model.model,
    description: [
      model.protocol,
      `${model.contextWindow} ctx`,
      `${model.maxOutputTokens} out`,
      model.effectiveCapabilities.toolCalls ? "tools" : "no tools",
      model.effectiveCapabilities.reasoning ? "reasoning" : "no reasoning",
      (model.reasoningProfiles ?? []).length > 0
        ? `profiles: ${(model.reasoningProfiles ?? []).map((profile) => profile.id).join(",")}`
        : "no profiles",
    ].join(" · "),
  }));
}
