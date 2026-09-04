/**
 * The one human-input surface for every live runtime interaction.
 *
 * Issue #185 unifies what used to be two asymmetric surfaces — a focused
 * questionnaire overlay and command-driven approvals — into a single modal
 * presentation over the Runtime Client's authoritative pending projection:
 *
 * ```text
 * PresentationState.pendingInteractions   (runtime-owned, routed)
 *        |
 *        v
 * HumanInteractionOverlay                 (this component)
 *   queue: every pending interaction, presentation order
 *   focus: exactly one, moved by Ctrl+Up/Down, settling nothing
 *   panel: typed per kind
 *     approval       Deny / Allow once, Deny preselected
 *     questionnaire  the existing bounded questionnaire surface
 *        |
 *        v
 * typed InteractionResponse, routed by the exact InteractionRef
 * ```
 *
 * Ownership stays where Issue #184 put it: the runtime creates, validates,
 * settles, and cancels interactions; this component only renders them,
 * collects explicit human input, and hands one typed response per explicit
 * action to the app, which sends it through `interaction_respond`. Nothing
 * here creates, recreates, or prematurely settles an interaction:
 *
 * - navigating the queue only changes which interaction is shown;
 * - Esc on an approval dismisses the *surface* (presentation-only), it never
 *   answers — the interaction stays pending in the activity section;
 * - Esc on a questionnaire is the existing explicit typed decline;
 * - an approval's fail-safe default selection is Deny, so a generic Enter on
 *   a freshly opened surface can never grant execution authority;
 * - the displayed arguments are the runtime's prepared invocation, shown for
 *   the decision; nothing the user types here becomes tool input.
 *
 * Per-kind panels are keyed by the full routed identity, so several pending
 * interactions — any mix of approvals and questionnaires, from the primary
 * conversation and from supervised subagents — coexist without overwriting
 * one another, and a questionnaire draft survives focus moves within the
 * surface. The surface as a whole is disposable: an authoritative resync
 * closes it and the next render rebuilds it from the projection, so no stale
 * overlay can submit against an interaction the runtime no longer owns.
 */

import { Key, matchesKey, truncateToWidth } from "@earendil-works/pi-tui";

import type {
  ApprovalDecision,
  InteractionRef,
  QuestionnaireResponse,
  RoutedInteraction,
} from "../../protocol/types.ts";
import {
  compareInteractionRefs,
  interactionRefLabel,
  moveInteractionFocus,
  sameInteractionRef,
} from "../../presentation/interaction-focus.ts";
import {
  interactionSourceName,
  originLabel,
} from "../../presentation/selectors.ts";
import {
  HEADER_BUDGET,
  interactionKey,
  isInteractionExpanded,
  type PresentationPreferences,
} from "../preferences.ts";
import { role, style } from "../theme.ts";
import { windowAroundSelected, type PopupContent } from "./popup-frame.ts";
import { QuestionnaireOverlay } from "./questionnaire.ts";
import {
  clipText,
  formatJson,
  preview,
  toLines,
  type ToolRenderContext,
} from "./tool-renderers.ts";

/** How many queue rows the surface shows at once. */
const QUEUE_BUDGET = 4;

/** The deny reason sent when the user picks Deny without typing prose. */
const DENY_REASON = "denied by the user";

const APPROVAL_FOOTER =
  "↑↓ choose · Enter confirm (Deny preselected) · Ctrl+E expand · Ctrl+↑↓ other pending · Esc dismiss · Ctrl+C cancel attempt";

export interface HumanInteractionOverlayOptions {
  /** The one typed response path for approvals: allow once, or deny. */
  onDecision: (interaction: InteractionRef, decision: ApprovalDecision) => void;
  /** The existing typed questionnaire submission, for the exact ref. */
  onQuestionnaireSubmit: (
    interaction: InteractionRef,
    response: QuestionnaireResponse,
  ) => void;
  /** The existing typed questionnaire decline, for the exact ref. */
  onQuestionnaireDecline: (interaction: InteractionRef) => void;
  /**
   * Presentation-only dismissal of the surface (approval Esc). Settles
   * nothing; the interaction remains pending and visible in the activity
   * section, and Ctrl+G reopens the surface.
   */
  onDismiss: (interaction: InteractionRef) => void;
  /** Cancellation intent for the owning attempt (Ctrl+C), never an answer. */
  onInterrupt: () => void;
  /** Presentation-only focus move to another pending interaction. */
  onNavigate: (interaction: InteractionRef) => void;
  /** Presentation-only disclosure toggle for one interaction's detail. */
  onToggleExpand: (interaction: InteractionRef) => void;
  onChange?: () => void;
}

