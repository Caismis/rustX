/**
 * `ToolCallId`, `ToolExecutionId`, and `InteractionId` are three presentation
 * identity domains.
 *
 * rustX models them as separate identities and all three happen to serialize
 * as transparent strings, so nothing on the wire stops the same string from
 * appearing in all three. The presentation preferences must therefore keep
 * them apart *structurally* — never by a naming convention, and never by
 * searching one string across the namespaces and taking the first match.
 *
 * Every assertion below uses the deliberately colliding value `same`, so a
 * single string-keyed set would fail every one of them.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { CommandDispatcher } from "../src/commands/dispatcher.ts";
import {
  renderBackground,
  renderInteractionSection,
} from "../src/ui/components/activity.ts";
import { renderToolCard } from "../src/ui/components/tool-card.ts";
import type { CorrelatedTool } from "../src/presentation/tools.ts";
import {
  DEFAULT_PREVIEW_CHARS,
  DEFAULT_PREVIEW_LINES,
  defaultPreferences,
  isBackgroundExecutionExpanded,
  isInteractionExpanded,
  isToolCallExpanded,
  withAllCollapsed,
  withExpandedBackgroundExecutions,
  withExpandedInteractions,
  withExpandedToolCalls,
  withToggledBackgroundExecution,
  withToggledInteraction,
  withToggledToolCall,
} from "../src/ui/preferences.ts";
import { plainText } from "../src/ui/theme.ts";
import {
  approvalInteraction,
  backgroundExecution,
  toolResult,
} from "./support/fixtures.ts";
import { stateOf } from "./support/render.ts";

const COLLIDING = "same";

const longBody = Array.from({ length: 40 }, (_, index) => `line ${index}`).join(
  "\n",
);

/** The state `/expand all` produces when every domain holds `same`. */
function allExpanded() {
  return withExpandedInteractions(
    withExpandedBackgroundExecutions(
      withExpandedToolCalls(defaultPreferences(), [COLLIDING]),
      [COLLIDING],
    ),
    [COLLIDING],
  );
}

function foregroundTool(): CorrelatedTool {
  return {
    callId: COLLIDING,
    toolId: "tool-unknown",
    name: "unknown_tool",
    argumentsText: "{}",
    lifecycle: {
      type: "settled",
      result: toolResult({ content: [{ type: "text", text: longBody }] }),
    },
    committed: true,
  };
}

function backgroundCard(preferences = defaultPreferences()): string {
  return plainText(
    renderBackground(
      backgroundExecution(COLLIDING, "succeeded", {
        result: toolResult({ content: [{ type: "text", text: longBody }] }),
      }),
      preferences,
    ),
  );
}

/**
 * One approval whose reason carries the same marker the other two cards do,
 * so a cross-toggle would be visible rather than merely possible.
 */
function interactionCard(preferences = defaultPreferences()): string {
  return plainText(
    renderInteractionSection(
      stateOf({
        pending_interactions: [
          {
            ...approvalInteraction(COLLIDING),
            kind: {
              ...approvalInteraction(COLLIDING).kind,
              reason: longBody,
            },
          },
        ],
      }),
      preferences,
    ),
  );
}

function foregroundCard(preferences = defaultPreferences()): string {
  return plainText(
    renderToolCard(foregroundTool(), {
      expanded: isToolCallExpanded(preferences, COLLIDING),
      budget: preferences.previewBudget,
    }),
  );
}

