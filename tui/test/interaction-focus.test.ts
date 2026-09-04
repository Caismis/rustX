/**
 * The presentation-only interaction focus model.
 *
 * These tests pin the deterministic contract Issue #185 requires of the
 * human-input queue: focus is derived from the authoritative sorted pending
 * list plus the previously focused routed identity, navigation never settles
 * anything, and every pending interaction — any mix of kinds, primary and
 * subagent, including several from one conversation — is independently
 * represented.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  compareInteractionRefs,
  moveInteractionFocus,
  reconcileInteractionFocus,
  sameInteractionRef,
} from "../src/presentation/interaction-focus.ts";
import type { RoutedInteraction } from "../src/protocol/types.ts";
import {
  approvalInteraction,
  childApprovalInteraction,
  childQuestionnaireInteraction,
  questionnaireInteraction,
} from "./support/fixtures.ts";

/** The five-interaction mixed queue from the issue's own example shape. */
function mixedQueue(): RoutedInteraction[] {
  return [
    approvalInteraction("attempt-1-interaction-approval-a"),
    questionnaireInteraction("attempt-1-interaction-question-b"),
    childApprovalInteraction("child-a-interaction-1", "implement"),
    childQuestionnaireInteraction("child-b-interaction-1", "reviewer"),
    childApprovalInteraction("child-c-interaction-1", "explore"),
  ].sort((left, right) =>
    compareInteractionRefs(left.interaction, right.interaction),
  );
}

describe("interaction focus", () => {
  it("sorts presentation order by the routed identity pair, never by kind or source", () => {
    const queue = mixedQueue();
    assert.deepEqual(
      queue.map(
        (entry) =>
          `${entry.interaction.conversation_id}::${entry.interaction.interaction_id}`,
      ),
      [
        "conv-child-1::child-a-interaction-1",
        "conv-child-1::child-b-interaction-1",
        "conv-child-1::child-c-interaction-1",
        "conv-test::attempt-1-interaction-approval-a",
        "conv-test::attempt-1-interaction-question-b",
      ],
    );
    // Several interactions from one conversation coexist; approval and
    // questionnaire coexist; primary and child coexist. No singleton slot.
    assert.equal(new Set(queue.map((entry) => entry.interaction.interaction_id)).size, 5);
  });

  it("focuses the smallest routed identity when nothing was focused", () => {
    const queue = mixedQueue();
    assert.deepEqual(reconcileInteractionFocus(queue, undefined), {
      conversation_id: "conv-child-1",
      interaction_id: "child-a-interaction-1",
    });
  });

  it("drops the focus when the queue empties", () => {
    assert.equal(
      reconcileInteractionFocus([], mixedQueue()[0]!.interaction),
      undefined,
    );
  });

  it("keeps a still-pending focus when unrelated interactions arrive or leave", () => {
    const queue = mixedQueue();
    const focused = queue[3]!.interaction; // a primary approval
    const withArrival = [
      ...queue,
      childQuestionnaireInteraction("child-a-interaction-0", "reviewer"),
    ].sort((left, right) =>
      compareInteractionRefs(left.interaction, right.interaction),
    );
    assert.deepEqual(reconcileInteractionFocus(withArrival, focused), focused);
    // Removing an unrelated pending interaction never disturbs the focus.
    const withoutUnrelated = queue.filter((_, index) => index !== 0);
    assert.deepEqual(reconcileInteractionFocus(withoutUnrelated, focused), focused);
  });

  it("advances to the successor when the focused interaction settles", () => {
    const queue = mixedQueue();
    const focused = queue[1]!.interaction;
    const remaining = queue.filter((_, index) => index !== 1);
    assert.deepEqual(reconcileInteractionFocus(remaining, focused), queue[2]!.interaction);
  });

  it("falls back to the new last item when the removed focus was last", () => {
    const queue = mixedQueue();
    const focused = queue[queue.length - 1]!.interaction;
    const remaining = queue.slice(0, -1);
    assert.deepEqual(
      reconcileInteractionFocus(remaining, focused),
      queue[queue.length - 2]!.interaction,
    );
  });

  it("moves focus deterministically and wraps at both ends", () => {
    const queue = mixedQueue();
    const first = queue[0]!.interaction;
    const second = queue[1]!.interaction;
    const last = queue[queue.length - 1]!.interaction;
    assert.deepEqual(moveInteractionFocus(queue, first, 1), second);
    assert.deepEqual(moveInteractionFocus(queue, first, -1), last);
    assert.deepEqual(moveInteractionFocus(queue, last, 1), first);
  });

  it("reconciles an unknown current identity before navigating", () => {
    const queue = mixedQueue();
    const stale = {
      conversation_id: "conv-gone",
      interaction_id: "interaction-9",
    };
    // "conv-gone" sorts between conv-child-1 and conv-test, so its successor
    // is the first primary interaction.
    assert.deepEqual(
      moveInteractionFocus(queue, stale, 1),
      queue[4]!.interaction,
    );
  });

  it("navigation is pure: the pending projection is never mutated", () => {
    const queue = mixedQueue();
    const before = JSON.stringify(queue);
    moveInteractionFocus(queue, queue[0]!.interaction, 1);
    reconcileInteractionFocus(queue, queue[2]!.interaction);
    assert.equal(JSON.stringify(queue), before);
  });
});

