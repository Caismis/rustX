/**
 * The unified human-input surface (Issue #185).
 *
 * One overlay presents every live pending routed interaction — approvals and
 * questionnaires, primary and subagent — with exactly one focused panel, a
 * deterministic queue, and kind-typed responses that always name the exact
 * `InteractionRef` they were collected for. These tests drive the component
 * directly with raw key sequences; the app-level routing (Esc precedence,
 * Ctrl+G, resync) is covered in `app.test.ts`.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { HumanInteractionOverlay } from "../src/ui/components/hitl.ts";
import type {
  ApprovalDecision,
  InteractionRef,
  QuestionnaireResponse,
  RoutedInteraction,
} from "../src/protocol/types.ts";
import {
  compareInteractionRefs,
  reconcileInteractionFocus,
} from "../src/presentation/interaction-focus.ts";
import {
  defaultPreferences,
  withExpandedInteractions,
  type PresentationPreferences,
} from "../src/ui/preferences.ts";
import { plainText } from "../src/ui/theme.ts";
import {
  approvalInteraction,
  childApprovalInteraction,
  childQuestionnaireInteraction,
  questionnaireInteraction,
} from "./support/fixtures.ts";

const ENTER = "\r";
const ESC = "\u001b";
const DOWN = "\u001b[B";
const CTRL_DOWN = "\u001b[1;5B";
const CTRL_UP = "\u001b[1;5A";
const CTRL_E = "\u0005";
const CTRL_C = "\u0003";

interface Recorded {
  decisions: Array<{ interaction: InteractionRef; decision: ApprovalDecision }>;
  submissions: Array<{ interaction: InteractionRef; response: QuestionnaireResponse }>;
  declines: InteractionRef[];
  dismissals: InteractionRef[];
  navigations: InteractionRef[];
  expansions: InteractionRef[];
  interrupts: number;
}

function surface(initial?: {
  interactions?: RoutedInteraction[];
  focused?: InteractionRef;
  preferences?: PresentationPreferences;
}): { overlay: HumanInteractionOverlay; recorded: Recorded } {
  const recorded: Recorded = {
    decisions: [],
    submissions: [],
    declines: [],
    dismissals: [],
    navigations: [],
    expansions: [],
    interrupts: 0,
  };
  const overlay = new HumanInteractionOverlay({
    onDecision: (interaction, decision) =>
      recorded.decisions.push({ interaction, decision }),
    onQuestionnaireSubmit: (interaction, response) =>
      recorded.submissions.push({ interaction, response }),
    onQuestionnaireDecline: (interaction) => recorded.declines.push(interaction),
    onDismiss: (interaction) => recorded.dismissals.push(interaction),
    onInterrupt: () => {
      recorded.interrupts += 1;
    },
    onNavigate: (interaction) => recorded.navigations.push(interaction),
    onToggleExpand: (interaction) => recorded.expansions.push(interaction),
  });
  const interactions = (initial?.interactions ?? [approvalInteraction()])
    .slice()
    .sort((left, right) => compareInteractionRefs(left.interaction, right.interaction));
  const focused =
    initial?.focused ?? reconcileInteractionFocus(interactions, undefined)!;
  overlay.update(
    interactions,
    focused,
    initial?.preferences ?? defaultPreferences(),
  );
  return { overlay, recorded };
}

/** Re-points the surface, as the app does on every authoritative render. */
function sync(
  overlay: HumanInteractionOverlay,
  interactions: RoutedInteraction[],
  currentFocus: InteractionRef | undefined,
  preferences = defaultPreferences(),
): InteractionRef | undefined {
  const sorted = interactions
    .slice()
    .sort((left, right) => compareInteractionRefs(left.interaction, right.interaction));
  const focus = reconcileInteractionFocus(sorted, currentFocus);
  if (focus !== undefined) {
    overlay.update(sorted, focus, preferences);
  }
  return focus;
}

function rendered(overlay: HumanInteractionOverlay, width = 100): string {
  overlay.setBodyHeight(40);
  return plainText(overlay.render(width).join("\n"));
}

