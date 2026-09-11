/** Native Session deletion contract (Runtime Client protocol 28).
 * Rust owns scope, durability and recovery. Callers send identity + revision,
 * never paths or a list of resources to remove.
 */
export type DeletionScope =
  | { kind: "node"; node_id: string; conversation_id: string }
  | { kind: "child"; conversation_id: string; parent_conversation: string };
export interface SessionDeletePreview {
  session_id: string;
  name: string | null;
  target_revision: string;
  scopes: DeletionScope[];
}
export type DeletionBlocker =
  | { kind: "current_session" }
  | { kind: "in_use" }
  | { kind: "workspace"; resources: string[] }
  | { kind: "invalid_ownership"; detail: string };
export type DeletionPhase = "cleanup_pending" | "deleted";
export interface DeletionRecord {
  session_id: string;
  target_revision: string;
  scopes: DeletionScope[];
  phase: DeletionPhase;
}
export type SessionDeleteResult =
  | { status: "preview"; preview: SessionDeletePreview }
  | { status: "deleted"; session_id: string }
  | { status: "stale"; session_id: string; actual_revision: string }
  | { status: "blocked"; session_id: string; reason: DeletionBlocker }
  | { status: "committed_cleanup_pending"; record: DeletionRecord; detail: string | null }
  | { status: "committed_durability_uncertain"; record: DeletionRecord; detail: string }
  | { status: "not_found"; session_id: string };
export type SessionDeletionRequest =
  | { method: "session_delete_preview"; id: number; session_id: string }
  | { method: "session_delete"; id: number; session_id: string; expected_target_revision: string }
  | { method: "session_delete_recover"; id: number; session_id: string };
export type SessionDeletionResponse = { type: "session_deletion"; result: SessionDeleteResult };
