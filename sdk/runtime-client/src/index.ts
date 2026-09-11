/** Native Session deletion contract (Runtime Client protocol 28).
 * Rust owns scope, durability and recovery. Callers send identity + revision,
 * never paths or a list of resources to remove.
 */
export interface SessionDeletePreview {
  session_id: string;
  name: string | null;
  target_revision: string;
  owned_node_count: number;
  owned_conversation_count: number;
  owned_child_count: number;
}
export type DeletionBlocker =
  | { kind: "current_session" }
  | { kind: "in_use" }
  | { kind: "workspace"; resource_count: number }
  | { kind: "invalid_ownership" };
export type SessionDeleteResult =
  | { status: "preview"; preview: SessionDeletePreview }
  | { status: "deleted"; session_id: string }
  | { status: "stale"; session_id: string; actual_revision: string }
  | { status: "blocked"; session_id: string; reason: DeletionBlocker }
  | { status: "committed_cleanup_pending"; session_id: string }
  | { status: "committed_durability_uncertain"; session_id: string }
  | { status: "not_found"; session_id: string };
export type SessionDeletionRequest =
  | { method: "session_delete_preview"; id: number; session_id: string }
  | { method: "session_delete"; id: number; session_id: string; expected_target_revision: string }
  | { method: "session_delete_recover"; id: number; session_id: string };
export type SessionDeletionResponse = { type: "session_deletion"; result: SessionDeleteResult };
