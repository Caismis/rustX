import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { TransientFeedbackSurface } from "../src/ui/components/transient-feedback.ts";

describe("TransientFeedbackSurface", () => {
  it("replaces feedback, bounds multiline errors, and clears on acknowledgement", () => {
    const surface = new TransientFeedbackSurface();
    surface.replace({ level: "info", text: "first result" });
    surface.replace({
      level: "error",
      text: "line one\nline two\nline three\nline four",
    });

    assert.equal(surface.feedback?.text.includes("first result"), false);
    const rendered = surface.render(80).join("\n");
    assert.match(rendered, /line one/);
    assert.match(rendered, /line three/);
    assert.doesNotMatch(rendered, /line four/);

    surface.acknowledge();
    assert.equal(surface.feedback, undefined);
    assert.deepEqual(surface.render(80), []);

    surface.replace({ level: "info", text: "attachment-local" });
    surface.clear();
    assert.deepEqual(
      surface.render(80),
      [],
      "attachment/session replacement clears the item",
    );
  });
});
