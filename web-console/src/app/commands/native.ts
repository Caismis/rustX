import type { CompletedResponseView, AttachmentTarget, MethodResult, SessionUserMessageBoundary, UserInputBlock } from '../../../../protocol/app-server/v12';
import { AppServerClient, sameTarget } from '../../client/app-server';
import { activeAttempt, lineageSwitchSafe } from '../../bindings/projection';

/** A UI continuation fence, never a cancellation token for server mutations. */
export class NavigationEpoch {
  private epoch = 0;
  invalidate() { this.epoch++; }
  capture() { const epoch = this.epoch; return () => this.epoch === epoch; }
}
export interface HistoricalSelection {
  target: AttachmentTarget;
  nodeId: string;
  boundary: SessionUserMessageBoundary | CompletedResponseView;
}
export type HistoryAction = 'fork' | 'branch' | 'retry';
/** Shared by the sidebar and /new. Only a current gesture may open the result. */
export async function createSession(client: AppServerClient, cwd: string, navigationCurrent: () => boolean) {
  const generation = client.getSnapshot().generation;
  const current = () => navigationCurrent() && client.getSnapshot().generation === generation && client.getSnapshot().connection === 'connected';
  if (!current()) return;
  const result = await client.request({ method: 'session/create', params: { settings: { cwd } } }, 'session_transition');
  if (!current()) return;
  if (result.durability_diagnostic) throw new Error(`Session ${result.session.id} committed with durability uncertainty: ${result.durability_diagnostic}. Inspect Sessions; do not repeat creation.`);
  await client.attach(result.session.id, result.session.active_node, current);
  if (!current()) return;
  const target = client.target(result.session.id);
  if (target.conversation_id !== result.session.active_conversation_id) throw new Error('Created Session attached a different Conversation.');
  await client.listSessions();
  if (current() && sameTarget(client.getSnapshot().views[result.session.id]?.target, target)) return { session: result.session, content: [] as UserInputBlock[] };
}
export class CommandSession {
  readonly target: AttachmentTarget;
  private readonly generation: number;
  constructor(readonly client: AppServerClient, readonly sessionId: string, private readonly navigationCurrent: () => boolean) {
    this.target = client.target(sessionId);
    this.generation = client.getSnapshot().generation;
  }
  current = () => {
    const state = this.client.getSnapshot(), view = state.views[this.sessionId];
    return this.navigationCurrent() && state.connection === 'connected' && state.generation === this.generation
      && !view?.deleting && view?.attachment === 'attached' && view.attachmentIntent === 'wanted' && sameTarget(view.target, this.target);
  };
  private requireCurrent() { if (!this.current()) throw new Error('Obsolete command view. Inspect current authoritative state.'); }
  async models() {
    this.requireCurrent();
    const [current, catalog] = await Promise.all([
      this.client.request({ method: 'settings/model', params: { target: this.target } }, 'model'),
      this.client.request({ method: 'settings/models', params: { target: this.target } }, 'models'),
    ]);
    this.requireCurrent();
    if (this.client.getSnapshot().views[this.sessionId]?.modelMutation) await this.client.repairAgentModel(this.sessionId);
    this.requireCurrent(); return { current: current.model, catalog: catalog.catalog };
  }
  async setModel(model: string, reasoningProfile?: string) {
    this.requireCurrent();
    // Pick an exact catalog identity. Changing model resets model-specific overrides
    // to its native defaults; no provider inference or configuration editor.
    await this.client.setAgentModel(this.sessionId, { model, ...(reasoningProfile === undefined ? {} : { reasoningProfile }) });
    if (!this.current()) return;
    await this.client.repairAgentModel(this.sessionId);
    if (this.current()) return this.models();
  }
  async boundaries(offset = 0) {
    this.requireCurrent();
    const page = await this.client.request({ method: 'session/boundaries', params: { target: this.target, offset, limit: 32 } }, 'boundaries');
    let nodeId: string | undefined;
    let next: number | null | undefined = 0;
    // Match the attached Conversation, never the catalog's mutable default node.
    while (next != null && !nodeId) {
      this.requireCurrent();
      const tree: Extract<MethodResult, { type: 'tree' }> = await this.client.request({ method: 'session/tree', params: { session_id: this.sessionId, offset: next, limit: 32 } }, 'tree');
      nodeId = tree.nodes.find(node => node.conversation_id === this.target.conversation_id)?.id;
      next = tree.next_offset;
    }
    this.requireCurrent();
    if (!nodeId) throw new Error('Attached Conversation is absent from the native Session tree.');
    this.client.rememberNode(this.target, nodeId);
    return { selections: page.boundaries.map(boundary => ({ target: this.target, nodeId: nodeId!, boundary })), nextOffset: page.next_offset };
  }
  async responseSelection(response: CompletedResponseView): Promise<HistoricalSelection> {
    let offset: number | null | undefined = 0;
    while (offset != null) {
      const page = await this.tree(offset);
      const node = page.nodes.find(node => node.conversation_id === this.target.conversation_id);
      if (node) return { target: this.target, nodeId: node.id, boundary: response };
      offset = page.next_offset;
    }
    throw new Error('Attached Conversation is absent from the native Session tree.');
  }
  async transition(action: HistoryAction, selection: HistoricalSelection) {
    this.requireCurrent();
    if (!sameTarget(selection.target, this.target)) throw new Error('Historical selection belongs to another attachment.');
    if (action !== 'fork' && !lineageSwitchSafe(this.client.getSnapshot().views[this.sessionId])) throw new Error('Wait for unresolved requests, accepted inbound and the current attempt to settle before switching lineage.');
    const response = 'closing_message_id' in selection.boundary ? selection.boundary : undefined;
    const boundary = response ? (action === 'retry' ? response.retry_message_id : response.closing_message_id) : ('message' in selection.boundary ? selection.boundary.message.id : undefined);
    if (!boundary) throw new Error('No native replay input for this response.');
    const params = { side: response && action !== 'retry' ? 'after' as const : 'before' as const, session_id: this.sessionId, node_id: selection.nodeId, surface_revision: selection.boundary.surface_revision, boundary };
    // Fork copies native settings: authorize the source before creating a child,
    // even when its existing attachment needs no new admission.
    if (action === 'fork' && !await this.client.admitAttachment(this.sessionId, this.current)) return;
    this.requireCurrent();
    // Never substitute a newer revision, retry on refusal, or copy browser history.
    const result = await this.client.request(action === 'fork' ? { method: 'session/fork', params } : { method: 'session/branch', params }, 'session_transition');
    if (!this.current()) return;
    if (result.durability_diagnostic) throw new Error(`Lineage committed with durability uncertainty: ${result.durability_diagnostic}. Inspect Sessions/tree; do not repeat the mutation.`);
    const session = result.session;
    if (action !== 'fork' && !lineageSwitchSafe(this.client.getSnapshot().views[this.sessionId])) throw new Error(`Branch ${session.active_node} committed, but the source now has unresolved inbound or accepted work. Open it from Session tree after execution settles; do not repeat the branch.`);
    // The manager permits one resident Conversation per Session. Switch only after
    // the branch exists. A lost branch-switch/attach response also stops this sequence.
    if (action !== 'fork') await this.client.switchNode(this.sessionId, session.active_node);
    const continuing = () => this.navigationCurrent() && this.client.getSnapshot().generation === this.generation;
    if (!continuing()) return;
    await this.client.attach(session.id, session.active_node, continuing);
    if (!continuing()) return;
    const target = this.client.target(session.id);
    if (target.conversation_id !== session.active_conversation_id) throw new Error('Transition attached a different Conversation; execution refused.');
    if (action === 'retry') {
      if (!result.editor_content?.length) throw new Error('Branch has no native editor input; execution refused.');
      // Native cut excludes the selected User message. Submit the native returned
      // input exactly once, including #319 receipts, to the new Conversation.
      await this.client.sendContent(session.id, result.editor_content);
      if (!continuing() || !sameTarget(this.client.getSnapshot().views[session.id]?.target, target)) return;
    }
    await this.client.listSessions();
    if (!continuing() || !sameTarget(this.client.getSnapshot().views[session.id]?.target, target)) return;
    return { session, content: action === 'retry' ? [] : result.editor_content ?? [] };
  }
  async compact() {
    this.requireCurrent();
    // Native manual maintenance may own the Conversation before pending inbound
    // is adopted. Pending input is not a compaction rejection condition.
    if (activeAttempt(this.client.getSnapshot().views[this.sessionId]?.snapshot)) throw new Error('Wait for the current attempt before compacting.');
    await this.client.request({ method: 'context/compact', params: { target: this.target } }, 'context');
    if (this.current()) await this.client.refresh(this.sessionId);
  }
  async tools() {
    this.requireCurrent();
    const result = await this.client.request({ method: 'resources/read', params: { target: this.target } }, 'capabilities');
    this.requireCurrent(); return result.capabilities;
  }
  async tree(offset = 0) {
    this.requireCurrent();
    const result = await this.client.request({ method: 'session/tree', params: { session_id: this.sessionId, offset, limit: 32 } }, 'tree');
    this.requireCurrent();
    const attached = result.nodes.find(node => node.conversation_id === this.target.conversation_id);
    if (attached) this.client.rememberNode(this.target, attached.id);
    return result;
  }
  async openNode(nodeId: string, conversationId: string) {
    this.requireCurrent();
    if (conversationId !== this.target.conversation_id) {
      if (!lineageSwitchSafe(this.client.getSnapshot().views[this.sessionId])) throw new Error('Wait for unresolved requests, accepted inbound and the current attempt to settle before switching lineage.');
      await this.client.switchNode(this.sessionId, nodeId);
      if (!this.navigationCurrent() || this.client.getSnapshot().generation !== this.generation) return;
      await this.client.attach(this.sessionId, nodeId, () => this.navigationCurrent() && this.client.getSnapshot().generation === this.generation);
    }
    if (!this.navigationCurrent() || this.client.getSnapshot().generation !== this.generation) return;
    if (this.client.target(this.sessionId).conversation_id !== conversationId) throw new Error('Different Conversation attached; inspect native state.');
    return true;
  }
}