describe("human-input surface", () => {
  it("represents every pending interaction independently in one queue", () => {
    const interactions = [
      approvalInteraction("attempt-1-interaction-approval-a"),
      questionnaireInteraction("attempt-1-interaction-question-b"),
      childApprovalInteraction("child-a-interaction-1", "implement"),
      childQuestionnaireInteraction("child-b-interaction-1", "reviewer"),
    ];
    const { overlay } = surface({ interactions });
    const text = rendered(overlay);
    assert.equal(overlay.popupTitle(), "Human input · 4 pending");
    for (const marker of [
      "[main]",
      "[implement]",
      "[reviewer]",
      "Approval · bash",
      "Question · Environment",
    ]) {
      assert.ok(text.includes(marker), `queue must show ${marker}`);
    }
  });

  it("shows the approval's prepared invocation with its source and identity", () => {
    const approval = childApprovalInteraction();
    const { overlay } = surface({ interactions: [approval] });
    const text = rendered(overlay);
    assert.ok(text.includes("Approval from reviewer"), "child source context");
    assert.ok(text.includes("conv-child-1"), "child conversation context");
    assert.ok(text.includes("bash"));
    assert.ok(text.includes("foreground · native · call call-1"));
    assert.ok(text.includes("native policy requires approval"));
    assert.ok(text.includes("printf original"), "bounded arguments");
    assert.ok(text.includes("conv-child-1::attempt-1-interaction-approval-1"));
  });

  it("opens with Deny preselected so a generic Enter can never allow", () => {
    const approval = approvalInteraction();
    const { overlay, recorded } = surface({ interactions: [approval] });
    overlay.handleInput(ENTER);
    assert.deepEqual(recorded.decisions, [
      {
        interaction: approval.interaction,
        decision: { type: "deny", reason: "denied by the user" },
      },
    ]);
  });

  it("allows exactly once only after explicit navigation to Allow once", () => {
    const approval = approvalInteraction();
    const { overlay, recorded } = surface({ interactions: [approval] });
    overlay.handleInput(DOWN);
    assert.equal(recorded.decisions.length, 0, "navigation settles nothing");
    overlay.handleInput(ENTER);
    assert.deepEqual(recorded.decisions, [
      { interaction: approval.interaction, decision: { type: "allow" } },
    ]);
  });

  it("emits exactly one response even when Enter repeats while submitting", () => {
    const approval = approvalInteraction();
    const { overlay, recorded } = surface({ interactions: [approval] });
    overlay.handleInput(ENTER);
    overlay.handleInput(ENTER);
    overlay.handleInput(DOWN);
    overlay.handleInput(ENTER);
    assert.equal(recorded.decisions.length, 1);
  });

  it("resets an armed Allow once when focus moves to another interaction", () => {
    const first = approvalInteraction("attempt-1-interaction-approval-a");
    const second = childApprovalInteraction("child-a-interaction-1");
    const interactions = [first, second];
    const { overlay, recorded } = surface({ interactions });
    // Focus starts on the child approval (conv-child-1 sorts first). Arm it.
    overlay.handleInput(DOWN);
    // Navigate to the primary approval: the app moves the focus, and the
    // surface must not carry the armed selection over.
    overlay.handleInput(CTRL_DOWN);
    assert.deepEqual(recorded.navigations, [
      { conversation_id: "conv-test", interaction_id: "attempt-1-interaction-approval-a" },
    ]);
    overlay.update(interactions, recorded.navigations[0]!, defaultPreferences());
    overlay.handleInput(ENTER);
    assert.deepEqual(recorded.decisions, [
      {
        interaction: first.interaction,
        decision: { type: "deny", reason: "denied by the user" },
      },
    ]);
  });

  it("dismisses an approval on Esc without answering anything", () => {
    const approval = approvalInteraction();
    const { overlay, recorded } = surface({ interactions: [approval] });
    overlay.escape();
    assert.deepEqual(recorded.dismissals, [approval.interaction]);
    assert.equal(recorded.decisions.length, 0);
  });

  it("declines a questionnaire on Esc through the typed decline path", () => {
    const questionnaire = questionnaireInteraction();
    const { overlay, recorded } = surface({ interactions: [questionnaire] });
    overlay.escape();
    assert.deepEqual(recorded.declines, [questionnaire.interaction]);
    assert.equal(recorded.dismissals.length, 0);
  });

  it("labels a child questionnaire with its routed source", () => {
    const questionnaire = childQuestionnaireInteraction();
    const { overlay } = surface({ interactions: [questionnaire] });
    const text = rendered(overlay);
    assert.ok(text.includes("Question from reviewer"));
  });

  it("submits the existing typed questionnaire response for the exact ref", () => {
    const questionnaire = questionnaireInteraction();
    const { overlay, recorded } = surface({ interactions: [questionnaire] });
    overlay.handleInput(ENTER); // choose "staging" on the question tab
    overlay.handleInput("\t"); // review tab
    overlay.handleInput(ENTER); // submit
    assert.deepEqual(recorded.submissions, [
      {
        interaction: questionnaire.interaction,
        response: {
          type: "submitted",
          value: {
            answers: [
              {
                question_index: 0,
                answer: { type: "single_option", value: { label: "staging" } },
              },
            ],
          },
        },
      },
    ]);
  });

  it("moves focus through the queue without emitting any response", () => {
    const interactions = [
      approvalInteraction("attempt-1-interaction-approval-a"),
      questionnaireInteraction("attempt-1-interaction-question-b"),
      childApprovalInteraction("child-a-interaction-1"),
    ];
    const { overlay, recorded } = surface({ interactions });
    for (const key of [CTRL_DOWN, CTRL_DOWN, CTRL_UP]) {
      overlay.handleInput(key);
      // The app applies the navigation by re-pointing the surface.
      const moved = recorded.navigations[recorded.navigations.length - 1];
      if (moved !== undefined) {
        overlay.update(interactions, moved, defaultPreferences());
      }
    }
    assert.deepEqual(
      recorded.navigations.map((interaction) => interaction.interaction_id),
      [
        "attempt-1-interaction-approval-a",
        "attempt-1-interaction-question-b",
        "attempt-1-interaction-approval-a",
      ],
    );
    assert.equal(recorded.decisions.length, 0);
    assert.equal(recorded.submissions.length, 0);
    assert.equal(recorded.declines.length, 0);
  });

  it("toggles disclosure only, in the shared interaction expansion domain", () => {
    const big = approvalInteraction();
    if (big.request.kind.type !== "approval") throw new Error("fixture");
    const args = Object.fromEntries(
      Array.from({ length: 30 }, (_, index) => [`key-${index}`, index]),
    );
    const approval: RoutedInteraction = {
      ...big,
      request: { ...big.request, kind: { ...big.request.kind, arguments: args } },
    };
    const { overlay, recorded } = surface({ interactions: [approval] });
    const collapsed = rendered(overlay);
    assert.ok(collapsed.includes("more"), "collapsed detail is bounded");
    overlay.handleInput(CTRL_E);
    assert.deepEqual(recorded.expansions, [approval.interaction]);
    assert.equal(recorded.decisions.length, 0, "expanding settles nothing");
    sync(
      overlay,
      [approval],
      approval.interaction,
      withExpandedInteractions(defaultPreferences(), [approval.interaction]),
    );
    const expanded = rendered(overlay);
    assert.ok(
      expanded.includes("key-28"),
      "expanded detail goes far beyond the collapsed budget",
    );
    assert.ok(!collapsed.includes("key-8"), "collapsed detail stops early");
  });

  it("advances the panel deterministically when the focused interaction settles", () => {
    const interactions = [
      approvalInteraction("attempt-1-interaction-approval-a"),
      childApprovalInteraction("child-a-interaction-1"),
    ];
    const { overlay, recorded } = surface({ interactions });
    const focused = overlay.focusedInteraction!.interaction;
    assert.equal(focused.conversation_id, "conv-child-1");
    // The runtime settles the focused child approval; the surface follows.
    const remaining = interactions.filter(
      (entry) => entry.interaction !== focused,
    );
    const next = sync(overlay, remaining, focused);
    assert.deepEqual(next, {
      conversation_id: "conv-test",
      interaction_id: "attempt-1-interaction-approval-a",
    });
    assert.equal(recorded.decisions.length, 0, "settlement needs no local answer");
    assert.ok(rendered(overlay).includes("[main]"));
  });

  it("keeps unrelated panels intact when one interaction is removed", () => {
    const questionnaire = questionnaireInteraction("attempt-1-interaction-question-b");
    const interactions = [childApprovalInteraction("child-a-interaction-1"), questionnaire];
    const { overlay, recorded } = surface({ interactions });
    // Draft an answer in the questionnaire panel, then remove the child
    // approval (e.g. its child died): the draft panel must survive.
    const focus = sync(overlay, interactions, undefined);
    assert.equal(focus?.interaction_id, "child-a-interaction-1");
    overlay.handleInput(CTRL_DOWN);
    sync(overlay, interactions, questionnaire.interaction);
    overlay.handleInput(ENTER); // choose "staging"
    const remaining = [questionnaire];
    sync(overlay, remaining, questionnaire.interaction);
    overlay.handleInput("\t");
    overlay.handleInput(ENTER);
    const text = rendered(overlay);
    assert.ok(!text.includes("child-a-interaction-1"), "dead child is gone");
    assert.deepEqual(recorded.submissions, [
      {
        interaction: questionnaire.interaction,
        response: {
          type: "submitted",
          value: {
            answers: [
              {
                question_index: 0,
                answer: { type: "single_option", value: { label: "staging" } },
              },
            ],
          },
        },
      },
    ]);
  });

  it("routes Ctrl+C to interruption, never to an answer", () => {
    const { overlay, recorded } = surface({
      interactions: [approvalInteraction()],
    });
    overlay.handleInput(CTRL_C);
    assert.equal(recorded.interrupts, 1);
    assert.equal(recorded.decisions.length, 0);
  });

  it("re-enables the exact failed panel by routed identity", () => {
    const approval = approvalInteraction();
    const questionnaire = childQuestionnaireInteraction("child-b-interaction-1");
    const { overlay, recorded } = surface({
      interactions: [approval, questionnaire],
    });
    // Focus is the child questionnaire; its rejection must not touch the
    // approval's submitting state, and vice versa.
    overlay.escape();
    assert.equal(recorded.declines.length, 1);
    overlay.submissionFailed(questionnaire.interaction);
    // The questionnaire panel is live again: a second Esc declines again
    // (the runtime would reject a duplicate; the surface stays responsive).
    overlay.escape();
    assert.equal(recorded.declines.length, 2);
  });
});
