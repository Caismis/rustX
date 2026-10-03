import { modelPreferences } from '../model-preference';
import { sameTarget, type AppServerClient } from '../../client/app-server';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import type { AttachmentTarget } from '../../../../protocol/app-server/v34';
import type { FirstSubmitPort } from './first-submit';

export function firstSubmitPort(client: AppServerClient, host: ProductHostWorkspaces, navigationCurrent: () => boolean, opened: (id: string) => (() => boolean) | void): FirstSubmitPort {
  const { generation, authorityRevision, endpoint } = client.getSnapshot();
  let target: AttachmentTarget | undefined;
  let navigation = navigationCurrent;
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
  return {
    current,
    async create(draft, acknowledged) {
      if (!endpoint || !draft.workspaceId) throw new Error('Choose a registered Workspace first.');
      const { cwd } = await host.resolveWorkspace(draft.workspaceId, endpoint);
      requireCurrent();
      const result = await client.request({ method: 'session/create', params: { settings: { cwd, ...(draft.model ? { model: draft.model } : {}) } } }, 'session_transition', result => {
        acknowledged({ id: result.session.id, node: result.session.active_node, conversation: result.session.active_conversation_id, diagnostic: result.durability_diagnostic ?? undefined });
        const state = client.getSnapshot();
        if (draft.model && state.endpoint === endpoint && state.authorityRevision === authorityRevision) modelPreferences().select(endpoint, draft.model);
      }, current);
      return { id: result.session.id, node: result.session.active_node, conversation: result.session.active_conversation_id, diagnostic: result.durability_diagnostic ?? undefined };
    },
    handoff(session) {
      const state = client.getSnapshot();
      if (!navigation() || state.endpoint !== endpoint || state.authorityRevision !== authorityRevision) throw new Error('Navigation authority changed. No operation was replayed.');
      client.restoreViews([session.id]);
      // Only this authorized transition may replace the captured route fence.
      navigation = opened(session.id) ?? navigation;
    },
    async attach(session) {
      await client.attach(session.id, session.node, current, attached => { target = attached; });
      requireCurrent();
      if (!target) throw new Error('Created Session was not attached.');
      if (target.conversation_id !== session.conversation) throw new Error('Created Session attached a different Conversation.');
      // Attach consumes the established native model/configuration. No second
      // selection, provider probe, model repair or catalog refresh owns startup.
    },
    async upload(session, file, acknowledged, operation) { bindSession(session); const [uploaded] = await client.upload(session.id, [file], { current, acknowledged: files => { if (files[0]) acknowledged(files[0].receipt); } }, operation); if (!uploaded) throw new Error('Upload receipt missing; inspect native state.'); return uploaded.receipt; },
    status: async (session, operation) => { bindSession(session); const result = await client.uploadStatus(session.id, operation); requireCurrent(); return result; },
    async send(session, draft, receipts, acknowledged) { bindSession(session); await client.send(session.id, draft.text, receipts, 'send', acknowledged, current); },
  };
}