export class HumanInteractionOverlay implements PopupContent {
  readonly #options: HumanInteractionOverlayOptions;
  #interactions: RoutedInteraction[] = [];
  #focused: InteractionRef | undefined;
  #focusedKey = "";
  #preferences: PresentationPreferences | undefined;
  #approvalChoice: 0 | 1 = 0;
  #approvalSubmitting = false;
  /**
   * One questionnaire panel per pending interaction, keyed by the routed
   * identity. Drafts (selections, custom answers) survive focus moves inside
   * the surface; a panel is dropped when its interaction leaves the
   * projection, and the whole map is dropped with the surface on resync.
   */
  readonly #questionnaires = new Map<string, QuestionnaireOverlay>();
  #bodyHeight = 24;

  constructor(options: HumanInteractionOverlayOptions) {
    this.#options = options;
  }

  /**
   * Re-points the surface at the current authoritative projection.
   *
   * Called on every render with the sorted pending list and the reconciled
   * focus. Changing the focused identity resets approval input to its
   * fail-safe default — a surface that was showing interaction A can never
   * carry an armed "Allow once" selection over to interaction B.
   */
  update(
    interactions: RoutedInteraction[],
    focused: InteractionRef,
    preferences: PresentationPreferences,
  ): void {
    const previousKey = this.#focusedKey;
    // Presentation order is the lexicographic routed identity pair, defined
    // here rather than inherited from list position: it orders the display
    // and nothing else.
    this.#interactions = interactions
      .slice()
      .sort((left, right) =>
        compareInteractionRefs(left.interaction, right.interaction),
      );
    this.#focused = focused;
    this.#focusedKey = interactionKey(focused);
    this.#preferences = preferences;
    if (this.#focusedKey !== previousKey) {
      this.#approvalChoice = 0;
      this.#approvalSubmitting = false;
    }
    const live = new Set(
      interactions.map((entry) => interactionKey(entry.interaction)),
    );
    for (const key of [...this.#questionnaires.keys()]) {
      if (!live.has(key)) {
        this.#questionnaires.delete(key);
      }
    }
  }

  /** The interaction the surface currently shows, from the live projection. */
  get focusedInteraction(): RoutedInteraction | undefined {
    const focused = this.#focused;
    if (focused === undefined) {
      return undefined;
    }
    return this.#interactions.find((entry) =>
      sameInteractionRef(entry.interaction, focused),
    );
  }

  /**
   * Esc, routed per kind — the single definition of Escape behavior:
   *
   * - approval: dismiss the surface without answering (presentation-only);
   * - questionnaire: the existing explicit typed decline.
   *
   * The app-level key listener calls this; it never answers an interaction
   * itself.
   */
  escape(): void {
    const focused = this.focusedInteraction;
    if (focused === undefined) {
      return;
    }
    if (focused.request.kind.type === "approval") {
      if (!this.#approvalSubmitting) {
        this.#options.onDismiss(focused.interaction);
      }
      return;
    }
    this.#questionnairePanel(focused)?.decline();
  }

