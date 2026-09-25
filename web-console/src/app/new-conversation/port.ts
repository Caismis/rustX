import { sameTarget, type AppServerClient } from '../../client/app-server';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import type { AttachmentTarget } from '../../../../protocol/app-server/v24';
import type { FirstSubmitPort } from './first-submit';
import { selectSessionModel } from '../model-preference';

export function firstSubmitPort(client: AppServerClient, host: ProductHostWorkspaces, navigationCurrent: () => boolean): FirstSubmitPort {
  const { generation, authorityRevision, endpoint } = client.getSnapshot();
  let target: AttachmentTarget | undefined;
  const current = () => {
    const state = client.getSnapshot();
    return navigationCurrent() && state.endpoint === endpoint && state.connection === 'connected' && state.generation === generation && state.authorityRevision === authorityRevision
      && (!target || sameTarget(state.views[target.session_id]?.target, target));
  };
  const requireCurrent = () => { if (!current()) throw new Error('New Conversation authority changed. Inspect Sessions; do not replay.'); };
  return {
    current,
    async create(draft) {
      if (!endpoint || !draft.workspaceId) throw new Error('Choose a registered Workspace first.');
      const { cwd } = await host.resolveWorkspace(draft.workspaceId, endpoint);
      requireCurrent();
      const result = await client.request({ method: 'session/create', params: { settings: { cwd } } }, 'session_transition');
      return { id: result.session.id, node: result.session.active_node, conversation: result.session.active_conversation_id, diagnostic: result.durability_diagnostic ?? undefined };
    },
    async attach(session) {
      await client.attach(session.id, session.node, current);
      requireCurrent(); target = client.target(session.id);
      if (target.conversation_id !== session.conversation) throw new Error('Created Session attached a different Conversation.');
      await client.listSessions();
    },
    async model(session, intent) {
      await selectSessionModel(client, session.id, intent); requireCurrent();
      await client.repairAgentModel(session.id); requireCurrent();
      const observed = client.getSnapshot().views[session.id];
      if (observed?.modelMutation || observed?.snapshot?.model?.configured.model !== intent.model
        || observed.snapshot.model.effective.model !== intent.model
        || (intent.reasoningProfile !== undefined && observed.snapshot.model.effective.reasoningProfile !== intent.reasoningProfile)) {
        throw new Error('Requested Session model has not been observed. No turn was started.');
      }
    },
    async upload(session, file) { const [uploaded] = await client.upload(session.id, [file]); if (!uploaded) throw new Error('Upload receipt missing; inspect native state.'); return uploaded.receipt; },
    async send(session, draft, receipts) { await client.send(session.id, draft.text, receipts); },
  };
}
