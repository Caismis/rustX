import { AppServerRequestError, UncertainOutcomeError } from "../app-server/client.ts";
import { sessionLabel } from "../presentation/selectors.ts";
import type { CatalogModelView, InteractionRef, SessionNodeView, SessionSummaryView, SessionUserMessageBoundaryObservation, SessionView, ToolCallId, ToolExecutionId, UserInputBlock } from "../protocol/app-server.ts";
import { compactDiagnostic } from "../diagnostic-text.ts";
import type { SessionTransition } from "../app-server/host.ts";


/** Typed presentation intents and the pure parsers and constructors that produce them. */

/** What the dispatcher wants the UI to do next. */
export type CommandOutcome =
  | { kind: "none" }
  | { kind: "inspect"; title: string; body: string }
  | { kind: "inspect_agent"; agentId: string }
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
      boundaries: SessionUserMessageBoundaryObservation[];
      nextOffset?: number;
    }
  | {
      kind: "choose_tree";
      session: SessionView;
      nodes: SessionNodeView[];
      nextNodeOffset?: number;
      boundaries: SessionUserMessageBoundaryObservation[];
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
      /** Explicit selection may retry a known failed native initialization. */
      retryInitialization?: true;
      nodeId?: string;
      /** Fork/tree content selected before publication; never history. */
      editorContent?: UserInputBlock[];
      /** A bounded note about how this Session came to exist. */
      notice?: string;
    }
  | {
      /** The first bounded page of committed `present` deliveries. */
      kind: "choose_delivery";
      page: import("../presentation/deliveries.ts").DeliveryPage;
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
 * content. Provider request parameters are configured separately through
 * the Model Profile and model selection.
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

/**
 * `/show-reasoning [on|off]` — a display preference, applied by the UI.
 *
 * It changes what is drawn and nothing else. The model's request
 * configuration lives in `SessionModelConfig.profile` and is only changeable
 * through `model_set`.
 */
export function reasoningPreference(argument: string): CommandOutcome {
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
 * /context-model [byte-offset]     inspect the original native context model
 * /expand                          toggle the latest tool call
 * /expand latest                   the same
 * /expand all                      expand every tool, background, and
 *                                  interaction card
 * /expand none                     collapse all three domains
 * /expand <tool-call-id>           toggle one foreground card
 * /expand job <exec-id>     toggle one background card
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
export function expandPreference(argument: string): CommandOutcome {
  if (argument === "" || argument === "latest") {
    return { kind: "preference", preference: { type: "expand", target: "latest" } };
  }
  if (argument === "all" || argument === "none") {
    return { kind: "preference", preference: { type: "expand", target: argument } };
  }
  const [head, ...rest] = argument.split(/\s+/);
  if (head === "job") {
    const executionId = rest.join(" ");
    if (executionId.length === 0) {
      return usage("/expand job <job-id>");
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

export function parseInteractionRef(value: string): InteractionRef | undefined {
  const separator = value.indexOf("::");
  if (separator <= 0 || separator === value.length - 2) {
    return undefined;
  }
  return {
    conversation_id: value.slice(0, separator),
    interaction_id: value.slice(separator + 2),
  };
}

export function usage(spelling: string): CommandOutcome {
  return transient("error", `usage: ${spelling}`);
}

export function inspect(title: string, body: string): CommandOutcome {
  return { kind: "inspect", title, body };
}

export function transient(
  level: "info" | "error",
  text: string,
): CommandOutcome {
  return { kind: "transient", level, text };
}

/** Turns one committed durable transition into a focus change. */
export function focusTransition(
  transition: SessionTransition,
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

export function failure(error: unknown): CommandOutcome {
  if (error instanceof UncertainOutcomeError) {
    // The response was lost, which is not evidence that the server refused
    // the request. Saying "failed" here would be a claim this client cannot
    // support, and resending would risk doing the work twice.
    return transient(
      "error",
      `${compactDiagnostic(error.message)} · not replayed; reconnect and check the authoritative state. Readback does not establish this request's outcome`,
    );
  }
  if (error instanceof AppServerRequestError) {
    return transient("error", compactDiagnostic(error.message));
  }
  return transient("error", compactDiagnostic(error));
}
