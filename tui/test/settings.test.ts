import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { it } from "node:test";
import { renderSettings } from "../src/commands/dispatcher.ts";
import { emptyPresentationState, replaceFromSnapshot } from "../src/presentation/projection.ts";
import { agentStatus, attemptModel, attemptView, runtimeCursor, sessionModel, snapshot, temporalSection } from "./support/fixtures.ts";

it("CFG3 reconnect reconstructs published and admitted generations without replay", () => {
  const native = snapshot({
    model: sessionModel("ses_5f9678b9-a001-7a5a-ba04-f2df4d59b899"),
    resources: { inspection: { main: null, agents: {}, workflows: {}, sources: {}, skills: [], skill_diagnostics: [], definitions: [], resource_diagnostics: [] }, revision: "8", context_files: [], agent_profile: false },
    attempt: attemptView({ model: attemptModel("admitted-model"), execution_settings: { resource_revision: "7", approval_mode: "policy" } }),
    effective_plugins: { goal: null, todo: {}, agent_status: null },
  });
  const live = replaceFromSnapshot(native, runtimeCursor(50));
  const reconnect = replaceFromSnapshot(structuredClone(native), runtimeCursor(50));
  assert.deepEqual(reconnect, live);
  const rendered = renderSettings(reconnect);
  for (const text of ["ses_5f9678b9-a001-7a5a-ba04-f2df4d59b899", "admitted-model", "Published generation: 8", "/settings edits User/Workspace sources", "/session settings inspects Session state"])
    assert.ok(rendered.includes(text), text);
  assert.deepEqual(reconnect.effectivePlugins, native.effective_plugins);
  assert.equal(reconnect.attempt?.executionSettings?.resource_revision, "7");
});

it("CFG3 Plugins are native profile facts, independent of observations and current Todo state", () => {
  const absent = replaceFromSnapshot(snapshot({
    effective_plugins: { goal: null, agent_status: null, todo: null },
    statuses: [agentStatus({ status_message_id: "m1", sections: [temporalSection()] })],
  }), runtimeCursor(1));
  assert.equal(absent.statuses.length, 1);
  assert.equal(absent.effectivePlugins?.agent_status, null);
  const composed = replaceFromSnapshot(snapshot({
    effective_plugins: { goal: null, todo: {}, agent_status: { time: { enabled: true, timezone: "Asia/Shanghai" }, background: { enabled: false } } },
    statuses: [],
  }), runtimeCursor(2));
  assert.match(renderSettings(composed), /Asia\/Shanghai/);
  assert.deepEqual(composed.statuses, []);
  assert.equal(emptyPresentationState(sessionModel("test")).effectivePlugins, null);
});

it("CFG3 historical settings report partial evidence without inventing a live profile", () => {
  const state = replaceFromSnapshot(snapshot({ model: null, settings_evidence: "historical_partial", effective_plugins: null, attempt: attemptView({ model: null, execution_settings: null }) }), runtimeCursor(0));
  assert.match(renderSettings(state), /partial/);
  assert.equal(state.effectivePlugins, null);
});

it("CFG275 redacted native causes match the TypeScript wire unions", () => {
  const expected: {
    source: import("../src/protocol/app-server.ts").SourceResolutionFailure;
    workflow: import("../src/protocol/app-server.ts").WorkflowDependencyFailure;
    skills: import("../src/protocol/app-server.ts").SkillDiagnostic[];
  } = {
  "source": {
    "kind": "unavailable",
    "detail": {}
  },
  "workflow": {
    "kind": "materialization",
    "detail": {}
  },
  "skills": [
    {
      "kind": "source_root_invalid",
      "source": "workspace",
      "root": "/workspace/.agents/skills"
    },
    {
      "kind": "package_invalid",
      "source": "workspace",
      "package": "/workspace/.agents/skills/review",
      "cause": {
        "cause": "invalid_name",
        "directory": "/workspace/.agents/skills/review"
      }
    },
    {
      "kind": "package_invalid",
      "source": "workspace",
      "package": "/workspace/.agents/skills/review",
      "cause": {
        "cause": "name_directory_mismatch",
        "directory": "/workspace/.agents/skills/review"
      }
    },
    {
      "kind": "package_invalid",
      "source": "workspace",
      "package": "/workspace/.agents/skills/review",
      "cause": {
        "cause": "missing_skill_markdown",
        "directory": "/workspace/.agents/skills/review"
      }
    },
    {
      "kind": "package_invalid",
      "source": "workspace",
      "package": "/workspace/.agents/skills/review",
      "cause": {
        "cause": "skill_markdown_not_regular_file",
        "directory": "/workspace/.agents/skills/review"
      }
    },
    {
      "kind": "package_invalid",
      "source": "workspace",
      "package": "/workspace/.agents/skills/review",
      "cause": {
        "cause": "malformed_frontmatter",
        "directory": "/workspace/.agents/skills/review"
      }
    },
    {
      "kind": "package_invalid",
      "source": "workspace",
      "package": "/workspace/.agents/skills/review",
      "cause": {
        "cause": "invalid_description",
        "directory": "/workspace/.agents/skills/review"
      }
    },
    {
      "kind": "package_invalid",
      "source": "workspace",
      "package": "/workspace/.agents/skills/review",
      "cause": {
        "cause": "invalid_compatibility",
        "directory": "/workspace/.agents/skills/review"
      }
    },
    {
      "kind": "package_invalid",
      "source": "workspace",
      "package": "/workspace/.agents/skills/review",
      "cause": {
        "cause": "malformed_metadata",
        "directory": "/workspace/.agents/skills/review"
      }
    },
    {
      "kind": "package_invalid",
      "source": "workspace",
      "package": "/workspace/.agents/skills/review",
      "cause": {
        "cause": "invalid_dependency_declaration",
        "directory": "/workspace/.agents/skills/review"
      }
    },
    {
      "kind": "package_invalid",
      "source": "workspace",
      "package": "/workspace/.agents/skills/review",
      "cause": {
        "cause": "unsupported_symlink",
        "path": "/workspace/.agents/skills/review"
      }
    },
    {
      "kind": "package_invalid",
      "source": "workspace",
      "package": "/workspace/.agents/skills/review",
      "cause": {
        "cause": "unrepresentable_root",
        "path": "/workspace/.agents/skills/review"
      }
    },
    {
      "kind": "package_invalid",
      "source": "workspace",
      "package": "/workspace/.agents/skills/review",
      "cause": {
        "cause": "io",
        "path": "/workspace/.agents/skills/review"
      }
    }
  ]
};
  const fixture = JSON.parse(readFileSync(new URL("../../tests/fixtures/runtime-client/redacted-diagnostics-v34.json", import.meta.url), "utf8"));
  assert.deepEqual(fixture, expected);
});
