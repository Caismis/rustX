import type { AttachmentTarget } from '../../../protocol/app-server/v36';
import { sameTarget, type AppServerClient } from '../client/app-server';
import { FilePreviewCoordinator, samePreviewSource, type FilePreviewLease } from '../client/session-files';
import { PREVIEW_POLICY } from '../client/preview-policy';
import type { WorkspaceAuthority } from '../workspaces/authority';
import type { ProductHostWorkspaces } from '../workspaces/host';
import type { PreviewArtifact } from './components/ArtifactPreview';
import type { PreviewViewState } from '../presentation/right-panel/preview-view-state';

export interface PreviewScope { sessionId: string; generation: number; authorityRevision: number | undefined; hostRevision: number; target: AttachmentTarget }
export function samePreviewScope(a: PreviewScope, b: PreviewScope) {
  return a.sessionId === b.sessionId && a.generation === b.generation && a.authorityRevision === b.authorityRevision
    && a.hostRevision === b.hostRevision && sameTarget(a.target, b.target);
}
export { samePreviewSource } from '../client/session-files';
export interface PreviewTab { id: number; artifact: PreviewArtifact; pane: number; view: PreviewViewState }
export interface PreviewPane { id: number; selected: number }
export interface SessionPreviewWorkspace {
  scope: PreviewScope; tabs: PreviewTab[]; panes: PreviewPane[]; activePane: number;
  fullscreen: boolean; expanded: boolean; ratio: number;
}
export type PreviewProblem = 'tabs' | 'sessions' | 'unavailable';
export interface PreviewWorkspaceSnapshot {
  geometryEpoch: number; width: number; workspace?: SessionPreviewWorkspace; mode: 'collapsed' | 'preview' | 'inspector' | 'workbench';
  leases: ReadonlyMap<number, FilePreviewLease>; problem?: PreviewProblem; downloadError?: string;
}
export const splitBounds = (width: number) => {
  const usable = Math.max(1, width - PREVIEW_POLICY.dividerWidth);
  return { min: Math.max(PREVIEW_POLICY.minRatio, PREVIEW_POLICY.paneMinWidth / usable), max: Math.min(PREVIEW_POLICY.maxRatio, 1 - PREVIEW_POLICY.paneMinWidth / usable) };
};
export const splitFits = (width: number) => width >= PREVIEW_POLICY.paneMinWidth * 2 + PREVIEW_POLICY.dividerWidth;
export const clampSplit = (ratio: number, width: number) => {
  const { min, max } = splitBounds(width);
  return splitFits(width) ? Math.min(max, Math.max(min, Number.isFinite(ratio) ? ratio : .5)) : .5;
};

/** Browser-only logical owner. Disposal linearizes before publication of each mutation.
 * Inactive workspaces contain metadata only; leases are separate and never persisted. */