/** A pending interaction with an exact, caller-chosen routed identity. */
function withIdentity(
  conversationId: string,
  interactionId: string,
): RoutedInteraction {
  const base = approvalInteraction(interactionId);
  return {
    ...base,
    interaction: {
      conversation_id: conversationId,
      interaction_id: interactionId,
    },
    request: { ...base.request, id: interactionId, conversation_id: conversationId },
  };
}

describe("identity equality and presentation ordering", () => {
  it("semantic equality is exact field equality, never a collation result", () => {
    const ref = { conversation_id: "conv-1", interaction_id: "int-1" };
    assert.ok(sameInteractionRef(ref, { conversation_id: "conv-1", interaction_id: "int-1" }));
    assert.ok(!sameInteractionRef(ref, { conversation_id: "conv-1", interaction_id: "int-2" }));
    assert.ok(!sameInteractionRef(ref, { conversation_id: "conv-2", interaction_id: "int-1" }));
  });

  it("distinct ids are never equal through collation equivalence", () => {
    // Composed vs decomposed Unicode collates as equal under common locale
    // collation, but these are distinct opaque runtime identities.
    const composed = { conversation_id: "conv-caf\u00e9", interaction_id: "int-1" };
    const decomposed = { conversation_id: "conv-cafe\u0301", interaction_id: "int-1" };
    assert.ok(!sameInteractionRef(composed, decomposed));
    assert.notEqual(compareInteractionRefs(composed, decomposed), 0);
    // Case and punctuation variants are distinct identities as well.
    assert.ok(!sameInteractionRef(
      { conversation_id: "conv-a", interaction_id: "int-1" },
      { conversation_id: "conv-A", interaction_id: "int-1" },
    ));
    assert.ok(!sameInteractionRef(
      { conversation_id: "conv-1", interaction_id: "int-1" },
      { conversation_id: "conv_1", interaction_id: "int-1" },
    ));
  });

  it("presentation ordering is locale-independent code-unit order", () => {
    // UTF-16 code-unit order: uppercase before lowercase, and digit
    // characters by code ("int-10" < "int-2") — identical on every host,
    // under every ambient locale.
    assert.ok(compareInteractionRefs(
      { conversation_id: "conv-Z", interaction_id: "x" },
      { conversation_id: "conv-a", interaction_id: "x" },
    ) < 0);
    assert.ok(compareInteractionRefs(
      { conversation_id: "conv-1", interaction_id: "int-10" },
      { conversation_id: "conv-1", interaction_id: "int-2" },
    ) < 0);
    // Antisymmetric, and exact identities order equal.
    const a = { conversation_id: "conv-1", interaction_id: "int-1" };
    const b = { conversation_id: "conv-1", interaction_id: "int-2" };
    assert.equal(
      Math.sign(compareInteractionRefs(a, b)),
      -Math.sign(compareInteractionRefs(b, a)),
    );
    assert.equal(compareInteractionRefs(a, { ...a }), 0);
  });

  it("focus reconciliation stays deterministic over case, punctuation, and Unicode ids", () => {
    const queue = [
      withIdentity("conv-a", "int-2"),
      withIdentity("conv-A", "int-1"),
      withIdentity("conv-caf\u00e9", "int-1"),
      withIdentity("conv-cafe\u0301", "int-1"),
    ];
    // Code-unit order: conv-A < conv-a < conv-cafe+combining < conv-caf\u00e9.
    assert.deepEqual(reconcileInteractionFocus(queue, undefined), {
      conversation_id: "conv-A",
      interaction_id: "int-1",
    });
    // The removed focused identity advances to its code-unit successor.
    const withoutFirst = queue.filter(
      (entry) => entry.interaction.conversation_id !== "conv-A",
    );
    assert.deepEqual(
      reconcileInteractionFocus(withoutFirst, {
        conversation_id: "conv-A",
        interaction_id: "int-1",
      }),
      { conversation_id: "conv-a", interaction_id: "int-2" },
    );
    // Navigation wraps through the same deterministic order.
    assert.deepEqual(
      moveInteractionFocus(queue, { conversation_id: "conv-caf\u00e9", interaction_id: "int-1" }, 1),
      { conversation_id: "conv-A", interaction_id: "int-1" },
    );
    assert.deepEqual(
      moveInteractionFocus(queue, { conversation_id: "conv-A", interaction_id: "int-1" }, -1),
      { conversation_id: "conv-caf\u00e9", interaction_id: "int-1" },
    );
  });
});
