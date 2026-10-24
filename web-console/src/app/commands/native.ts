import { parseObservedValue } from '../../../../protocol/app-server/domain-observation';
import { readTraceCollection, readTraceValue } from '../../client/trace-content';
import type { TraceFieldReader } from '../../client/trace';
import { readEditor } from '../../../../protocol/app-server/editor';
import type { CompletedResponseView, AttachmentTarget, SessionUserMessageBoundaryObservation, CanonicalContentLocator } from '../../../../protocol/app-server/v57';
import { AppServerClient } from '../../client/app-server';
import { sameTarget } from '../../../../protocol/app-server/attachment-identity';
import { selectSessionModel } from '../model-preference';
import { lineageSwitchSafe } from '../../bindings/projection';

export interface HistoricalSelection {
  target: AttachmentTarget;
  nodeId: string;
  boundary: SessionUserMessageBoundaryObservation | CompletedResponseView;
}
export type HistoryAction = 'fork' | 'branch' | 'retry';
/** The lineage actions a completed response offers in place. */
export type ResponseAction = Extract<HistoryAction, 'fork' | 'retry'>;
export class CommandSession {
  readonly target: AttachmentTarget;
  private readonly generation: number;
  private readonly admission: ReturnType<AppServerClient['getSnapshot']>['views'][string]['attachmentObservation'];
  constructor(readonly client: AppServerClient, readonly sessionId: string, private readonly navigationCurrent: () => boolean) {
    this.target = client.target(sessionId);
    this.admission = client.getSnapshot().views[sessionId].attachmentObservation;
    this.generation = client.getSnapshot().generation;
  }
  current = () => {
    const state = this.client.getSnapshot(), view = state.views[this.sessionId];
    return this.navigationCurrent() && state.connection === 'connected' && state.generation === this.generation
      && this.client.isAttachmentControlCurrent(this.sessionId, view?.attachmentObservation) && sameTarget(view?.target, this.target);
  };
  private requireCurrent() { if (!this.current()) throw new Error('Obsolete command view. Inspect current authoritative state.'); }
  async models() {
    this.requireCurrent();
    const [current, catalog] = await Promise.all([
      this.client.reads.modelRead(this.target,this.current),
      this.client.reads.modelCatalogRead(this.target,this.current),
    ]);
    this.requireCurrent();
    if (this.client.getSnapshot().views[this.sessionId]?.modelMutation) await this.client.repairAgentModel(this.sessionId);
    this.requireCurrent(); return { current, catalog: catalog.catalog, page:catalog.page };
  }
  async setModel(model: string, profile?: string) {
    this.requireCurrent();
    // Pick an exact catalog identity. Changing model resets model-specific overrides
    // to its native defaults; no provider inference or configuration editor.
    await selectSessionModel(this.client, this.sessionId, { model, ...(profile === undefined ? {} : { profile }) });
    if (!this.current()) return;
    await this.client.repairAgentModel(this.sessionId);
    if (this.current()) return this.models();
  }
  async boundaries(offset = 0) {
    this.requireCurrent();
    const page = await this.client.request({ method: 'session/boundaries', params: { target: this.target, offset, limit: 32 } }, 'boundaries');
    this.requireCurrent();
    const nodeId = this.admission!.nodeId;
    return { selections: page.boundaries.map(boundary => ({ target: this.target, nodeId: nodeId!, boundary })), nextOffset: page.next_offset };
  }
  async responseSelection(response: CompletedResponseView): Promise<HistoricalSelection> {
    this.requireCurrent();
    return { target: this.target, nodeId: this.admission!.nodeId, boundary: response };
  }
  async transition(action: HistoryAction, selection: HistoricalSelection) {
    this.requireCurrent();
    if (!sameTarget(selection.target, this.target)) throw new Error('Historical selection belongs to another attachment.');
    if (action !== 'fork' && !lineageSwitchSafe(this.client.getSnapshot().views[this.sessionId])) throw new Error('Wait for unresolved requests, accepted inbound and the current attempt to settle before switching lineage.');
    const response = 'closing_locator' in selection.boundary ? selection.boundary : undefined;
    const boundary: CanonicalContentLocator | undefined | null = response ? (action === 'retry' ? response.retry_locator : response.closing_locator) : ('locator' in selection.boundary ? selection.boundary.locator : undefined);
    if (!boundary) throw new Error('No native replay input for this response.');
    const params = { side: response && action !== 'retry' ? 'after' as const : 'before' as const, session_id: this.sessionId, node_id: selection.nodeId, surface_revision: selection.boundary.surface_revision, boundary };
    // Fork copies native settings: authorize the source before creating a child,
    // even when its existing attachment needs no new admission.
    const admitted = action === 'fork' ? await this.client.admitSessionOperation(this.sessionId, this.current) : this.current;
    if (!admitted || !(typeof admitted === 'function' ? admitted() : admitted.current())) return;
    this.requireCurrent();
    // Never substitute a newer revision, retry on refusal, or copy browser history.
    const result = await this.client.request(action === 'fork' ? { method: 'session/fork', params } : { method: 'session/branch', params }, 'session_transition', undefined, admitted);
    // Native acknowledgement remains observable after presentation supersession.
    if (this.client.getSnapshot().generation === this.generation) void this.client.listSessions().catch(() => {});
    if (!this.current()) return;
    if (result.durability_diagnostic) throw new Error(`Lineage committed with durability uncertainty: ${result.durability_diagnostic}. Inspect Sessions/tree; do not repeat the mutation.`);
    const session = result.session;
    if (action !== 'fork' && !lineageSwitchSafe(this.client.getSnapshot().views[this.sessionId])) throw new Error(`Branch ${session.active_node} committed, but the source now has unresolved inbound or accepted work. Open it from Session tree after execution settles; do not repeat the branch.`);
    // The manager permits one resident Conversation per Session. Switch only after
    // the branch exists. A lost branch-switch/attach response also stops this sequence.
    if (action !== 'fork') await this.client.switchNode(this.sessionId, session.active_node, () => this.navigationCurrent() && this.client.getSnapshot().generation === this.generation);
    const continuing = () => this.navigationCurrent() && this.client.getSnapshot().generation === this.generation;
    if (!continuing()) return;
    await this.client.attach(session.id, session.active_node, continuing);
    if (!continuing()) return;
    const target = this.client.target(session.id);
    if (target.conversation_id !== session.active_conversation_id) throw new Error('Transition attached a different Conversation; execution refused.');
    const editorContent = result.editor ? await readEditor(result.editor,
      async (block_index, offset) => (await this.client.request({ method: 'session/editorRead', params: { locator: result.editor!, block_index, offset, max_bytes: 65_536 } }, 'editor_page', undefined, continuing)).page,
      continuing) : [];
    let replay: { target: AttachmentTarget; messageId: string } | undefined;
    if (action === 'retry') {
      if (!editorContent.length) throw new Error('Branch has no native editor input; execution refused.');
      // Native cut excludes the selected User message. Submit the native returned
      // input exactly once, including #319 receipts, to the new Conversation.
      const accepted = await this.client.sendContent(session.id, editorContent, 'send', undefined, continuing);
      replay = { target, messageId: accepted.message_id };
      if (!continuing() || !sameTarget(this.client.getSnapshot().views[session.id]?.target, target)) return;
    }
    if (!continuing() || !sameTarget(this.client.getSnapshot().views[session.id]?.target, target)) return;
    return { session, content: action === 'retry' ? [] : editorContent, replay };
  }
  async tools() {
    this.requireCurrent();
    const result = await this.client.request({ method: 'resources/read', params: { target: this.target } }, 'domain_page');
    this.requireCurrent();
    const page = result.page, cut = page.read_cut, item = page.items[0];
    if (!item || page.items.length !== 1 || page.domain !== 'capabilities') throw Error('Tool directory header is unavailable.');
    const fields = page.collections ?? [];
    if (!item.deferred && item.value != null && item.body_complete !== false && fields.every(field => field.next_offset == null)) return page;
    const reader: TraceFieldReader = {
      page: async (cut, domain, offset, current) => (await this.client.request({ method: 'conversation/checkpointPage', params: { target: this.target, cut, domain, offset, limit: 32 } }, 'checkpoint_page', undefined, current)).page,
      value: async (cut, field, offset, current) => (await this.client.request({ method: 'conversation/checkpointValueRead', params: { target: this.target, cut, ...field, offset, max_bytes: 65536 } }, 'checkpoint_value_chunk', undefined, current)).chunk,
      content: async (_cut, ref, offset, current) => this.client.reads.readContent(this.target, undefined, ref, offset, current),
    };
    const value: Record<string, unknown> = { ...(item.value as Record<string, unknown> | null) };
    if (item.value == null || item.deferred) {
      if (!fields.length) throw Error('Tool directory source is unavailable.');
      value.revision = parseObservedValue((await readTraceValue(reader, cut, { domain: page.domain, item_offset: 0, path: ['revision'] }, this.current)).text);
    }
    for (const field of fields) {
      if (!/^capabilities\.(configured_tools|tools|available_tools|skills|sources)$/.test(field.domain)) throw Error('Tool directory field changed.');
      if (item.value != null && !item.deferred && field.next_offset == null) continue;
      const rows = await readTraceCollection(reader, cut, field, this.current);
      const values: unknown[] = [];
      for (const row of rows) values.push(row.deferred || row.value == null || row.body_complete === false
        ? parseObservedValue((await readTraceValue(reader, cut, { domain: field.domain, item_offset: Number(row.offset), path: [] }, this.current)).text) : row.value);
      value[field.domain.slice('capabilities.'.length)] = values;
    }
    this.requireCurrent();
    return { ...page, items: [{ ...item, value, deferred: false, body_complete: true }] };
  }
  async tree(offset = 0) {
    this.requireCurrent();
    const result = await this.client.request({ method: 'session/tree', params: { session_id: this.sessionId, offset, limit: 32 } }, 'tree');
    this.requireCurrent();
    const attached = result.nodes.find(node => node.conversation_id === this.target.conversation_id);
    if (attached) this.client.assertAttachmentNode(this.target, attached.id);
    return result;
  }
  async openNode(nodeId: string, conversationId: string) {
    this.requireCurrent();
    if (conversationId !== this.target.conversation_id) {
      if (!lineageSwitchSafe(this.client.getSnapshot().views[this.sessionId])) throw new Error('Wait for unresolved requests, accepted inbound and the current attempt to settle before switching lineage.');
      await this.client.switchNode(this.sessionId, nodeId, () => this.navigationCurrent() && this.client.getSnapshot().generation === this.generation);
      if (!this.navigationCurrent() || this.client.getSnapshot().generation !== this.generation) return;
      await this.client.attach(this.sessionId, nodeId, () => this.navigationCurrent() && this.client.getSnapshot().generation === this.generation);
    }
    if (!this.navigationCurrent() || this.client.getSnapshot().generation !== this.generation) return;
    if (this.client.target(this.sessionId).conversation_id !== conversationId) throw new Error('Different Conversation attached; inspect native state.');
    return true;
  }
}
