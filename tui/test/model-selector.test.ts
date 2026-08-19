/**
 * The model selector, driven as a component without a terminal.
 *
 * Everything asserted here is a `CatalogModelView` field the runtime
 * published. The selector must never add a capability, invent a reasoning
 * scale, or reorder a catalog it was handed.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  ModelSelector,
  capabilityLine,
  reasoningLine,
} from "../src/ui/components/model-selector.ts";
import { plainText } from "../src/ui/theme.ts";
import type { CatalogModelView } from "../src/protocol/types.ts";
import { attemptModel, catalogModel, sessionModel } from "./support/fixtures.ts";

const CATALOG: CatalogModelView[] = [
  catalogModel("alpha/model-a", {
    effectiveCapabilities: {
      inputModalities: ["text", "image"],
      outputModalities: ["text"],
      toolCalls: true,
      reasoning: true,
    },
    contextWindow: 200_000,
    maxOutputTokens: 8_192,
    reasoningProfiles: [
      { id: "low", enabled: true },
      { id: "medium", enabled: true },
      { id: "high", enabled: true },
    ],
    defaultReasoningProfile: "medium",
  }),
  catalogModel("beta/model-b", { protocol: "openai_responses" }),
  catalogModel("beta/sonnet-x", { protocol: "anthropic_messages" }),
];

function selector(overrides: Partial<ConstructorParameters<typeof ModelSelector>[0]> = {}) {
  return new ModelSelector({
    models: CATALOG,
    sessionModel: sessionModel("alpha/model-a"),
    ...overrides,
  });
}

function lines(component: ModelSelector, width = 80): string[] {
  return component.render(width).map(plainText);
}

describe("search", () => {
  it("shows the catalog in the runtime's own order when the query is empty", () => {
    assert.deepEqual(
      selector()
        .visibleModels()
        .map((model) => model.model),
      ["alpha/model-a", "beta/model-b", "beta/sonnet-x"],
    );
  });

  it("filters fuzzily and deterministically", () => {
    const component = selector();
    component.setQuery("sonnet");
    assert.deepEqual(
      component.visibleModels().map((model) => model.model),
      ["beta/sonnet-x"],
    );

    component.setQuery("bt");
    const twice = selector();
    twice.setQuery("bt");
    assert.deepEqual(
      component.visibleModels().map((model) => model.model),
      twice.visibleModels().map((model) => model.model),
      "the same query over the same catalog gives the same order",
    );
  });

  it("reports a no-results state instead of an empty overlay", () => {
    const component = selector();
    component.setQuery("zzzz");
    assert.equal(component.visibleModels().length, 0);
    assert.ok(
      lines(component).some((line) => line.includes('no model matches "zzzz"')),
    );
  });

  it("accepts printable input and ignores control sequences", () => {
    const component = selector();
    component.handleInput("s");
    component.handleInput("o");
    assert.equal(component.query, "so");
    component.handleInput("[5~");
    assert.equal(component.query, "so", "an unhandled key never types itself");
    component.handleInput("");
    assert.equal(component.query, "s");
  });
});

describe("displayed metadata", () => {
  it("shows published capability and window for the highlighted row", () => {
    const rendered = lines(selector()).join("\n");
    assert.match(rendered, /Chat Completions · 200k ctx · 8k out · tools · in text\/image/);
  });

  it("renders reasoning profiles exactly as the catalog published them", () => {
    // The row states the *catalog's* fallback and says so in those words, so
    // it can never be read as the session's current configuration.
    assert.equal(
      reasoningLine(CATALOG[0]!),
      "catalog reasoning: low medium high (catalog default medium)",
    );
    // A model without reasoning is reported as unsupported, not as "off".
    assert.equal(reasoningLine(CATALOG[1]!), "catalog reasoning: unsupported");
    // Reasoning-capable with no selectable profile is its own third case: no
    // universal off/low/medium/high is invented.
    assert.equal(
      reasoningLine(
        catalogModel("g/h", {
          effectiveCapabilities: {
            inputModalities: ["text"],
            outputModalities: ["text"],
            toolCalls: true,
            reasoning: true,
          },
        }),
      ),
      "catalog reasoning: supported, no selectable profile",
    );
  });

  it("describes only effective capability", () => {
    const model = catalogModel("a/b", {
      declaredCapabilities: {
        inputModalities: ["text", "image"],
        outputModalities: ["text"],
        toolCalls: true,
        reasoning: true,
      },
      effectiveCapabilities: {
        inputModalities: ["text"],
        outputModalities: ["text"],
        toolCalls: false,
        reasoning: false,
      },
    });
    assert.match(capabilityLine(model), /no tools/);
    assert.match(capabilityLine(model), /in text$/);
  });
});

describe("model identity distinctions", () => {
  /**
   * Four cases the runtime can publish, all of which must survive the UI:
   *
   * ```text
   * A  configured == effective, no attempt
   * B  configured != effective, no attempt
   * C  configured == effective, attempt frozen elsewhere
   * D  configured, effective, and attempt all different
   * ```
   */
  function running(model: string) {
    return {
      attemptId: "a1",
      phase: { type: "running" as const },
      turn: 1,
      model: attemptModel(model),
      foreground: [],
    };
  }

  it("A · compresses configured and effective when they agree", () => {
    const rendered = lines(selector()).join("\n");
    assert.match(rendered, /configured · effective {2}alpha\/model-a/);
    // With one unambiguous meaning, one unambiguous marker.
    assert.match(rendered, /alpha\/model-a current/);
    assert.ok(!/beta\/model-b current/.test(rendered));
  });

  it("B · shows configured and effective separately when they differ", () => {
    const rendered = lines(
      selector({
        sessionModel: {
          ...sessionModel("beta/model-b"),
          configured: { model: "alpha/model-a" },
        },
      }),
    ).join("\n");
    assert.match(rendered, /configured {2}alpha\/model-a/);
    assert.match(rendered, /effective {3}beta\/model-b/);
    // Neither row may claim the ambiguous label.
    assert.ok(!/ current/.test(rendered));
    assert.match(rendered, /alpha\/model-a configured/);
    assert.match(rendered, /beta\/model-b effective/);
  });

  it("C · shows the attempt's frozen model next to an agreeing session", () => {
    const rendered = lines(
      selector({
        sessionModel: sessionModel("beta/model-b"),
        attempt: running("alpha/model-a"),
      }),
    ).join("\n");
    assert.match(rendered, /configured · effective {2}beta\/model-b/);
    assert.match(
      rendered,
      /attempt {5}alpha\/model-a · frozen at admission; a change applies to the next attempt/,
    );
    assert.match(rendered, /alpha\/model-a attempt/);
    assert.match(rendered, /beta\/model-b configured · effective/);
  });

  it("D · keeps all three identities when every one of them differs", () => {
    const rendered = lines(
      selector({
        sessionModel: {
          ...sessionModel("beta/model-b"),
          configured: { model: "alpha/model-a" },
        },
        attempt: running("beta/sonnet-x"),
      }),
    ).join("\n");
    assert.match(rendered, /configured {2}alpha\/model-a/);
    assert.match(rendered, /effective {3}beta\/model-b/);
    assert.match(rendered, /attempt {5}beta\/sonnet-x · frozen at admission/);
    // Each row is labelled with the role it actually holds, and no row is
    // labelled with a role it does not.
    assert.match(rendered, /alpha\/model-a configured/);
    assert.match(rendered, /beta\/model-b effective/);
    assert.match(rendered, /beta\/sonnet-x attempt/);
    assert.ok(!/ current/.test(rendered));
  });

  it("reports a settled attempt as settled rather than as pending guidance", () => {
    const rendered = lines(
      selector({
        attempt: {
          attemptId: "a1",
          phase: {
            type: "settled",
            outcome: { type: "completed", finish_reason: { type: "stop" } },
          },
          turn: 1,
          model: attemptModel("alpha/model-a"),
          foreground: [],
        },
      }),
    ).join("\n");
    assert.ok(!/a change applies to the next attempt/.test(rendered));
    assert.match(rendered, /attempt {5}alpha\/model-a · frozen at admission \(settled\)/);
  });
});

