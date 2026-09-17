/** Disposable presentation projections; no Session or Workspace authority. */
export interface SessionNode {
  id: string; title: string; pendingInteraction?: 'approval' | 'plan-review' | 'question';
  running: boolean; runningSubagentCount: number;
  updatedAt: number; observation?: string;
}
export interface GroupNode {
  key: string; workspaceId: string | undefined; cwd: string | undefined; createdAt: number | undefined;
  label: string; sessionCount: number; expanded: boolean; containsCurrent: boolean; sessions: readonly SessionNode[];
}
export interface SearchResultNode extends Omit<SessionNode, 'updatedAt'> { workspace: string; snippet?: string }
