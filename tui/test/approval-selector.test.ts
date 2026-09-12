import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { ApprovalSelector } from "../src/ui/components/approval-selector.ts";
import { plainText } from "../src/ui/theme.ts";
import { stateOf } from "./support/render.ts";
import { attemptView } from "./support/fixtures.ts";
import type { ApprovalMode } from "../src/protocol/types.ts";

function harness() {
  let state = stateOf({ attempt: attemptView(), effective_approval_mode: "policy", pending_approval_mode: "full_access" });
  let finish!: () => void;
  const held = new Promise<void>((resolve) => { finish = resolve; });
  const requests: ApprovalMode[] = [];
  let closed = 0;
  const selector = new ApprovalSelector({ state: () => state,
    submit: (mode) => { requests.push(mode); return held; },
    close: () => { closed++; }, change: () => {},
  });
  return { selector, requests, finish, held, closed: () => closed,
    replace: (next: typeof state) => { state = next; },
    text: () => plainText(selector.render(100).join("\n")),
  };
}

describe("approval picker intent boundary", () => {
  it("opening, navigation and Esc issue zero requests; effective and pending are distinct", () => {
    const h = harness();
    assert.match(h.text(), /✓ Policy/);
    assert.match(h.text(), /Current attempt: Policy/);
    assert.match(h.text(), /Next attempt: Full access/);
    h.selector.handleInput("\x1b[B");
    assert.match(h.text(), /❯   Full access/);
    assert.match(h.text(), /✓ Policy/);
    h.selector.handleInput("\x1b");
    assert.deepEqual(h.requests, []);
    assert.equal(h.closed(), 1);
  });
  it("Policy submits exactly once with a deferred response and no optimistic effective mode", async () => {
    const h = harness();
    h.replace(stateOf({ effective_approval_mode: "full_access" }));
    h.selector.handleInput("\r"); h.selector.handleInput("\r");
    assert.deepEqual(h.requests, ["policy"]);
    assert.match(h.text(), /Effective: Full access/);
    assert.equal(h.closed(), 0);
    h.finish(); await h.held;
    assert.equal(h.closed(), 1);
  });
  it("Full access selection opens safe-default confirmation; Enter cancels with zero mutations", () => {
    const h = harness();
    h.selector.handleInput("\x1b[B"); h.selector.handleInput("\r");
    assert.equal(h.selector.popupTitle(), "Enable full access?");
    assert.match(h.text(), /❯ Cancel/);
    assert.match(h.text(), /Already-admitted Tools/);
    assert.match(h.text(), /command execution/);
    assert.match(h.text(), /Questionnaire or Workflow Review/);
    assert.match(h.text(), /filesystem\/network sandbox profile/);
    assert.deepEqual(h.requests, []);
    h.selector.handleInput("\r");
    assert.equal(h.closed(), 1);
    assert.deepEqual(h.requests, []);
  });
  it("explicit Full access confirmation submits once; pending keys cannot resubmit", async () => {
    const h = harness();
    h.selector.handleInput("\x1b[B"); h.selector.handleInput("\r");
    h.selector.handleInput("\t"); h.selector.handleInput("\r");
    for (const key of ["\r", "\t", "\r", "\x1b[A", "\r"]) h.selector.handleInput(key);
    assert.deepEqual(h.requests, ["full_access"]);
    assert.match(h.text(), /Current attempt: Policy/);
    h.replace(stateOf({ effective_approval_mode: "full_access" }));
    assert.match(h.text(), /Effective: Full access/);
    assert.doesNotMatch(h.text(), /Next attempt/);
    h.finish(); await h.held;
  });
  it("snapshot replacement while confirmation is open cannot promote stale highlighted state", () => {
    const h = harness();
    h.selector.handleInput("\x1b[B"); h.selector.handleInput("\r");
    h.replace(stateOf({ attempt: attemptView(), effective_approval_mode: "full_access", pending_approval_mode: "policy" }));
    assert.match(h.text(), /Current attempt: Full access/);
    assert.match(h.text(), /Next attempt: Policy/);
    h.selector.handleInput("\x1b");
    assert.deepEqual(h.requests, []);
  });
  it("keeps the selected row visible and confirmation copy scrollable at constrained sizes", () => {
    const h = harness();
    h.selector.setBodyHeight(1); h.selector.handleInput("\x1b[B");
    assert.match(plainText(h.selector.render(30).join("")), /Full access/);
    h.selector.handleInput("\r"); h.selector.setBodyHeight(4);
    for (let i = 0; i < 30; i++) h.selector.handleInput("\x1b[B");
    assert.match(h.text(), /sandbox profile/);
    assert.deepEqual(h.requests, []);
  });
});


describe("approval native lifetime labels", () => {
  for (const phase of ["absent", "settled"] as const) {
    for (const pending of [undefined, "full_access"] as const) {
      it(`${phase} attempt displays effective mode without inventing current work${pending ? " even with pending transition" : ""}`, () => {
        const h = harness();
        h.replace(stateOf({ effective_approval_mode: "policy", pending_approval_mode: pending,
          attempt: phase === "absent" ? undefined : attemptView({
            phase: { type: "settled", outcome: { type: "completed", finish_reason: { type: "stop" } } },
          }),
        }));
        assert.match(h.text(), /Effective: Policy/);
        assert.doesNotMatch(h.text(), /Current attempt/);
        if (pending) assert.match(h.text(), /Next attempt: Full access/);
        else assert.doesNotMatch(h.text(), /Next attempt/);
        assert.deepEqual(h.requests, []);
      });
    }
  }
  for (const phase of ["admitted", "running"] as const) {
    for (const pending of [undefined, "full_access"] as const) {
      it(`${phase} attempt labels the frozen effective mode${pending ? " and distinct next attempt" : " without a fabricated transition"}`, () => {
        const h = harness();
        h.replace(stateOf({ attempt: attemptView({ phase: { type: phase } }),
          effective_approval_mode: "policy", pending_approval_mode: pending,
        }));
        assert.match(h.text(), /Current attempt: Policy/);
        if (pending) assert.match(h.text(), /Next attempt: Full access/);
        else assert.doesNotMatch(h.text(), /Next attempt/);
        assert.deepEqual(h.requests, []);
      });
    }
  }
});