describe("catalog reasoning is never presented as current configuration", () => {
  it("names the catalog default as the catalog's, and the session's as the session's", () => {
    const rendered = lines(
      selector({
        sessionModel: {
          ...sessionModel("alpha/model-a", {
            capabilities: {
              inputModalities: ["text"],
              outputModalities: ["text"],
              toolCalls: true,
              reasoning: true,
            },
            reasoningProfile: "low",
            reasoningEnabled: true,
          }),
          configured: { model: "alpha/model-a", reasoningProfile: "high" },
        },
      }),
    ).join("\n");
    // The catalog says its fallback is `medium`; the session asked for `high`
    // and the runtime resolved `low`. Three facts, three distinct statements.
    assert.match(rendered, /catalog default medium/);
    assert.match(rendered, /configured reasoning {2}profile high/);
    assert.match(rendered, /effective reasoning {3}on \(profile low\)/);
  });

  it("says a session configured nothing rather than borrowing the catalog default", () => {
    const rendered = lines(selector()).join("\n");
    assert.match(
      rendered,
      /configured reasoning {2}not configured \(the runtime decides\)/,
    );
    // The catalog default is still visible, still labelled as the catalog's.
    assert.match(rendered, /catalog default medium/);
  });

  it("invents no reasoning scale for a capable model with no profiles", () => {
    const rendered = lines(
      selector({
        sessionModel: sessionModel("alpha/model-a", {
          capabilities: {
            inputModalities: ["text"],
            outputModalities: ["text"],
            toolCalls: true,
            reasoning: true,
          },
          reasoningEnabled: true,
        }),
      }),
    ).join("\n");
    assert.match(
      rendered,
      /effective reasoning {3}on \(runtime default, no selectable profile\)/,
    );
    assert.ok(!/off\/low\/medium\/high/.test(rendered));
  });

  it("reports reasoning the runtime disabled as off, not as absent", () => {
    const rendered = lines(
      selector({
        sessionModel: sessionModel("alpha/model-a", {
          capabilities: {
            inputModalities: ["text"],
            outputModalities: ["text"],
            toolCalls: true,
            reasoning: true,
          },
          reasoningProfile: "low",
          reasoningEnabled: false,
        }),
      }),
    ).join("\n");
    assert.match(rendered, /effective reasoning {3}off \(profile low\)/);
  });
});

