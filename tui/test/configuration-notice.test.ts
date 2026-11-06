/**
 * Issue #518: a committed Session configuration change is one concise
 * Codex-style info notice in the transcript, rendered identically from the
 * live event and from a paged durable row; the footer carries only transient
 * status. Rendering is proven from strings.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { mergeTranscriptPage, reduce } from "../src/presentation/projection.ts";
import { renderFooter } from "../src/ui/components/status.ts";
import { plainText } from "../src/ui/theme.ts";
import { runtimeCursor } from "./support/fixtures.ts";
import { stateOf, transcriptString } from "./support/render.ts";

const none = { names: [], total: 0 };
const item = {
  type: "configuration_applied" as const,
  event_id: "evt_1",
  timestamp: "2026-01-01T00:00:00Z",
  change: {
    reason: "source" as const, previous_binding_revision: "1", binding_revision: "2",
    domains: ["instructions" as const, "tools" as const],
    tools: { added: { names: ["fetch", "search"], total: 18 }, removed: { names: ["read"], total: 1 }, changed: none },
    skills: { added: none, removed: none, changed: none },
    instructions_bytes: "14", catalog_bytes: "2",
  },
  instructions: { conversation_id: "conv", source: { type: "configuration_instructions" as const, journal_sequence: 7 }, representation: "utf8_text" as const, version: 1, content_revision: 1, readable_end: 14 },
  catalog: { conversation_id: "conv", source: { type: "configuration_catalog" as const, journal_sequence: 7 }, representation: "utf8_text" as const, version: 1, content_revision: 1, readable_end: 2 },
};
const expected = [
  "• Configuration applied · instructions, tools",
  "  Tools added: fetch, search +16 more",
  "  Tools removed: read",
].join("\n");

describe("configuration change notice", () => {
  it("renders one concise notice from the live event and the paged row alike", () => {
    const live = reduce(stateOf(), { cursor: runtimeCursor(1), event: { type: "configuration_applied", item, transcript_cursor: "7" } });
    assert.equal(transcriptString(live), expected);
    const paged = mergeTranscriptPage(stateOf(), { entries: [{ cursor: "7", item }] } as never);
    assert.equal(transcriptString(paged), expected);
  });

  it("keeps the footer status-only: no configuration text once applied", () => {
    const application = { scope: "s", sources: [{ kind: "user" as const }], version: "2", desired: { input_revision: "i", attempt: "2" }, units: { instructions: { status: "applied" as const } } };
    assert.doesNotMatch(plainText(renderFooter(stateOf(), "connected", 120, undefined, undefined, application)), /config/);
  });
});