  /**
   * Re-enables input after the Runtime Client rejected a response.
   *
   * Routed by the exact identity the app tried to settle: a rejection for an
   * unfocused questionnaire re-enables its panel without disturbing the
   * focused one.
   */
  submissionFailed(interaction: InteractionRef): void {
    const panel = this.#questionnaires.get(interactionKey(interaction));
    if (panel !== undefined) {
      panel.submissionFailed();
      return;
    }
    if (
      this.#focused !== undefined &&
      sameInteractionRef(this.#focused, interaction)
    ) {
      this.#approvalSubmitting = false;
      this.#changed();
    }
  }

  popupTitle(): string {
    return `Human input · ${this.#interactions.length} pending`;
  }

  invalidate(): void {
    // Surface state (focus panel, questionnaire drafts) is intentionally
    // retained across redraws. Authoritative replacement closes the surface
    // and the next sync constructs a new one, discarding only drafts.
  }

  popupFooter(): string[] {
    const focused = this.focusedInteraction;
    if (focused === undefined) {
      return [];
    }
    if (focused.request.kind.type === "approval") {
      return [APPROVAL_FOOTER];
    }
    const panel = this.#questionnairePanel(focused);
    return [...(panel?.popupFooter() ?? []), "Ctrl+↑↓ other pending"];
  }

  setBodyHeight(height: number): void {
    this.#bodyHeight = Math.max(1, Math.floor(height));
  }

  handleInput(data: string): void {
    const focused = this.focusedInteraction;
    if (focused === undefined) {
      return;
    }
    if (matchesKey(data, Key.ctrl("c"))) {
      this.#options.onInterrupt();
      return;
    }
    if (matchesKey(data, Key.ctrl("up"))) {
      this.#navigate(-1);
      return;
    }
    if (matchesKey(data, Key.ctrl("down"))) {
      this.#navigate(1);
      return;
    }
    const kind = focused.request.kind;
    if (kind.type === "approval") {
      this.#approvalInput(focused.interaction, data);
      return;
    }
    // The questionnaire panel owns its established input contract: tabs,
    // rows, multi-select toggles, bounded custom answers, review, submit.
    this.#questionnairePanel(focused)?.handleInput(data);
  }

  render(width: number): string[] {
    const safeWidth = Math.max(1, Math.floor(width));
    const focused = this.focusedInteraction;
    if (focused === undefined || this.#focused === undefined) {
      return [fitLine(role.meta("no pending interaction"), safeWidth)];
    }

    const focusedIndex = this.#interactions.findIndex((entry) =>
      sameInteractionRef(entry.interaction, this.#focused!),
    );
    const queueBudget = Math.min(QUEUE_BUDGET, this.#interactions.length);
    const window = windowAroundSelected(
      this.#interactions.length,
      Math.max(0, focusedIndex),
      queueBudget,
      () => 1,
    );
    const queue: string[] = [];
    for (let index = window.start; index < window.end; index += 1) {
      queue.push(fitLine(this.#queueRow(this.#interactions[index]!, index === focusedIndex), safeWidth));
    }

    const detailHeight = Math.max(1, this.#bodyHeight - queue.length - 1);
    const detail =
      focused.request.kind.type === "approval"
        ? this.#renderApproval(focused, safeWidth, detailHeight)
        : this.#renderQuestionnaire(focused, safeWidth, detailHeight);

    return [...queue, "", ...detail]
      .slice(0, this.#bodyHeight)
      .map((line) => fitLine(line, safeWidth));
  }

  /** One queue row: focus marker, source, kind, and a bounded summary. */
  #queueRow(routed: RoutedInteraction, focused: boolean): string {
    const source = clipText(
      interactionSourceName(routed.source),
      HEADER_BUDGET.maxChars,
    );
    const kind = routed.request.kind;
    const summary =
      kind.type === "approval"
        ? `Approval · ${kind.tool_name}`
        : `Question · ${kind.questionnaire.questions[0]?.header ?? "questionnaire"}`;
    const marker = focused ? role.accent("›") : " ";
    return `${marker} ${role.accent(`[${source}]`)} ${clipText(summary, HEADER_BUDGET.maxChars)}`;
  }

  #approvalInput(interaction: InteractionRef, data: string): void {
    if (this.#approvalSubmitting) {
      return;
    }
    if (matchesKey(data, Key.escape)) {
      this.#options.onDismiss(interaction);
      return;
    }
    if (
      matchesKey(data, Key.up) ||
      matchesKey(data, Key.down) ||
      matchesKey(data, Key.left) ||
      matchesKey(data, Key.right)
    ) {
      this.#approvalChoice = this.#approvalChoice === 0 ? 1 : 0;
      this.#changed();
      return;
    }
    if (matchesKey(data, Key.ctrl("e"))) {
      this.#options.onToggleExpand(interaction);
      return;
    }
    if (matchesKey(data, Key.enter)) {
      // Deny is index 0 and is where every approval starts: an affirmative
      // grant requires explicit navigation to "Allow once" first.
      const decision: ApprovalDecision =
        this.#approvalChoice === 1
          ? { type: "allow" }
          : { type: "deny", reason: DENY_REASON };
      this.#approvalSubmitting = true;
      this.#options.onDecision(interaction, decision);
      this.#changed();
    }
    // Every other key is swallowed: nothing the user types here is editor
    // input, and none of it can become an approval response.
  }

  #navigate(delta: -1 | 1): void {
    if (this.#focused === undefined) {
      return;
    }
    const next = moveInteractionFocus(this.#interactions, this.#focused, delta);
    if (next !== undefined && !sameInteractionRef(next, this.#focused)) {
      this.#options.onNavigate(next);
    }
  }

  /**
   * The focused approval: the runtime's prepared invocation, shown for the
   * decision. The choices stay pinned below the bounded reason/argument
   * bands so even an expanded detail can never push Deny/Allow off the
   * surface.
   */
  #renderApproval(
    routed: RoutedInteraction,
    width: number,
    height: number,
  ): string[] {
    const kind = routed.request.kind;
    if (kind.type !== "approval") {
      return [];
    }
    const preferences = this.#preferences;
    const context: ToolRenderContext = {
      expanded:
        preferences !== undefined &&
        isInteractionExpanded(preferences, routed.interaction),
      budget: preferences?.previewBudget ?? { maxLines: 8, maxChars: 1_000 },
    };
    const head = [
      role.meta(
        routed.source.type === "primary"
          ? `Approval from ${interactionSourceName(routed.source)}`
          : `Approval from ${interactionSourceName(routed.source)} · subagent ${clipText(routed.source.child_conversation_id, HEADER_BUDGET.maxChars)}`,
      ),
      `${role.toolTitle(style.bold(clipText(kind.tool_name, HEADER_BUDGET.maxChars)))} ${role.meta(interactionRefLabel(routed.interaction))}`,
      role.meta(
        `${kind.mode} · ${originLabel(kind.origin)} · call ${clipText(kind.call_id, HEADER_BUDGET.maxChars)}`,
      ),
    ];
    const foot = this.#approvalSubmitting
      ? ["", role.pending("Submitting response…")]
      : [
          "",
          `${this.#approvalChoice === 0 ? role.accent("›") : " "} ${role.strong("Deny")}`,
          `${this.#approvalChoice === 1 ? role.accent("›") : " "} ${role.strong("Allow once")}`,
        ];
    const middle = [
      ...preview(toLines(kind.reason), context, "reason line"),
      ...preview(formatJson(kind.arguments), context, "argument line").map(
        (line) => role.meta(line),
      ),
    ];
    const room = Math.max(0, height - head.length - foot.length);
    const visible = middle.slice(0, room);
    if (middle.length > room && room > 0) {
      visible[room - 1] = role.meta("…");
    }
    return [...head, ...visible, ...foot];
  }

  #renderQuestionnaire(
    routed: RoutedInteraction,
    width: number,
    height: number,
  ): string[] {
    const panel = this.#questionnairePanel(routed);
    if (panel === undefined) {
      return [];
    }
    panel.setBodyHeight(height);
    return panel.render(width);
  }

  /**
   * Returns the panel that owns one pending questionnaire, creating it on
   * first focus. Panel callbacks close over the exact routed identity, so a
   * response always names the interaction it was collected for.
   */
  #questionnairePanel(routed: RoutedInteraction): QuestionnaireOverlay | undefined {
    if (routed.request.kind.type !== "questionnaire") {
      return undefined;
    }
    const key = interactionKey(routed.interaction);
    const existing = this.#questionnaires.get(key);
    if (existing !== undefined) {
      return existing;
    }
    const ref = routed.interaction;
    const panel = new QuestionnaireOverlay({
      interactionId: interactionRefLabel(ref),
      questionnaire: routed.request.kind.questionnaire,
      sourceLabel:
        routed.source.type === "primary"
          ? undefined
          : `Question from ${interactionSourceName(routed.source)}`,
      onSubmit: (response) => this.#options.onQuestionnaireSubmit(ref, response),
      onDecline: () => this.#options.onQuestionnaireDecline(ref),
      onInterrupt: () => this.#options.onInterrupt(),
      onChange: () => this.#changed(),
    });
    this.#questionnaires.set(key, panel);
    return panel;
  }

  #changed(): void {
    this.#options.onChange?.();
  }
}

function fitLine(value: string, width: number): string {
  return truncateToWidth(value, Math.max(1, width), "…");
}
