import assert from "node:assert/strict";
import { test } from "node:test";
import type { DeletionRecord, SessionDeleteResult, SessionDeletionRequest } from "../src/index.ts";
const record: DeletionRecord = { session_id: "session-1", target_revision: "a".repeat(64), scopes: [{ kind: "node", node_id: "node-1", conversation_id: "conversation-1" }, { kind: "child", conversation_id: "child-1", parent_conversation: "conversation-1" }], phase: "cleanup_pending" };
const variants = {
  preview: { status: "preview", preview: { session_id: record.session_id, name: null, target_revision: record.target_revision, scopes: record.scopes } },
  deleted: { status: "deleted", session_id: record.session_id },
  stale: { status: "stale", session_id: record.session_id, actual_revision: "b".repeat(64) },
  blocked: { status: "blocked", session_id: record.session_id, reason: { kind: "in_use" } },
  committed_cleanup_pending: { status: "committed_cleanup_pending", record, detail: null },
  committed_durability_uncertain: { status: "committed_durability_uncertain", record, detail: "directory barrier failed" },
  not_found: { status: "not_found", session_id: "unknown" },
} satisfies Record<SessionDeleteResult["status"], SessionDeleteResult>;
function classify(result: SessionDeleteResult): string {
  switch (result.status) {
    case "preview": return result.preview.target_revision;
    case "deleted": case "not_found": return result.session_id;
    case "stale": return result.actual_revision;
    case "blocked": return result.reason.kind;
    case "committed_cleanup_pending": case "committed_durability_uncertain": return result.record.phase;
    default: { const exhaustive: never = result; return exhaustive; }
  }
}
for (const result of Object.values(variants)) test(`deletion ${result.status} is typed and serializable`, () => {
  assert.deepEqual(JSON.parse(JSON.stringify(result)), result);
  assert.equal(typeof classify(result), "string");
});
test("requests carry native identity and revision only", () => {
  const requests: SessionDeletionRequest[] = [
    { method: "session_delete_preview", id: 1, session_id: record.session_id },
    { method: "session_delete", id: 2, session_id: record.session_id, expected_target_revision: record.target_revision },
    { method: "session_delete_recover", id: 3, session_id: record.session_id },
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
    { kind: "workspace", resources: ["workflow:one"] },
    { kind: "invalid_ownership", detail: "ambiguous owner" },
  ] satisfies import("../src/index.ts").DeletionBlocker[];
  assert.equal(new Set(reasons.map((r) => r.kind)).size, 4);
  const final: SessionDeleteResult = { status: "committed_durability_uncertain", record: { ...record, phase: "deleted" }, detail: "final directory barrier failed" };
  assert.equal(final.status, "committed_durability_uncertain");
});
