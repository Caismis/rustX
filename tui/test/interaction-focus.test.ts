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
