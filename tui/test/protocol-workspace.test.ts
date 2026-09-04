/**
 * Cross-language Runtime Client workspace wire-contract regression (v15).
 *
 * The fixtures under `tests/fixtures/runtime-client/` are the shared
 * contract between the Rust projection and this TypeScript mirror: a Rust
 * wire-contract test serializes `RuntimeClientSubagentWorkspace` and asserts
 * byte equality with each fixture, while this suite parses the same bytes and
 * asserts deep equality with values typed as the mirror declarations. If the
 * Rust projection drifted back to the pre-#187 flat `workspace`/`isolated`
 * schema, or this mirror did, one side fails — the typecheck pins the
 * declaration, the deep equality pins the runtime bytes. The Issue #187
 * workspace authority shape is carried into v15 unchanged while the
 * Runtime Client gains the explicit disposal operation.
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

import type { RuntimeClientSubagentWorkspace } from "../src/protocol/types.ts";

function fixture(name: string): unknown {
  return JSON.parse(
    readFileSync(
      new URL(`../../tests/fixtures/runtime-client/${name}`, import.meta.url),
      "utf8",
    ),
  );
}

describe("Runtime Client v15 workspace wire contract", () => {
  it("shared isolation matches the Rust fixture", () => {
    const expected: RuntimeClientSubagentWorkspace = {
      logical_workspace: "/repo",
      isolation: { type: "shared" },
    };
    assert.deepEqual(fixture("workspace-shared-v15.json"), expected);
  });

  it("isolated subdirectory with retained handoff matches the Rust fixture", () => {
    const expected: RuntimeClientSubagentWorkspace = {
      logical_workspace: "/runtime-root/worktrees/subagent-1/backend",
      isolation: {
        type: "git_worktree",
        source_repository_root: "/repo",
        repository_relative_workspace: "backend",
        physical_worktree_root: "/runtime-root/worktrees/subagent-1",
        base_commit: "0123456789abcdef0123456789abcdef01234567",
        branch: "rustx/subagent-1",
        parent_had_uncommitted_changes: true,
      },
      handoff: {
        logical_workspace: "/runtime-root/worktrees/subagent-1/backend",
        physical_worktree_root: "/runtime-root/worktrees/subagent-1",
        branch: "rustx/subagent-1",
        base_commit: "0123456789abcdef0123456789abcdef01234567",
        head_commit: "89abcdef012345670123456789abcdef01234567",
        dirty: false,
      },
    };
    assert.deepEqual(fixture("workspace-git-worktree-v15.json"), expected);
  });
});
