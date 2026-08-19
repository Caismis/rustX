/**
 * `ToolCallId` and `ToolExecutionId` are two presentation identity domains.
 *
 * rustX models them as separate identities and they both happen to serialize
 * as transparent strings, so nothing on the wire stops the same string from
 * appearing in both. The presentation preferences must therefore keep them
 * apart *structurally* — never by a naming convention, and never by searching
 * one string across both namespaces and taking the first match.
 *
 * Every assertion below uses the deliberately colliding value `same`, so a
 * single string-keyed set would fail every one of them.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { CommandDispatcher } from "../src/commands/dispatcher.ts";
import { renderBackground } from "../src/ui/components/activity.ts";
import { renderToolCard } from "../src/ui/components/tool-card.ts";
import type { CorrelatedTool } from "../src/presentation/tools.ts";
import {
  DEFAULT_PREVIEW_CHARS,
  DEFAULT_PREVIEW_LINES,
  defaultPreferences,
  isBackgroundExecutionExpanded,
  isToolCallExpanded,
  withAllCollapsed,
  withExpandedBackgroundExecutions,
  withExpandedToolCalls,
  withToggledBackgroundExecution,
  withToggledToolCall,
} from "../src/ui/preferences.ts";
import { plainText } from "../src/ui/theme.ts";
import { backgroundExecution, toolResult } from "./support/fixtures.ts";
import { stateOf } from "./support/render.ts";

const COLLIDING = "same";

const longBody = Array.from({ length: 40 }, (_, index) => `line ${index}`).join(
  "\n",
);

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

  it("does not cross-toggle when both domains hold the same string", () => {
    let preferences = withToggledToolCall(defaultPreferences(), COLLIDING);
    preferences = withToggledBackgroundExecution(preferences, COLLIDING);
    assert.equal(isToolCallExpanded(preferences, COLLIDING), true);
    assert.equal(isBackgroundExecutionExpanded(preferences, COLLIDING), true);

    // Collapsing one leaves the other exactly as it was.
    preferences = withToggledToolCall(preferences, COLLIDING);
    assert.equal(isToolCallExpanded(preferences, COLLIDING), false);
    assert.equal(isBackgroundExecutionExpanded(preferences, COLLIDING), true);
  });

  it("expands only the card whose domain was addressed", () => {
    const callExpanded = withToggledToolCall(defaultPreferences(), COLLIDING);
    assert.ok(foregroundCard(callExpanded).includes("line 39"));
    assert.ok(
      !backgroundCard(callExpanded).includes("line 39"),
      "the background card stayed collapsed",
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
  });

  it("collapses both domains for `none`", () => {
    const both = withExpandedBackgroundExecutions(
      withExpandedToolCalls(defaultPreferences(), [COLLIDING]),
      [COLLIDING],
    );
    const collapsed = withAllCollapsed(both);
    assert.equal(isToolCallExpanded(collapsed, COLLIDING), false);
    assert.equal(isBackgroundExecutionExpanded(collapsed, COLLIDING), false);
    assert.ok(!foregroundCard(collapsed).includes("line 39"));
    assert.ok(!backgroundCard(collapsed).includes("line 39"));
  });

  it("expands both domains for `all`", () => {
    const both = withExpandedBackgroundExecutions(
      withExpandedToolCalls(defaultPreferences(), [COLLIDING]),
      [COLLIDING],
    );
    assert.ok(foregroundCard(both).includes("line 39"));
    assert.ok(backgroundCard(both).includes("line 39"));
  });

  it("relies on no naming convention to tell the domains apart", () => {
    // `call_*` / `exec_*` are incidental wire spellings. The domains are kept
    // apart by structure, so an id that looks like the *other* domain still
    // behaves as the domain it was filed under.
    const misleading = withToggledToolCall(defaultPreferences(), "exec_7");
    assert.equal(isToolCallExpanded(misleading, "exec_7"), true);
    assert.equal(isBackgroundExecutionExpanded(misleading, "exec_7"), false);
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

  it("rejects a background request with no identity rather than guessing", async () => {
    const outcome = await expand("background");
    assert.equal(outcome.kind, "message");
    assert.equal(outcome.kind === "message" ? outcome.level : "", "error");
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
