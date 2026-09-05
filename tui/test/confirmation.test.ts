import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { ConfirmationView } from "../src/ui/components/confirmation.ts";
import { plain } from "./support/render.ts";

describe("retained workspace confirmation", () => {
  it("describes discarded changes and confirms exactly once", () => {
    let confirmed = 0;
    let cancelled = 0;
    const view = new ConfirmationView({
      title: "Dispose retained workspace",
      subject: "Subagent conv-1-subagent-1",
      warning: "This removes the retained worktree and discards source changes.",
      onConfirm: () => { confirmed += 1; },
      onCancel: () => { cancelled += 1; },
    });

    assert.match(view.render(80).map(plain).join("\n"), /discards source changes/);
    view.handleInput("y");
    view.handleInput("y");
    assert.equal(confirmed, 1);
    assert.equal(cancelled, 0);
  });

  it("lets the user cancel without invoking the runtime action", () => {
    let confirmed = 0;
    let cancelled = 0;
    const view = new ConfirmationView({
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