export class PreviewWorkspaceOwner {
  private sessions = new Map<string, SessionPreviewWorkspace>();
  private selected?: string;
  private mode: PreviewWorkspaceSnapshot['mode'] = 'collapsed';
  private occurrence = 0;
  private paneSequence = 0;
  private width = 0;
  private geometryEpoch = 0;
  private coordinator?: FilePreviewCoordinator;
  private resourceScope?: PreviewScope;
  private leases = new Map<number, FilePreviewLease>();
  private listeners = new Set<() => void>();
  private snapshot: PreviewWorkspaceSnapshot = { mode: 'collapsed', leases: new Map(), geometryEpoch: 0, width: 0 };
  private problem?: PreviewProblem;
  private downloadError?: string;
  constructor(private client: AppServerClient, private host: ProductHostWorkspaces, private authority: WorkspaceAuthority) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.snapshot;
  start() {
    const client = this.client.subscribe(this.invalidate), host = this.authority.subscribe(this.invalidate);
    this.invalidate(); this.publish();
    return () => { client(); host(); this.retireResources(); };
  }
  private scope(id: string): PreviewScope | undefined {
    const state = this.client.getSnapshot(), view = state.views[id];
    if (!view?.target || view.deleting || view.attachmentIntent !== 'wanted' || view.attachment !== 'attached' || state.connection !== 'connected') return;
    return { sessionId: id, generation: state.generation, authorityRevision: state.authorityRevision, hostRevision: this.authority.getRevision(), target: { ...view.target } };
  }
  private invalidate = () => {
    let changed = false;
    if (this.resourceScope) {
      const scope = this.scope(this.resourceScope.sessionId);
      if (!scope || !samePreviewScope(scope, this.resourceScope)) { this.retireResources(); changed = true; }
    }
    for (const [id, workspace] of this.sessions) {
      const scope = this.scope(id);
      if (!scope || !samePreviewScope(workspace.scope, scope)) {
        if (id === this.selected) this.retireResources();
        this.sessions.delete(id); changed = true;
      }
    }
    if (changed) { if (!this.current() && this.mode === 'preview') this.mode = 'collapsed'; this.publish(); }
  };
  private current() { return this.selected ? this.sessions.get(this.selected) : undefined; }
  private retireResources() {
    for (const lease of this.leases.values()) lease.dispose();
    this.leases.clear(); this.coordinator?.dispose(); this.coordinator = undefined; this.resourceScope = undefined;
  }
  private reconcile() {
    const workspace = this.current();
    const visible = workspace && this.mode === 'preview'
      ? workspace.panes.filter(pane => splitFits(this.width) || pane.id === workspace.activePane).map(pane => pane.selected) : [];
    for (const [id, lease] of this.leases) if (!visible.includes(id)) { lease.dispose(); this.leases.delete(id); }
    if (workspace && visible.length) {
      this.resourceScope = workspace.scope;
      this.coordinator ??= new FilePreviewCoordinator(this.client, workspace.scope.sessionId, this.host, this.authority);
      for (const id of visible) if (!this.leases.has(id)) {
        const tab = workspace.tabs.find(tab => tab.id === id)!;
        this.leases.set(id, this.coordinator.acquire(id, tab.artifact.source));
      }
    }
  }
  private invalidateGeometry() { this.width = 0; this.geometryEpoch++; }
  private publish(reconcile = true, layoutWidth?: number) {
    if (this.mode !== 'preview' || !this.current()) this.invalidateGeometry();
    if (reconcile) this.reconcile();
    this.snapshot = { geometryEpoch: this.geometryEpoch, width: layoutWidth ?? this.width, workspace: this.current(), mode: this.mode, leases: new Map(this.leases), problem: this.problem, downloadError: this.downloadError };
    this.listeners.forEach(listener => listener());
  }
  selectSession(id?: string) {
    if (this.selected === id) return;
    this.retireResources(); this.invalidateGeometry(); this.selected = id; this.problem = undefined; this.downloadError = undefined;
    this.invalidate();
    if (this.mode !== 'inspector') this.mode = this.current()?.expanded ? 'preview' : 'collapsed'; this.publish();
  }
  measure(width: number, epoch: number) {
    if (epoch !== this.geometryEpoch || this.mode !== 'preview' || !this.current()) return;
    const measured = Number.isFinite(width) && width > 0 ? width : 0;
    if (measured === this.width) {
      if (!measured && this.leases.size > 1) this.publish();
      return;
    }
    if (!measured) this.invalidateGeometry();
    else this.width = measured;
    this.publish();
  }
  openPreview(artifact: PreviewArtifact): number | undefined {
    this.invalidate();
    const scope = this.selected ? this.scope(this.selected) : undefined;
    if (!scope) { this.problem = 'unavailable'; this.publish(); return; }
    let workspace = this.current();
    if (!workspace) {
      if (this.sessions.size >= PREVIEW_POLICY.retainedSessions) { this.problem = 'sessions'; this.publish(); return; }
      workspace = { scope, tabs: [], panes: [], activePane: ++this.paneSequence, fullscreen: false, expanded: true, ratio: .5 };
      this.sessions.set(scope.sessionId, workspace);
    }
    const existing = workspace.tabs.find(tab => samePreviewSource(tab.artifact.source, artifact.source));
    if (!existing && workspace.tabs.length >= PREVIEW_POLICY.tabsPerSession) { this.problem = 'tabs'; this.publish(); return; }
    const tab: PreviewTab = existing ?? { id: ++this.occurrence, artifact: { ...artifact, source: structuredClone(artifact.source) }, pane: workspace.activePane, view: {} };
    workspace = { ...workspace, tabs: existing ? workspace.tabs : [...workspace.tabs, tab], expanded: true, activePane: tab.pane,
      panes: workspace.panes.length ? workspace.panes.map(pane => pane.id === tab.pane ? { ...pane, selected: tab.id } : pane) : [{ id: tab.pane, selected: tab.id }] };
    this.sessions.set(scope.sessionId, workspace); this.mode = 'preview'; this.problem = undefined; this.publish(); return tab.id;
  }
  selectTab(id: number) {
    const w = this.current(), tab = w?.tabs.find(tab => tab.id === id); if (!w || !tab) return;
    this.replace({ ...w, activePane: tab.pane, panes: w.panes.map(pane => pane.id === tab.pane ? { ...pane, selected: id } : pane) });
  }
  activatePane(id: number) { const w = this.current(); if (w && w.activePane !== id && w.panes.some(pane => pane.id === id)) this.replace({ ...w, activePane: id }); }
  private without(w: SessionPreviewWorkspace, id: number): SessionPreviewWorkspace {
    const tab = w.tabs.find(tab => tab.id === id)!;
    const siblings = w.tabs.filter(other => other.pane === tab.pane), index = siblings.indexOf(tab);
    const next = siblings[index + 1] ?? siblings[index - 1]; // right sibling first, then left
    const panes = w.panes.flatMap(pane => pane.id !== tab.pane || pane.selected !== id ? [pane] : next ? [{ ...pane, selected: next.id }] : []);
    return { ...w, tabs: w.tabs.filter(tab => tab.id !== id), panes, activePane: panes.some(pane => pane.id === w.activePane) ? w.activePane : panes[0]?.id ?? w.activePane };
  }
  closeTab(id: number) {
    const w = this.current(); if (!w?.tabs.some(tab => tab.id === id)) return;
    const next = this.without(w, id); this.problem = undefined;
    if (!next.tabs.length) { this.retireResources(); this.sessions.delete(w.scope.sessionId); this.mode = 'collapsed'; this.publish(); }
    else this.replace(next);
  }
  split() {
    const w = this.current(); if (!w || w.panes.length >= PREVIEW_POLICY.panes || w.tabs.length < 2 || !splitFits(this.width)) return;
    const selected = w.panes[0].selected, newPane = ++this.paneSequence;
    this.leases.get(selected)?.dispose(); this.leases.delete(selected);
    const next = this.without(w, selected);
    this.replace({ ...next, tabs: w.tabs.map(tab => tab.id === selected ? { ...tab, pane: newPane } : tab), panes: [...next.panes, { id: newPane, selected }], activePane: newPane, ratio: .5 });
  }
  move(id: number) {
    const w = this.current(), tab = w?.tabs.find(tab => tab.id === id);
    if (!w || !tab || w.panes.length !== 2) return;
    const other = w.panes.find(pane => pane.id !== tab.pane)!;
    this.leases.get(id)?.dispose(); this.leases.delete(id);
    const next = this.without(w, id);
    this.replace({ ...next, tabs: w.tabs.map(tab => tab.id === id ? { ...tab, pane: other.id } : tab), activePane: other.id,
      panes: next.panes.map(pane => pane.id === other.id ? { ...pane, selected: id } : pane) });
  }
  setRatio(ratio: number) { const w = this.current(); if (w && splitFits(this.width)) this.replace({ ...w, ratio: clampSplit(ratio, this.width) }); }
  updateView(id: number, patch: Partial<PreviewViewState>) {
    const w = this.current(); if (!w?.tabs.some(tab => tab.id === id)) return;
    // A body can flush its last scroll position while hidden, never after occurrence deletion.
    this.replace({ ...w, tabs: w.tabs.map(tab => tab.id === id ? { ...tab, view: { ...tab.view, ...patch } } : tab) });
  }
  toggleFullscreen(commitPresentation: (publish: () => void) => void) {
    const w = this.current(); if (!w) return;
    // Same keyed visible tree: request a synchronous layout commit, without
    // acquiring against the old coordinate system. Its layout effect measures
    // the new presentation before reconciling the existing leases.
    this.sessions.set(w.scope.sessionId, { ...w, fullscreen: !w.fullscreen });
    const layoutWidth = this.width;
    this.invalidateGeometry();
    try { commitPresentation(() => this.publish(false, layoutWidth)); }
    finally { if (!this.width) this.publish(); }
  }
  collapse() { const w = this.current(); if (w) this.sessions.set(w.scope.sessionId, { ...w, expanded: false }); this.mode = 'collapsed'; this.publish(); }
  reveal() { const w = this.current(); if (!w) return; this.mode = 'preview'; this.replace({ ...w, expanded: true }); }
  showWorkbench() { this.mode = 'workbench'; this.publish(); }
  toggleInspector() { this.mode = this.mode === 'inspector' ? this.current()?.expanded ? 'preview' : 'collapsed' : 'inspector'; this.publish(); }
  dismissProblem() { this.problem = undefined; this.downloadError = undefined; this.publish(); }
  async download(artifact: PreviewArtifact) {
    if (this.downloadError !== undefined) { this.downloadError = undefined; this.publish(); }
    const scope = this.selected ? this.scope(this.selected) : undefined;
    if (!scope) { this.problem = 'unavailable'; this.publish(); return; }
    this.resourceScope = scope;
    this.coordinator ??= new FilePreviewCoordinator(this.client, scope.sessionId, this.host, this.authority);
    const coordinator = this.coordinator;
    try { await coordinator.download(artifact.source, artifact.name, artifact.mimeType); }
    catch (error) { if (coordinator === this.coordinator) { this.downloadError = String(error); this.publish(); } }
  }
  private replace(workspace: SessionPreviewWorkspace) { this.sessions.set(workspace.scope.sessionId, workspace); this.publish(); }
}
