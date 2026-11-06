import type { AppServerClient } from '../client/app-server';
import { PreviewContentCache } from '../client/preview-content';
import { PREVIEW_POLICY } from '../client/preview-policy';
import type { WorkspaceAuthority } from '../workspaces/authority';
import type { WorkspaceAuthorityScope } from '../workspaces/host';
import type { WorkbenchTarget } from '../workspaces/workbench';
import type { FileTabView } from './components/WorkbenchFiles';
import type { TabId } from '../presentation/dockkit/contract/types';
import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import { sameEndpoint } from '../workspaces/endpoint';
interface RetainedWorkbench { views: Map<TabId, FileTabView>; current(): boolean }
/** App-private retention, never a path/permission registry. Every result is
 * still reauthorized by its source adapter before a body can restore it. */
export class WorkbenchPreviewRetention {
  private readonly records = new Map<string, RetainedWorkbench>();
  constructor(private readonly client: AppServerClient, private readonly authority: WorkspaceAuthority, readonly cache: PreviewContentCache) {}
  start() {
    const client = this.client.subscribe(this.invalidate), host = this.authority.subscribe(this.invalidate);
    return () => { client(); host(); for (const record of this.records.values()) this.dispose(record); this.records.clear(); };
  }
  private dispose(record: RetainedWorkbench) { for (const view of record.views.values()) view.preview?.dispose(); record.views.clear(); }
  private invalidate = () => { for (const [key, record] of this.records) if (!record.current()) { this.dispose(record); this.records.delete(key); } };
  acquire(scope: WorkspaceAuthorityScope | undefined, target: WorkbenchTarget | undefined): RetainedWorkbench {
    this.invalidate();
    const state = this.client.getSnapshot(), generation = state.generation, revision = state.authorityRevision, hostRevision = this.authority.getRevision();
    const observed = this.authority.capture();
    const observation = scope && observed?.scope.authorityId === scope.authorityId && sameEndpoint(observed.scope.endpoint, scope.endpoint) ? observed : undefined;
    const captured = target && structuredClone(target);
    const attachment = captured && 'session_id' in captured ? state.views[captured.session_id]?.target : undefined;
    const current = () => {
      const now = this.client.getSnapshot();
      if (!observation?.current() || !captured || generation !== now.generation || revision !== now.authorityRevision || hostRevision !== this.authority.getRevision()) return false;
      if ('workspace_id' in captured) return !!this.authority.getCatalog()?.workspaces.some(row => row.id === captured.workspace_id);
      const view = now.views[captured.session_id];
      return now.connection === 'connected' && !!attachment && sameTarget(view?.target, attachment) && !view?.deleting && view?.summary?.active_node === captured.active_node;
    };
    const key = JSON.stringify([generation, revision, hostRevision, scope?.authorityId, scope?.endpoint, captured]);
    const retained = this.records.get(key);
    if (retained) { this.records.delete(key); this.records.set(key, retained); return retained; }
    const record = { current, views: new Map<TabId, FileTabView>() };
    if (!current()) return record;
    if (this.records.size >= PREVIEW_POLICY.retainedSessions) { const oldest = this.records.entries().next().value!; this.dispose(oldest[1]); this.records.delete(oldest[0]); }
    this.records.set(key, record); return record;
  }
}
