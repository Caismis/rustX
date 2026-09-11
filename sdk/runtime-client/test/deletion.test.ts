import assert from "node:assert/strict";
import { test } from "node:test";
import type { SessionDeleteResult, SessionDeletionRequest } from "../src/index.ts";
const target = { session_id: "session-1", target_revision: "a".repeat(64) };
const variants = {
  preview: { status: "preview", preview: { session_id: target.session_id, name: null, target_revision: target.target_revision, owned_node_count: 1, owned_conversation_count: 2, owned_child_count: 1 } },
  deleted: { status: "deleted", session_id: target.session_id },
  stale: { status: "stale", session_id: target.session_id, actual_revision: "b".repeat(64) },
  blocked: { status: "blocked", session_id: target.session_id, reason: { kind: "in_use" } },
  committed_cleanup_pending: { status: "committed_cleanup_pending", session_id: target.session_id },
  committed_durability_uncertain: { status: "committed_durability_uncertain", session_id: target.session_id },
  not_found: { status: "not_found", session_id: "unknown" },
} satisfies Record<SessionDeleteResult["status"], SessionDeleteResult>;
function classify(result: SessionDeleteResult): string {
  switch (result.status) {
    case "preview": return result.preview.target_revision;
    case "deleted": case "not_found": return result.session_id;
    case "stale": return result.actual_revision;
    case "blocked": return result.reason.kind;
    case "committed_cleanup_pending": case "committed_durability_uncertain": return result.session_id;
    default: { const exhaustive: never = result; return exhaustive; }
  }
}
for (const result of Object.values(variants)) test(`deletion ${result.status} is typed and serializable`, () => {
  assert.deepEqual(JSON.parse(JSON.stringify(result)), result);
  assert.equal(typeof classify(result), "string");
});
test("requests carry native identity and revision only", () => {
  const requests: SessionDeletionRequest[] = [
    { method: "session_delete_preview", id: 1, session_id: target.session_id },
    { method: "session_delete", id: 2, session_id: target.session_id, expected_target_revision: target.target_revision },
    { method: "session_delete_recover", id: 3, session_id: target.session_id },
  ];
  assert.equal(new Set(requests.map((r) => r.method)).size, 3);
});

test("Rust and TypeScript use the same complete result fixtures", async () => {
  const { readFile } = await import("node:fs/promises");
  const fixtures = JSON.parse(await readFile(new URL("./deletion-fixtures.json", import.meta.url), "utf8"));
  assert.deepEqual(fixtures, Object.values(variants));
});

test("all blocker reasons and finalization uncertainty remain discriminated", () => {
  const reasons = [
    { kind: "current_session" }, { kind: "in_use" },
    { kind: "workspace", resource_count: 1 },
    { kind: "invalid_ownership" },
  ] satisfies import("../src/index.ts").DeletionBlocker[];
  assert.equal(new Set(reasons.map((r) => r.kind)).size, 4);
  const final: SessionDeleteResult = { status: "committed_durability_uncertain", session_id: target.session_id };
  assert.equal(final.status, "committed_durability_uncertain");
});

// Compile-time guard against leaking native recovery authority into the wire.
function externalOnly(result: SessionDeleteResult): void {
  if (result.status === "preview") {
    // @ts-expect-error Native scopes are not part of the protocol DTO.
    result.preview.scopes;
  }
  if (result.status === "committed_cleanup_pending") {
    // @ts-expect-error Persisted cleanup records stay native.
    result.record;
  }
}
test("external DTOs do not expose cleanup authority", () => {
  externalOnly(variants.preview);
  externalOnly(variants.committed_cleanup_pending);
});