describe("identity domains never alias", () => {
  it("keeps one string in two domains as two independent preferences", () => {
    const preferences = withToggledToolCall(defaultPreferences(), COLLIDING);
    assert.equal(isToolCallExpanded(preferences, COLLIDING), true);
    assert.equal(
      isBackgroundExecutionExpanded(preferences, COLLIDING),
      false,
      "a ToolCallId never expands a ToolExecutionId with the same wire string",
    );

    const other = withToggledBackgroundExecution(defaultPreferences(), COLLIDING);
    assert.equal(isBackgroundExecutionExpanded(other, COLLIDING), true);
    assert.equal(
      isToolCallExpanded(other, COLLIDING),
      false,
      "a ToolExecutionId never expands a ToolCallId with the same wire string",
    );
  });

  it("does not cross-toggle when all three domains hold the same string", () => {
    let preferences = withToggledToolCall(defaultPreferences(), COLLIDING);
    preferences = withToggledBackgroundExecution(preferences, COLLIDING);
    preferences = withToggledInteraction(preferences, COLLIDING);
    assert.equal(isToolCallExpanded(preferences, COLLIDING), true);
    assert.equal(isBackgroundExecutionExpanded(preferences, COLLIDING), true);
    assert.equal(isInteractionExpanded(preferences, COLLIDING), true);

    // Collapsing one leaves the other two exactly as they were.
    preferences = withToggledToolCall(preferences, COLLIDING);
    assert.equal(isToolCallExpanded(preferences, COLLIDING), false);
    assert.equal(isBackgroundExecutionExpanded(preferences, COLLIDING), true);
    assert.equal(isInteractionExpanded(preferences, COLLIDING), true);

    preferences = withToggledInteraction(preferences, COLLIDING);
    assert.equal(isBackgroundExecutionExpanded(preferences, COLLIDING), true);
    assert.equal(isInteractionExpanded(preferences, COLLIDING), false);
  });

  it("expands only the card whose domain was addressed", () => {
    const callExpanded = withToggledToolCall(defaultPreferences(), COLLIDING);
    assert.ok(foregroundCard(callExpanded).includes("line 39"));
    assert.ok(
      !backgroundCard(callExpanded).includes("line 39"),
      "the background card stayed collapsed",
    );
    assert.ok(
      !interactionCard(callExpanded).includes("line 39"),
      "the interaction card stayed collapsed",
    );

    const executionExpanded = withToggledBackgroundExecution(
      defaultPreferences(),
      COLLIDING,
    );
    assert.ok(backgroundCard(executionExpanded).includes("line 39"));
    assert.ok(
      !foregroundCard(executionExpanded).includes("line 39"),
      "the tool card stayed collapsed",
    );
    assert.ok(
      !interactionCard(executionExpanded).includes("line 39"),
      "the interaction card stayed collapsed",
    );

    const interactionExpanded = withToggledInteraction(
      defaultPreferences(),
      COLLIDING,
    );
    assert.ok(interactionCard(interactionExpanded).includes("line 39"));
    assert.ok(
      !foregroundCard(interactionExpanded).includes("line 39"),
      "the tool card stayed collapsed",
    );
    assert.ok(
      !backgroundCard(interactionExpanded).includes("line 39"),
      "the background card stayed collapsed",
    );
  });

  it("collapses all three domains for `none`", () => {
    const collapsed = withAllCollapsed(allExpanded());
    assert.equal(isToolCallExpanded(collapsed, COLLIDING), false);
    assert.equal(isBackgroundExecutionExpanded(collapsed, COLLIDING), false);
    assert.equal(isInteractionExpanded(collapsed, COLLIDING), false);
    assert.ok(!foregroundCard(collapsed).includes("line 39"));
    assert.ok(!backgroundCard(collapsed).includes("line 39"));
    assert.ok(!interactionCard(collapsed).includes("line 39"));
  });

  it("expands all three domains for `all`", () => {
    const all = allExpanded();
    assert.ok(foregroundCard(all).includes("line 39"));
    assert.ok(backgroundCard(all).includes("line 39"));
    assert.ok(interactionCard(all).includes("line 39"));
  });

  it("relies on no naming convention to tell the domains apart", () => {
    // See also below: `exec_7` filed as a ToolCallId stays a ToolCallId.
    // `call_*` / `exec_*` are incidental wire spellings. The domains are kept
    // apart by structure, so an id that looks like the *other* domain still
    // behaves as the domain it was filed under.
    const misleading = withToggledToolCall(defaultPreferences(), "exec_7");
    assert.equal(isToolCallExpanded(misleading, "exec_7"), true);
    assert.equal(isBackgroundExecutionExpanded(misleading, "exec_7"), false);
    assert.equal(isInteractionExpanded(misleading, "exec_7"), false);
  });

  it("changes no semantic display fact", () => {
    // Whatever the preferences say, the runtime's own settlement, duration,
    // and truncation report identically.
    const preferences = withToggledToolCall(defaultPreferences(), COLLIDING);
    for (const rendered of [foregroundCard(), foregroundCard(preferences)]) {
      assert.match(rendered.split("\n")[0] ?? "", /ok/);
      assert.match(rendered, /12ms/);
    }
    assert.equal(defaultPreferences().previewBudget.maxLines, DEFAULT_PREVIEW_LINES);
    assert.equal(defaultPreferences().previewBudget.maxChars, DEFAULT_PREVIEW_CHARS);
  });
});

describe("/expand addresses one domain at a time", () => {
  // The parser is the unit under test; the dispatcher only needs a state to
  // consider itself attached. No command below reaches the runtime.
  const dispatcher = new CommandDispatcher({
    session: { state: stateOf() } as never,
    diagnostics: () => ({}) as never,
  });

  async function expand(argument: string) {
    return dispatcher.submit(`/expand${argument === "" ? "" : ` ${argument}`}`);
  }

  it("treats a bare id as a ToolCallId, always", async () => {
    assert.deepEqual(await expand("call-7"), {
      kind: "preference",
      preference: { type: "expand_call", callId: "call-7" },
    });
    // Even when the string looks like an execution id: there is no search
    // across namespaces and no "first match wins".
    assert.deepEqual(await expand("exec-7"), {
      kind: "preference",
      preference: { type: "expand_call", callId: "exec-7" },
    });
  });

  it("addresses a background execution only when told to", async () => {
    assert.deepEqual(await expand("background exec-7"), {
      kind: "preference",
      preference: { type: "expand_background", executionId: "exec-7" },
    });
    assert.deepEqual(await expand("bg exec-7"), {
      kind: "preference",
      preference: { type: "expand_background", executionId: "exec-7" },
    });
  });

  it("addresses a pending interaction only when told to", async () => {
    assert.deepEqual(await expand("interaction attempt-1-interaction-1"), {
      kind: "preference",
      preference: {
        type: "expand_interaction",
        interactionId: "attempt-1-interaction-1",
      },
    });
  });

  it("rejects a background or interaction request with no identity", async () => {
    for (const argument of ["background", "interaction"]) {
      const outcome = await expand(argument);
      assert.equal(outcome.kind, "transient", argument);
      assert.equal(outcome.kind === "transient" ? outcome.level : "", "error");
    }
  });

  it("spells latest, all, and none exactly", async () => {
    for (const [argument, target] of [
      ["", "latest"],
      ["latest", "latest"],
      ["all", "all"],
      ["none", "none"],
    ] as const) {
      assert.deepEqual(
        await expand(argument),
        { kind: "preference", preference: { type: "expand", target } },
        argument,
      );
    }
  });
});
