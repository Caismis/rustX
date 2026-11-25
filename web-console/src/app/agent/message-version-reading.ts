import { useEffect, useState } from 'react';
import type { AttachmentTarget, SessionMessageVersionFamily, SessionMessageVersionsPage } from '../../../../protocol/app-server/v81';
import { sameTarget } from '../../../../protocol/app-server/attachment-identity';
import { canonicalLocatorKey } from '../../../../protocol/app-server/content-identity';
import type { AppServerClient } from '../../client/app-server';

const empty = new Map<string, SessionMessageVersionFamily>();

/** Read sparse version families once per native reading owner, independently of transcript paging. */
export function useMessageVersions(client: AppServerClient, target: AttachmentTarget | undefined, nodeId: string | undefined) {
  const [loaded, setLoaded] = useState<{
    target: AttachmentTarget;
    nodeId: string;
    versions: ReadonlyMap<string, SessionMessageVersionFamily>;
  }>();
  useEffect(() => {
    if (!target || !nodeId) return;
    let live = true;
    const current = () => {
      const state = client.getSnapshot();
      const view = state.views[target.session_id];
      return live && state.connection === 'connected' && view?.nodeId === nodeId && sameTarget(view.target, target);
    };
    void (async () => {
      const versions = new Map<string, SessionMessageVersionFamily>();
      let offset = 0;
      while (current()) {
        const result = await client.request({ method: 'session/messageVersions', params: {
          session_id: target.session_id, node_id: nodeId, offset, limit: 32,
        } }, 'session_message_versions', undefined, current);
        const page: SessionMessageVersionsPage = result.page;
        if (!current()) return;
        if (page.versions.length > 32 || page.next_offset != null && page.next_offset !== offset + page.versions.length
          || page.next_offset === offset) throw new Error('Invalid message version page.');
        for (const family of page.versions) {
          const version = family.current;
          if (version.node_id !== nodeId || version.conversation_id !== target.conversation_id) continue;
          versions.set(canonicalLocatorKey(version.boundary), family);
          if (version.message_id) versions.set(canonicalLocatorKey({ type: 'by_message_id', message_id: version.message_id }), family);
        }
        if (page.next_offset == null) break;
        offset = page.next_offset;
      }
      if (current()) setLoaded({ target, nodeId, versions });
    })().catch(() => {});
    return () => { live = false; };
  }, [client, target, nodeId]);

  return target && loaded && loaded.nodeId === nodeId && sameTarget(loaded.target, target) ? loaded.versions : empty;
}
