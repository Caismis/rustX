import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { ConfirmationView } from "../src/ui/components/confirmation.ts";
import { plain } from "./support/render.ts";

describe("retained workspace confirmation", () => {
  it("describes discarded changes and confirms exactly once", () => {
    let confirmed = 0;
    let cancelled = 0;
    const view = new ConfirmationView({
      confirmLabel: "Dispose workspace",
      title: "Dispose retained workspace",
      subject: "Subagent conv-1-subagent-1",
      warning: "This removes the retained worktree and discards source changes.",
      onConfirm: () => { confirmed += 1; },
      onCancel: () => { cancelled += 1; },
    });

    assert.match(view.render(80).map(plain).join("\n"), /discards source changes/);
    view.handleInput("\t");
    view.handleInput("\r");
    view.handleInput("\r");
    assert.equal(confirmed, 1);
    assert.equal(cancelled, 0);
  });

  it("lets the user cancel without invoking the runtime action", () => {
    let confirmed = 0;
    let cancelled = 0;
    const view = new ConfirmationView({
      confirmLabel: "Dispose workspace",
      title: "Dispose retained workspace",
      subject: "Subagent conv-1-subagent-1",
      warning: "This removes the retained worktree and discards source changes.",
      onConfirm: () => { confirmed += 1; },
      onCancel: () => { cancelled += 1; },
    });

    view.handleInput("n");
    assert.equal(confirmed, 0);
    assert.equal(cancelled, 1);
  });
});

it("defaults Enter to Cancel and requires a focused destructive choice", () => {
  let confirmed = 0, cancelled = 0;
  const view = new ConfirmationView({ title: "Delete", subject: "target", warning: "permanent", confirmLabel: "Delete", onConfirm: () => confirmed++, onCancel: () => cancelled++ });
  assert.match(view.render(80).map(plain).join("\n"), /❯ Cancel/);
  view.handleInput("y"); assert.equal(confirmed, 0);
  view.handleInput("\r"); view.handleInput("\r");
  assert.equal(confirmed, 0); assert.equal(cancelled, 1);
});

it("keeps the selected action visible in a one-row narrow viewport", () => {
  const view = new ConfirmationView({ title: "Delete", subject: "target", warning: "permanent", confirmLabel: "Delete", onConfirm: () => {}, onCancel: () => {} });
  view.setBodyHeight(1); view.handleInput("\t");
  assert.match(view.render(8).map(plain).join(""), /❯ Delete/);
});
