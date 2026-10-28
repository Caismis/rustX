import { UploadFailure } from '../../client/uploads';
import { RequestNotDispatched } from '../../client/errors';
import { modelPreferences } from '../model-preference';
import { sameEndpoint } from '../../workspaces/endpoint';
import type { AppServerClient } from '../../client/app-server';
import { sameTarget } from '../../../../protocol/app-server/attachment-identity';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import type { AttachmentTarget } from '../../../../protocol/app-server/v60';
import type { FirstSubmitPort } from './first-submit';

export function firstSubmitPort(client: AppServerClient, host: ProductHostWorkspaces, navigationCurrent: () => boolean, preparePublication: () => ((id: string) => (() => boolean) | void), existing = false): FirstSubmitPort {
  const { generation, authorityRevision, endpoint } = client.getSnapshot();
  let target: AttachmentTarget | undefined;
  let navigation = navigationCurrent;
  let publication: { id: string } | undefined;
  let opened: ReturnType<typeof preparePublication> | undefined;
  const current = () => {
    const state = client.getSnapshot();
    return navigation() && state.endpoint === endpoint && state.connection === 'connected' && state.generation === generation && state.authorityRevision === authorityRevision
      && (!target || sameTarget(state.views[target.session_id]?.target, target));
  };
  const requireCurrent = () => { if (!current()) throw new Error('New Conversation authority changed. Inspect Sessions; do not replay.'); };
  const bindSession = (session: { id: string; conversation: string }) => {
    requireCurrent(); const attached = client.target(session.id);
    if (attached.conversation_id !== session.conversation) throw new Error('Retained upload belongs to another Conversation');
    target = attached;
  };
  const recordSubmissionIntent = async (id: string, node: string, current: () => boolean) => {
    // This receipt can start native initialization. Authorize the fixed Session
    // cwd again at dispatch, before any MCP-dependent attachment is awaited.
    const admitted = await client.admitSessionOperation(id, current);
    if (!admitted) throw new RequestNotDispatched('Workspace admission changed before submission intent. No operation was sent.');
    await client.request({ method: 'session/submissionIntent', params: { session_id: id, node_id: node } }, 'submission_intent_accepted', undefined, admitted);
  };
  return {
    current,
    async readExisting(id) {
      const node = client.getSnapshot().views[id]?.nodeId ??
        (await client.request({ method: 'session/read', params: { session_id: id } }, 'session', undefined, current)).session.active_node;
      const sameNode = () => current() && (client.getSnapshot().views[id]?.nodeId ?? node) === node;
      await recordSubmissionIntent(id, node, sameNode);
      await client.waitForAttachment(id, sameNode);
      requireCurrent();
      if (client.getSnapshot().views[id]?.nodeId !== node) throw new Error('Submission node changed. Inspect the Session; do not replay.');
      const attached = client.target(id);
      target = attached;
      return { id, node, conversation: attached.conversation_id };
    },
    async create(draft, acknowledged) {
      opened = preparePublication();
      if (!endpoint || !draft.workspaceId) throw new Error('Choose a registered Workspace first.');
      const catalog = await host.listWorkspaces();
      if (!sameEndpoint(catalog.endpoint, endpoint) || !catalog.workspaces.some(workspace => workspace.id === draft.workspaceId)) throw new Error('Workspace authority changed before submission.');
      const submission = draft.preparation ?? { draft_id: crypto.randomUUID(), generation: 1, host_authority: catalog.authorityId };
      if (submission.host_authority !== catalog.authorityId) throw new Error('Workspace Host authority changed before submission.');
      const { cwd } = await host.resolveWorkspace(draft.workspaceId, endpoint);
      const [initialLocation] = await host.classifyLocations([cwd], endpoint, catalog.authorityId);
      if (!initialLocation?.authorized) throw new RequestNotDispatched('Session cwd is not authorized by this Product Host. No operation was sent.');
      requireCurrent();
      const result = await client.request({ method: 'session/create', params: { submission, settings: { cwd, ...(draft.model ? { model: draft.model } : {}) } } }, 'session_transition', result => {
        acknowledged({ id: result.session.id, node: result.session.active_node, conversation: result.session.active_conversation_id, diagnostic: result.durability_diagnostic ?? undefined });
        const state = client.getSnapshot();
        if (draft.model && state.endpoint === endpoint && state.authorityRevision === authorityRevision) modelPreferences().select(endpoint, draft.model);
      }, { current, validate: async signal => {
        const fresh = await host.listWorkspaces(signal);
        if (!current() || fresh.authorityId !== submission.host_authority || !sameEndpoint(fresh.endpoint, endpoint)) return false;
        const [location] = await host.classifyLocations([cwd], endpoint, fresh.authorityId, signal);
        if (!location?.authorized) throw new RequestNotDispatched(`Session cwd is no longer authorized by this Product Host (${location?.reason ?? 'unavailable'}). No operation was sent.`);
        const resolved = await host.resolveWorkspace(draft.workspaceId, endpoint);
        return current() && resolved.cwd === cwd;
      } });
      // Native receipt of this actual Send already armed the shared deadline,
      // before durable creation and MCP-dependent load/attachment. The decoded
      // creation acknowledgement remains authoritative through navigation loss.
      return { id: result.session.id, node: result.session.active_node, conversation: result.session.active_conversation_id, diagnostic: result.durability_diagnostic ?? undefined };
    },
    handoff(session) {
      const state = client.getSnapshot();
      if (!navigation() || state.endpoint !== endpoint || state.authorityRevision !== authorityRevision) throw new Error('Navigation authority changed. No operation was replayed.');
      client.restoreViews([session.id]);
      // Only this authorized transition may replace the captured route fence.
      publication = session;
    },
    async attach(session) {
      if (existing) {
        await client.waitForAttachment(session.id, current);
        bindSession(session);
      } else await client.attach(session.id, session.node, current, attached => { target = attached; });
      requireCurrent();
      if (!target) throw new Error('Created Session was not attached.');
      if (target.conversation_id !== session.conversation) throw new Error('Created Session attached a different Conversation.');
      if (publication) { navigation = opened!(publication.id) ?? navigation; publication = undefined; }
      requireCurrent();
      // Attach consumes the established native model/configuration. No second
      // selection, provider probe, model repair or catalog refresh owns startup.
    },
    async upload(session, file, acknowledged, operation) { try { bindSession(session); } catch (error) { throw new UploadFailure('failed', error); } const [uploaded] = await client.upload(session.id, [file], { current, acknowledged: files => { if (files[0]) acknowledged(files[0].receipt); } }, operation); if (!uploaded) throw new Error('Upload receipt missing; inspect native state.'); return uploaded.receipt; },
    status: async (session, operation) => { bindSession(session); const result = await client.uploadStatus(session.id, operation); requireCurrent(); return result; },
    async send(session, draft, receipts, acknowledged) { bindSession(session); draft.submission?.bind(target!, JSON.stringify([generation, target]), receipts); await client.send(session.id, draft.text, receipts, 'send', acceptance => { const view = client.getSnapshot().views[session.id]; const conversation = view?.nodeConversationId ?? view?.target?.conversation_id ?? view?.snapshot?.conversation_id; draft.submission?.accepted(acceptance, conversation ? { session_id: session.id, conversation_id: conversation } : undefined); acknowledged(); }, current); },
  };
}
