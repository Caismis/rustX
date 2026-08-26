/**
 * The loaded-resources banner and the resource projection behind it.
 *
 * The contract under test is provenance, not layout: every name the banner
 * prints comes from a runtime-published field, a section the runtime said
 * nothing about is absent rather than empty, and a resource reload is a fact
 * the client folds on its own — not something it infers from a capability
 * revision that did not move.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { reduce } from "../src/presentation/projection.ts";
import { renderResourceBanner, displayPath } from "../src/ui/components/resources.ts";
import { plainText } from "../src/ui/theme.ts";
import { runtimeCursor } from "./support/fixtures.ts";
import { stateOf } from "./support/render.ts";

const banner = (state: Parameters<typeof renderResourceBanner>[0], workspace?: string) =>
  plainText(renderResourceBanner(state, workspace === undefined ? {} : { workspace }));

describe("loaded resources", () => {
  it("names the context files, Skills, and active Tools the runtime published", () => {
    const state = stateOf({
      resources: {
        revision: 3,
        context_files: [
          { path: "/home/dev/AGENTS.md", bytes: 12 },
          { path: "/work/project/AGENTS.md", bytes: 40 },
        ],
        agent_profile: false,
      },
    });
    const rendered = banner(state, "/work/project");

    assert.match(rendered, /^\[Context\]$/m);
    assert.match(rendered, /^ {2}\/home\/dev\/AGENTS\.md, AGENTS\.md$/m);
    assert.match(rendered, /^\[Skills\]$/m);
    assert.match(rendered, /^ {2}review$/m);
    assert.match(rendered, /^\[Tools\]$/m);
    // The active catalog, alphabetically, by the model-facing name.
    assert.match(rendered, /^ {2}bash, search$/m);
  });

  it("keeps context files in the runtime's own precedence order", () => {
    const state = stateOf({
      resources: {
        revision: 1,
        context_files: [
          { path: "/work/zeta/AGENTS.md", bytes: 1 },
          { path: "/work/alpha/AGENTS.md", bytes: 1 },
        ],
      },
    });
    // Root-most first, exactly as the runtime concatenated them: sorting
    // these would misreport which instruction wins.
    assert.match(banner(state), /zeta\/AGENTS\.md, \/work\/alpha\/AGENTS\.md/);
  });

  it("omits a section the runtime published nothing for", () => {
    const state = stateOf({
      capabilities: { revision: 1, tools: [], available_tools: [], skills: [] },
    });
    const rendered = banner(state);

    assert.ok(!rendered.includes("[Context]"), "no project instructions were loaded");
    assert.ok(!rendered.includes("[Skills]"));
    assert.ok(!rendered.includes("[Tools]"));
    assert.equal(rendered, "", "an empty banner is drawn as nothing at all");
  });

  it("reports a frozen agent profile as context", () => {
    const state = stateOf({
      resources: { revision: 2, context_files: [], agent_profile: true },
    });
    assert.match(banner(state), /\[Context\]\n {2}agent profile/);
  });

  it("folds a reload's whole generation from one event", () => {
    const before = stateOf({
      resources: { revision: 1, context_files: [], agent_profile: false },
    });
    const after = reduce(before, {
      cursor: runtimeCursor(before.cursor + 1),
      event: {
        type: "resource_generation_updated",
        capabilities: {
          ...before.capabilities,
          revision: before.capabilities.revision + 1,
          skills: [
            {
              id: "skill-generation",
              version_id: "skill-generation@1",
              name: "generation-skill",
              description: "published together",
              location: ".rustx/skills/generation/SKILL.md",
            },
          ],
        },
        resources: {
          revision: 2,
          context_files: [{ path: "/work/project/AGENTS.md", bytes: 9 }],
          agent_profile: false,
        },
      },
    });

    // One event, one cursor, both halves. There is no intermediate state in
    // which this client holds the new capability generation beside the
    // resource generation the same reload retired.
    assert.equal(after.resources.revision, 2);
    assert.equal(after.capabilities.revision, before.capabilities.revision + 1);
    assert.deepEqual(
      (after.capabilities.skills ?? []).map((skill) => skill.name),
      ["generation-skill"],
    );
    assert.match(banner(after, "/work/project"), /\[Context\]\n {2}AGENTS\.md/);
  });

  it("folds a resource-only reload without waiting for a capability revision", () => {
    const before = stateOf({
      resources: { revision: 1, context_files: [], agent_profile: false },
    });
    const after = reduce(before, {
      cursor: runtimeCursor(before.cursor + 1),
      event: {
        type: "resource_generation_updated",
        // A reload that only rewrote project instructions repeats the
        // capability view it composed against, unchanged revision and all.
        capabilities: before.capabilities,
        resources: {
          revision: 2,
          context_files: [{ path: "/work/project/AGENTS.md", bytes: 9 }],
          agent_profile: false,
        },
      },
    });

    assert.equal(after.resources.revision, 2);
    assert.match(banner(after, "/work/project"), /\[Context\]\n {2}AGENTS\.md/);
    assert.equal(after.capabilities.revision, before.capabilities.revision);
  });
});

describe("resource path display", () => {
  it("prefers the workspace-relative spelling", () => {
    assert.equal(displayPath("/work/project/docs/AGENTS.md", "/work/project"), "docs/AGENTS.md");
  });

  it("leaves a path outside the workspace absolute rather than inventing a base", () => {
    assert.equal(displayPath("/etc/rustx/AGENTS.md", "/work/project"), "/etc/rustx/AGENTS.md");
  });
});
