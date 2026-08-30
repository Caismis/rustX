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
  searchTerms,
} from "../src/ui/components/model-selector.ts";
import { PopupFrame } from "../src/ui/components/popup-frame.ts";
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

  /**
   * Issue #79 asks for fuzzy search over "model reference and useful display
   * metadata". Each case below matches on a `CatalogModelView` field the
   * runtime published, and the last one proves the corpus does not fabricate.
   */
  it("still matches the model reference", () => {
    const component = selector();
    component.setQuery("model-b");
    assert.deepEqual(
      component.visibleModels().map((model) => model.model),
      ["beta/model-b"],
    );
  });

  it("matches the published protocol, by label and by wire spelling", () => {
    for (const query of ["Messages", "anthropic_messages"]) {
      const component = selector();
      component.setQuery(query);
      assert.deepEqual(
        component.visibleModels().map((model) => model.model),
        ["beta/sonnet-x"],
        `query ${query}`,
      );
    }
    const responses = selector();
    responses.setQuery("Responses");
    assert.deepEqual(
      responses.visibleModels().map((model) => model.model),
      ["beta/model-b"],
    );
  });

  it("matches a published input modality", () => {
    const component = selector();
    component.setQuery("image");
    assert.deepEqual(
      component.visibleModels().map((model) => model.model),
      ["alpha/model-a"],
      "only the row whose catalog declares image input",
    );
  });

  it("matches a published reasoning profile id and the reasoning capability", () => {
    const profile = selector();
    profile.setQuery("high");
    assert.deepEqual(
      profile.visibleModels().map((model) => model.model),
      ["alpha/model-a"],
    );

    const capability = selector();
    capability.setQuery("reasoning");
    assert.deepEqual(
      capability.visibleModels().map((model) => model.model),
      ["alpha/model-a"],
      "a row without the capability publishes no reasoning term",
    );
  });

  it("matches the published tool capability", () => {
    const component = selector();
    component.setQuery("tools");
    assert.equal(component.visibleModels().length, CATALOG.length);
  });

  it("narrows rather than widens when tokens are combined", () => {
    const component = selector();
    component.setQuery("beta messages");
    assert.deepEqual(
      component.visibleModels().map((model) => model.model),
      ["beta/sonnet-x"],
      "every token must match a published term",
    );
  });

  it("never fabricates a match from metadata the catalog did not publish", () => {
    for (const query of ["image", "high", "reasoning"]) {
      const component = selector();
      component.setQuery(query);
      const matched = component.visibleModels().map((model) => model.model);
      assert.ok(
        !matched.includes("beta/model-b") && !matched.includes("beta/sonnet-x"),
        `${query} matched a row whose catalog never published it`,
      );
    }
  });

  it("searches only catalog-published facts, never session state", () => {
    // The corpus is built from the row. `configured` and `effective` are
    // facts about the session, so they are not searchable row metadata, and
    // no provider alias is invented from a model reference either.
    const terms = searchTerms(CATALOG[0]!);
    assert.deepEqual(terms, [
      "alpha/model-a",
      "openai_chat_completions",
      "Chat Completions",
      "text",
      "image",
      "tools",
      "reasoning",
      "low",
      "medium",
      "high",
      "200k",
      "8k",
    ]);
    for (const forbidden of ["configured", "effective", "current", "claude", "gpt"]) {
      assert.ok(!terms.includes(forbidden), `${forbidden} is not a catalog fact`);
    }
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

  it("uses pi-tui editing semantics inside the search field", () => {
    const component = selector();
    component.handleInput("a");
    component.handleInput("b");
    // Left arrow moves the cursor without changing the filtered catalog.
    component.handleInput("[D");
    component.handleInput("X");
    assert.equal(component.query, "aXb");
    // Backspace deletes at the cursor, rather than always deleting the last
    // character as the old selector did.
    component.handleInput("");
    assert.equal(component.query, "ab");

    // Forward delete is also delegated to Input.
    component.handleInput("[3~");
    assert.equal(component.query, "a");

    // Bracketed paste is accepted as one edit and still flows through the
    // catalog's published-fact filter.
    const pasted = selector();
    pasted.handleInput("[200~beta/model-b[201~");
    assert.equal(pasted.query, "beta/model-b");
    assert.equal(pasted.visibleModels()[0]?.model, "beta/model-b");

    // Pi's undo binding remains available to the same native input object.
    pasted.handleInput("");
    assert.equal(pasted.query, "");
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

describe("finite viewport", () => {
  // Issue #161 blocker regression: the selector must lay out its list inside
  // the finite body rows PopupFrame allocated, so a selection the user can
  // move to is always visible — never clipped away by the frame.
  // Budget 16 is a realistic constrained popup: a 24-row terminal at the
  // selector's 70% overlay height.
  const manyModels = Array.from({ length: 12 }, (_, index) =>
    catalogModel(`prov/model-${index}`),
  );

  function framedSelector(budget = 16) {
    const component = new ModelSelector({
      models: manyModels,
      sessionModel: sessionModel("prov/model-0"),
    });
    const frame = new PopupFrame(component);
    frame.setViewportHeight(budget);
    return { component, frame };
  }

  function interiorRows(frame: PopupFrame): string[] {
    return frame
      .render(80)
      .map(plainText)
      .filter((line) => line.startsWith("│"));
  }

  it("keeps the selected model visible when selection moves beyond the first viewport", () => {
    const { component, frame } = framedSelector();
    const chosen: string[] = [];
    component.onSelect = (model) => chosen.push(model.model);

    // The initial window cannot hold all twelve models at this budget.
    const initial = interiorRows(frame).join("\n");
    assert.doesNotMatch(initial, /model-11/);

    // Move the logical selection well beyond the first viewport.
    for (let index = 0; index < 7; index += 1) component.handleInput("\u001b[B");
    assert.equal(component.selectedModel()?.model, "prov/model-7");

    const rows = interiorRows(frame);
    const markerRow = rows.find((line) => line.includes("❯"));
    assert.ok(markerRow?.includes("prov/model-7"), "the selected row is visible and marked");
    // The selected entry keeps its detail rows when the budget allows them.
    assert.ok(
      rows.some((line) => line.includes("catalog reasoning")),
      "the selected entry's detail rows render",
    );

    // Enter selects exactly the visible, marked item.
    component.handleInput("\r");
    assert.deepEqual(chosen, ["prov/model-7"]);

    // Moving back to the beginning shows the first model again.
    for (let index = 0; index < 7; index += 1) component.handleInput("\u001b[A");
    assert.equal(component.selectedModel()?.model, "prov/model-0");
    const back = interiorRows(frame);
    assert.ok(
      back.find((line) => line.includes("❯"))?.includes("prov/model-0"),
      "selection at the start of the list is visible and marked",
    );
  });

  it("yields the subordinate context block before the selected entry", () => {
    // Body rows: frame budget 10 → 4 body rows (6 chrome). The selected entry
    // alone costs 3; the configured/effective/reasoning context must yield.
    const { component, frame } = framedSelector(10);
    for (let index = 0; index < 5; index += 1) component.handleInput("\u001b[B");
    const rows = interiorRows(frame);
    assert.ok(
      rows.find((line) => line.includes("❯"))?.includes("prov/model-5"),
      "the selected model is still visible",
    );
    assert.ok(
      rows.every((line) => !line.includes("configured reasoning")),
      "subordinate context yields under constrained height",
    );
    // The interactive search header is required context and never yields.
    assert.ok(rows.some((line) => line.includes("Search:")));
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

  it("shows the keyboard hints inside the popup frame", () => {
    const framed = new PopupFrame(selector()).render(80).map(plainText);
    assert.ok(
      framed.some((line) =>
        line.includes("↑↓ navigate · Enter select · Esc close"),
      ),
    );
    // The hint is contained: it is not the popup's outer boundary row.
    const hint = framed.findIndex((line) => line.includes("↑↓ navigate"));
    assert.ok(hint > 0 && hint < framed.length - 1);
  });
});