describe("keyboard selection", () => {
  it("navigates, selects, and cancels", () => {
    const component = selector();
    const chosen: string[] = [];
    let cancelled = 0;
    component.onSelect = (model) => chosen.push(model.model);
    component.onCancel = () => {
      cancelled += 1;
    };

    component.handleInput("[B");
    assert.equal(component.selectedModel()?.model, "beta/model-b");
    component.handleInput("[A");
    assert.equal(component.selectedModel()?.model, "alpha/model-a");
    component.handleInput("\r");
    assert.deepEqual(chosen, ["alpha/model-a"]);

    component.handleInput("");
    assert.equal(cancelled, 1);
  });

  it("selects nothing when nothing matches", () => {
    const component = selector();
    let selected = 0;
    component.onSelect = () => {
      selected += 1;
    };
    component.setQuery("zzzz");
    component.handleInput("\r");
    assert.equal(selected, 0);
  });

  it("resets the highlight when the filter changes", () => {
    const component = selector();
    component.handleInput("[B");
    assert.equal(component.selectedModel()?.model, "beta/model-b");
    component.setQuery("beta");
    assert.equal(component.selectedModel()?.model, "beta/model-b");
    component.setQuery("");
    assert.equal(component.selectedModel()?.model, "alpha/model-a");
  });

  it("shows the keyboard hints", () => {
    assert.ok(
      lines(selector()).some((line) =>
        line.includes("↑↓ navigate · Enter select · Esc close"),
      ),
    );
  });
});
