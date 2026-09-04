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

import { visibleWidth } from "@earendil-works/pi-tui";

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
const PAGE_UP = "\u001b[5~";
const PAGE_DOWN = "\u001b[6~";

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

/**
 * One rendered page as a continuous character sequence. Wrap boundaries are
 * presentation, not content: a marker may legitimately straddle two wrapped
 * rows of the same page, so reachability is checked against the flattened
 * page, never against a single row.
 */
function flattened(text: string): string {
  return text.replaceAll("\n", "");
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

  it("never re-arms an in-flight approval when focus leaves and returns", () => {
    const first = approvalInteraction("attempt-1-interaction-approval-a");
    const second = approvalInteraction("attempt-1-interaction-approval-b");
    const interactions = [first, second];
    const { overlay, recorded } = surface({ interactions });
    // Focus starts on A (the smallest identity). Submit A; its response stays
    // in flight until the authoritative projection removes A.
    overlay.handleInput(ENTER);
    assert.equal(recorded.decisions.length, 1);
    // Navigate A -> B -> A while A's response is unresolved.
    overlay.handleInput(CTRL_DOWN);
    overlay.update(interactions, recorded.navigations.at(-1)!, defaultPreferences());
    assert.equal(
      overlay.focusedInteraction!.interaction.interaction_id,
      "attempt-1-interaction-approval-b",
    );
    overlay.handleInput(CTRL_UP);
    overlay.update(interactions, recorded.navigations.at(-1)!, defaultPreferences());
    assert.equal(
      overlay.focusedInteraction!.interaction.interaction_id,
      "attempt-1-interaction-approval-a",
    );
    // The in-flight guard belongs to A's exact routed identity, not to the
    // focused panel: a second Enter emits no second semantic response.
    overlay.handleInput(ENTER);
    assert.equal(recorded.decisions.length, 1, "exactly one response for A");
    assert.ok(
      rendered(overlay).includes("Submitting response"),
      "A stays visibly in flight after the focus round trip",
    );
  });

  it("a failed submission re-arms exactly that interaction and no other", () => {
    const first = approvalInteraction("attempt-1-interaction-approval-a");
    const second = approvalInteraction("attempt-1-interaction-approval-b");
    const interactions = [first, second];
    const { overlay, recorded } = surface({ interactions });
    overlay.handleInput(ENTER); // A in flight
    overlay.handleInput(CTRL_DOWN);
    overlay.update(interactions, recorded.navigations.at(-1)!, defaultPreferences());
    overlay.handleInput(ENTER); // B in flight
    assert.equal(recorded.decisions.length, 2);
    // The Runtime Client rejected A's response: A — and only A — re-arms.
    overlay.submissionFailed(first.interaction);
    // B is focused and still in flight: Enter on B emits nothing.
    overlay.handleInput(ENTER);
    assert.equal(recorded.decisions.length, 2, "B stays in flight");
    // Returning to A, the intentional retry goes through — once, and then the
    // guard is armed again.
    overlay.handleInput(CTRL_UP);
    overlay.update(interactions, recorded.navigations.at(-1)!, defaultPreferences());
    overlay.handleInput(ENTER);
    assert.equal(recorded.decisions.length, 3);
    assert.equal(
      recorded.decisions[2]!.interaction.interaction_id,
      "attempt-1-interaction-approval-a",
    );
    overlay.handleInput(ENTER);
    assert.equal(recorded.decisions.length, 3, "the retried A is guarded again");
  });

  it("authoritative removal of a submitted approval leaves the survivor intact", () => {
    const first = approvalInteraction("attempt-1-interaction-approval-a");
    const second = approvalInteraction("attempt-1-interaction-approval-b");
    const { overlay, recorded } = surface({ interactions: [first, second] });
    overlay.handleInput(ENTER); // A in flight
    // The runtime settles A: the projection drops it, focus advances to B,
    // and A's now-obsolete in-flight guard goes with it.
    const focus = sync(overlay, [second], first.interaction);
    assert.equal(focus?.interaction_id, "attempt-1-interaction-approval-b");
    // B never carried any in-flight state and remains submittable.
    assert.ok(
      !rendered(overlay).includes("Submitting response"),
      "B is not in flight",
    );
    overlay.handleInput(ENTER);
    assert.deepEqual(
      recorded.decisions.map((entry) => entry.interaction.interaction_id),
      ["attempt-1-interaction-approval-a", "attempt-1-interaction-approval-b"],
    );
  });

  it("pages the expanded approval detail deterministically to its final line", () => {
    const base = approvalInteraction();
    if (base.request.kind.type !== "approval") throw new Error("fixture");
    const args = Object.fromEntries(
      Array.from({ length: 100 }, (_, index) => [`key-${index}`, `value-${index}`]),
    );
    const approval: RoutedInteraction = {
      ...base,
      request: { ...base.request, kind: { ...base.request.kind, arguments: args } },
    };
    const { overlay, recorded } = surface({
      interactions: [approval],
      preferences: withExpandedInteractions(defaultPreferences(), [
        approval.interaction,
      ]),
    });
    // The expanded detail is the complete invocation, but the viewport stays
    // bounded: the tail starts past the visible window.
    const initial = rendered(overlay);
    assert.ok(initial.includes("key-0"), "the first page shows the head");
    assert.ok(!initial.includes("key-99"), "the tail starts past the viewport");
    assert.ok(
      initial.includes("PgUp/PgDn scroll"),
      "the position line names the deterministic navigation",
    );
    assert.equal(recorded.decisions.length, 0, "rendering settles nothing");
    // Each PageDown exposes content that lay past the viewport, in order,
    // until the final formatted line is on screen.
    overlay.handleInput(PAGE_DOWN);
    const secondPage = rendered(overlay);
    assert.ok(!secondPage.includes("key-0"), "the window moved past the head");
    assert.ok(!secondPage.includes("key-99"), "still not the tail");
    overlay.handleInput(PAGE_DOWN);
    overlay.handleInput(PAGE_DOWN);
    const tail = rendered(overlay);
    assert.ok(tail.includes("key-99"), "the final argument line is reachable");
    assert.ok(tail.includes("}"), "the closing line of the invocation shows");
    assert.ok(tail.includes("of 103"), "the position line reports the full length");
    // PageUp walks back to the head.
    overlay.handleInput(PAGE_UP);
    overlay.handleInput(PAGE_UP);
    overlay.handleInput(PAGE_UP);
    assert.ok(rendered(overlay).includes("key-0"), "the head is reachable again");
    // Scrolling settled nothing, and the pinned choices still work.
    assert.equal(recorded.decisions.length, 0, "paging settles nothing");
    overlay.handleInput(DOWN);
    overlay.handleInput(ENTER);
    assert.deepEqual(recorded.decisions, [
      { interaction: approval.interaction, decision: { type: "allow" } },
    ]);
  });

  it("keeps a collapsed approval bounded and unpaged, choices pinned", () => {
    const base = approvalInteraction();
    if (base.request.kind.type !== "approval") throw new Error("fixture");
    const args = Object.fromEntries(
      Array.from({ length: 100 }, (_, index) => [`key-${index}`, `value-${index}`]),
    );
    const approval: RoutedInteraction = {
      ...base,
      request: { ...base.request, kind: { ...base.request.kind, arguments: args } },
    };
    const { overlay, recorded } = surface({ interactions: [approval] });
    const collapsed = rendered(overlay);
    assert.ok(collapsed.includes("more"), "collapsed detail stays bounded");
    assert.ok(!collapsed.includes("key-99"));
    assert.ok(collapsed.includes("Deny") && collapsed.includes("Allow once"));
    // Paging is inert while collapsed: nothing scrolls, nothing is emitted.
    overlay.handleInput(PAGE_DOWN);
    assert.equal(rendered(overlay), collapsed, "collapsed detail does not page");
    assert.equal(recorded.decisions.length, 0);
    overlay.handleInput(ENTER);
    assert.deepEqual(recorded.decisions, [
      {
        interaction: approval.interaction,
        decision: { type: "deny", reason: "denied by the user" },
      },
    ]);
  });

  it("makes every character of a very long single-line argument reachable", () => {
    const base = approvalInteraction();
    if (base.request.kind.type !== "approval") throw new Error("fixture");
    const tail = "-END-OF-AUTHORITATIVE-PAYLOAD";
    const approval: RoutedInteraction = {
      ...base,
      request: {
        ...base.request,
        kind: {
          ...base.request.kind,
          arguments: { payload: `BEGIN-${"x".repeat(2_000)}${tail}` },
        },
      },
    };
    const width = 40;
    const { overlay, recorded } = surface({ interactions: [approval] });
    // Collapsed stays bounded and never exposes the tail.
    const collapsed = rendered(overlay, width);
    assert.ok(collapsed.includes("more"), "collapsed detail is bounded");
    assert.ok(!collapsed.includes(tail), "collapsed detail hides the tail");
    // Expanding is disclosure only.
    overlay.handleInput(CTRL_E);
    assert.deepEqual(recorded.expansions, [approval.interaction]);
    assert.equal(recorded.decisions.length, 0, "expanding settles nothing");
    sync(
      overlay,
      [approval],
      approval.interaction,
      withExpandedInteractions(defaultPreferences(), [approval.interaction]),
    );
    // The beginning shows immediately; the tail starts past the viewport.
    const initial = rendered(overlay, width);
    assert.ok(initial.includes("BEGIN-"), "the head shows first");
    assert.ok(!flattened(initial).includes(tail), "the tail starts past the viewport");
    // No rendered row overflows the terminal width.
    for (const line of overlay.render(width)) {
      assert.ok(plainText(line).length <= width, "no horizontal overflow");
    }
    // Paging the wrapped rows eventually reaches the final character.
    let paged = initial;
    for (let page = 0; page < 8 && !flattened(paged).includes(tail); page += 1) {
      overlay.handleInput(PAGE_DOWN);
      paged = rendered(overlay, width);
    }
    assert.ok(flattened(paged).includes(tail), "the authoritative tail is reachable");
    assert.ok(paged.includes("Deny") && paged.includes("Allow once"), "choices stay pinned");
    assert.equal(recorded.decisions.length, 0, "paging settles nothing");
    // After inspection, an explicit decision still emits exactly one response.
    overlay.handleInput(DOWN);
    overlay.handleInput(ENTER);
    assert.deepEqual(recorded.decisions, [
      { interaction: approval.interaction, decision: { type: "allow" } },
    ]);
  });

  it("makes every character of a very long reason line reachable", () => {
    const base = approvalInteraction();
    if (base.request.kind.type !== "approval") throw new Error("fixture");
    const marker = "REASON-END-MARKER";
    const approval: RoutedInteraction = {
      ...base,
      request: {
        ...base.request,
        kind: {
          ...base.request.kind,
          reason: `REASON-START ${"the runtime requires your review ".repeat(60)}${marker}`,
        },
      },
    };
    const width = 40;
    const { overlay, recorded } = surface({
      interactions: [approval],
      preferences: withExpandedInteractions(defaultPreferences(), [
        approval.interaction,
      ]),
    });
    const initial = rendered(overlay, width);
    assert.ok(initial.includes("REASON-START"), "the reason head shows first");
    assert.ok(!flattened(initial).includes(marker), "the reason tail starts past the viewport");
    let paged = initial;
    for (let page = 0; page < 8 && !flattened(paged).includes(marker); page += 1) {
      overlay.handleInput(PAGE_DOWN);
      paged = rendered(overlay, width);
    }
    assert.ok(flattened(paged).includes(marker), "the reason tail is reachable");
    assert.equal(recorded.decisions.length, 0, "paging settles nothing");
  });

  it("preserves a semantic space exactly on an expanded-detail wrap boundary", () => {
    const base = approvalInteraction();
    if (base.request.kind.type !== "approval") throw new Error("fixture");
    const approval: RoutedInteraction = {
      ...base,
      request: {
        ...base.request,
        kind: {
          ...base.request.kind,
          arguments: { command: "prefix foo bar suffix" },
        },
      },
    };
    // At width 21 the space between "prefix" and "foo" is the last column of
    // the first wrapped row of the argument line. A prose wrapper trims it
    // and the flattened content reads "prefixfoo"; the authorization surface
    // may not.
    const width = 21;
    const { overlay, recorded } = surface({
      interactions: [approval],
      preferences: withExpandedInteractions(defaultPreferences(), [
        approval.interaction,
      ]),
    });
    const rows = overlay.render(width).map((line) => plainText(line));
    assert.ok(
      rows.some((line) => line === '  "command": "prefix '),
      "the boundary row keeps its trailing space",
    );
    const page = rendered(overlay, width);
    assert.ok(
      flattened(page).includes('{  "command": "prefix foo bar suffix"}'),
      "the flattened detail reconstructs the authoritative line exactly",
    );
    for (const line of overlay.render(width)) {
      assert.ok(visibleWidth(line) <= width, "no horizontal overflow");
    }
    assert.equal(recorded.decisions.length, 0, "wrapping settles nothing");
  });

  it("preserves repeated and value-internal whitespace across wrap boundaries", () => {
    const base = approvalInteraction();
    if (base.request.kind.type !== "approval") throw new Error("fixture");
    const cases = [
      // The two spaces between "alpha" and "beta" are columns 17 and 18: one
      // ends the first row, the other opens the second, at width 18. Both
      // must survive; a trim on either side reads "alphabeta".
      {
        arguments: { value: "alpha  beta" },
        width: 18,
        boundaryRow: '  "value": "alpha ',
        reconstructed: '"alpha  beta"',
      },
      // Significant leading/trailing spaces inside the JSON string value,
      // with the trailing pair landing on row boundaries at width 27.
      {
        arguments: { value: "  payload tail  " },
        width: 27,
        boundaryRow: '  "value": "  payload tail ',
        reconstructed: '"  payload tail  "',
      },
    ];
    for (const { arguments: args, width, boundaryRow, reconstructed } of cases) {
      const approval: RoutedInteraction = {
        ...base,
        request: {
          ...base.request,
          kind: { ...base.request.kind, arguments: args },
        },
      };
      const { overlay, recorded } = surface({
        interactions: [approval],
        preferences: withExpandedInteractions(defaultPreferences(), [
          approval.interaction,
        ]),
      });
      const rows = overlay.render(width).map((line) => plainText(line));
      assert.ok(
        rows.some((line) => line === boundaryRow),
        `the boundary row keeps its whitespace at width ${width}`,
      );
      assert.ok(
        flattened(rendered(overlay, width)).includes(reconstructed),
        `the flattened detail reconstructs ${reconstructed} at width ${width}`,
      );
      assert.equal(recorded.decisions.length, 0, "wrapping settles nothing");
    }
  });

  it("preserves wide and composed graphemes across wrap boundaries", () => {
    const base = approvalInteraction();
    if (base.request.kind.type !== "approval") throw new Error("fixture");
    // CJK (two columns), an emoji, a composed grapheme, and another emoji:
    // the value is wider than the viewport, so the wrap must cut between
    // graphemes without breaking one and without changing the content.
    const value = "認証🔒é🚀";
    const approval: RoutedInteraction = {
      ...base,
      request: {
        ...base.request,
        kind: {
          ...base.request.kind,
          arguments: { note: value },
        },
      },
    };
    const width = 14;
    const { overlay, recorded } = surface({
      interactions: [approval],
      preferences: withExpandedInteractions(defaultPreferences(), [
        approval.interaction,
      ]),
    });
    const page = rendered(overlay, width);
    assert.ok(
      flattened(page).includes(value),
      "the flattened detail reconstructs the wide-grapheme value exactly",
    );
    for (const line of overlay.render(width)) {
      assert.ok(visibleWidth(line) <= width, "no row exceeds the width");
    }
    assert.equal(recorded.decisions.length, 0, "wrapping settles nothing");
  });

  it("loses no content when the viewport width changes", () => {
    const base = approvalInteraction();
    if (base.request.kind.type !== "approval") throw new Error("fixture");
    const tail = "-END-OF-AUTHORITATIVE-PAYLOAD";
    const approval: RoutedInteraction = {
      ...base,
      request: {
        ...base.request,
        kind: {
          ...base.request.kind,
          arguments: { payload: `BEGIN-${"x".repeat(1_000)}${tail}` },
        },
      },
    };
    const preferences = withExpandedInteractions(defaultPreferences(), [
      approval.interaction,
    ]);
    const totalRows = (width: number): number => {
      const { overlay } = surface({ interactions: [approval], preferences });
      const text = rendered(overlay, width);
      const paged = /of (\d+)/.exec(text);
      const complete = /detail: complete, (\d+) rows/.exec(text);
      const match = paged ?? complete;
      assert.ok(match !== null, "the position line reports the scroll domain");
      return Number(match[1]);
    };
    const narrow = totalRows(30);
    const wide = totalRows(90);
    assert.ok(narrow > wide, "a narrower width wraps into more rows");
    // The same tail is reachable at both widths, deterministically.
    for (const width of [30, 90]) {
      const { overlay } = surface({ interactions: [approval], preferences });
      let paged = rendered(overlay, width);
      for (let page = 0; page < 16 && !flattened(paged).includes(tail); page += 1) {
        overlay.handleInput(PAGE_DOWN);
        paged = rendered(overlay, width);
      }
      assert.ok(flattened(paged).includes(tail), `the tail is reachable at width ${width}`);
    }
  });

  it("keeps disclosure and paging presentation-only", () => {
    const base = approvalInteraction();
    if (base.request.kind.type !== "approval") throw new Error("fixture");
    const approval: RoutedInteraction = {
      ...base,
      request: {
        ...base.request,
        kind: {
          ...base.request.kind,
          arguments: { payload: `BEGIN-${"x".repeat(1_000)}-END` },
        },
      },
    };
    const { overlay, recorded } = surface({ interactions: [approval] });
    overlay.handleInput(CTRL_E);
    sync(
      overlay,
      [approval],
      approval.interaction,
      withExpandedInteractions(defaultPreferences(), [approval.interaction]),
    );
    rendered(overlay, 40);
    overlay.handleInput(PAGE_DOWN);
    overlay.handleInput(PAGE_UP);
    overlay.handleInput(CTRL_E);
    assert.deepEqual(recorded.expansions, [
      approval.interaction,
      approval.interaction,
    ]);
    assert.equal(recorded.decisions.length, 0, "no approval response");
    assert.equal(recorded.submissions.length, 0, "no questionnaire submission");
    assert.equal(recorded.declines.length, 0, "no questionnaire decline");
    assert.equal(recorded.dismissals.length, 0, "no dismissal");
    assert.equal(recorded.interrupts, 0, "no interruption");
  });
});
