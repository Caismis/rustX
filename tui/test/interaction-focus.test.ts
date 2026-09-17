/**
 * The presentation-only interaction focus model.
 *
 * These tests pin the deterministic contract Issue #185 requires of the
 * human-input queue: focus is derived from the authoritative ordered pending
 * list plus the previously focused routed identity, navigation never settles
 * anything, and every pending interaction — any mix of kinds, primary and
 * subagent, including several from one conversation — is independently
 * represented.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  moveInteractionFocus,
  reconcileInteractionFocus,
  sameInteractionRef,
} from "../src/presentation/interaction-focus.ts";
import type { RoutedInteraction } from "../src/protocol/app-server.ts";
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
  ];
}

describe("interaction focus", () => {
  it("focuses the first native publication, independent of UUID order", () => {
    const queue = mixedQueue();
    assert.deepEqual(reconcileInteractionFocus(queue, undefined), queue[0]!.interaction);
    assert.deepEqual(reconcileInteractionFocus([...queue].reverse(), undefined), queue.at(-1)!.interaction);
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
    ];
    assert.deepEqual(reconcileInteractionFocus(withArrival, focused), focused);
    // Removing an unrelated pending interaction never disturbs the focus.
    const withoutUnrelated = queue.filter((_, index) => index !== 0);
    assert.deepEqual(reconcileInteractionFocus(withoutUnrelated, focused), focused);
  });

  it("returns to the first native item when the focused interaction settles", () => {
    const queue = mixedQueue();
    const focused = queue[1]!.interaction;
    const remaining = queue.filter((_, index) => index !== 1);
    assert.deepEqual(reconcileInteractionFocus(remaining, focused), queue[0]!.interaction);
  });

  it("uses native order when the removed focus was last", () => {
    const queue = mixedQueue();
    const focused = queue[queue.length - 1]!.interaction;
    const remaining = queue.slice(0, -1);
    assert.deepEqual(
      reconcileInteractionFocus(remaining, focused),
      queue[0]!.interaction,
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
      conversation_id: "conv_196f78ff-e909-7914-90eb-e95e7f64fca0",
      interaction_id: "interaction-9",
    };
    // Unknown focus first reconciles to the native first item.
    assert.deepEqual(
      moveInteractionFocus(queue, stale, 1),
      queue[1]!.interaction,
    );
  });

  it("navigation is pure: the pending projection is never mutated", () => {
    const queue = mixedQueue();
    const before = JSON.stringify(queue);
    moveInteractionFocus(queue, queue[0]!.interaction, 1);
    reconcileInteractionFocus(queue, queue[0]!.interaction);
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
    const ref = { conversation_id: "conv_36524fd8-f674-7fc2-b125-06d01fee0e18", interaction_id: "int-1" };
    assert.ok(sameInteractionRef(ref, { conversation_id: "conv_36524fd8-f674-7fc2-b125-06d01fee0e18", interaction_id: "int-1" }));
    assert.ok(!sameInteractionRef(ref, { conversation_id: "conv_36524fd8-f674-7fc2-b125-06d01fee0e18", interaction_id: "int-2" }));
    assert.ok(!sameInteractionRef(ref, { conversation_id: "conv_1eef1854-fea7-788b-9e49-ca0ec811fb0c", interaction_id: "int-1" }));
  });

  it("different UUIDs never compare equal", () => {
    assert.ok(!sameInteractionRef(withIdentity("conv_00000000-0000-7000-8000-000000000001", "int-1").interaction, withIdentity("conv_00000000-0000-7000-8000-000000000002", "int-1").interaction));
  });
});
