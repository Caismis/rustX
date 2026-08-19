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
  it("marks the configured model as current", () => {
    const rendered = lines(selector()).join("\n");
    assert.match(rendered, /alpha\/model-a current/);
    assert.ok(!/beta\/model-b current/.test(rendered));
  });

  it("shows published capability and window for the highlighted row", () => {
    const rendered = lines(selector()).join("\n");
    assert.match(rendered, /Chat Completions · 200k ctx · 8k out · tools · in text\/image/);
  });

  it("renders reasoning profiles exactly as the catalog published them", () => {
    assert.equal(
      reasoningLine(CATALOG[0]!),
      "reasoning: low medium high (default medium)",
    );
    // A model without reasoning is reported as unsupported, not as "off".
    assert.equal(reasoningLine(CATALOG[1]!), "reasoning: unsupported");
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
      "reasoning: supported, no selectable profile",
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
  it("shows configured and effective separately", () => {
    const rendered = lines(
      selector({
        sessionModel: {
          ...sessionModel("beta/model-b"),
          configured: { model: "beta/model-b" },
        },
      }),
    ).join("\n");
    assert.match(rendered, /configured beta\/model-b · effective beta\/model-b/);
  });

  it("says the running attempt keeps the model it froze", () => {
    const rendered = lines(
      selector({
        sessionModel: sessionModel("beta/model-b"),
        attempt: {
          attemptId: "a1",
          phase: { type: "running" },
          turn: 1,
          model: attemptModel("alpha/model-a"),
          foreground: [],
        },
      }),
    ).join("\n");
    assert.match(
      rendered,
      /the running attempt stays on alpha\/model-a; a change applies to the next attempt/,
    );
  });

  it("says nothing about a settled attempt", () => {
    const rendered = lines(
      selector({
        attempt: {
          attemptId: "a1",
          phase: { type: "settled", outcome: { type: "completed", finish_reason: { type: "stop" } } },
          turn: 1,
          model: attemptModel("alpha/model-a"),
          foreground: [],
        },
      }),
    ).join("\n");
    assert.ok(!/the running attempt stays on/.test(rendered));
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
