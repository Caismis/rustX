import { useEffect, useRef, useState } from 'react';
import type { RuntimeClientSessionDeletePreview } from '../../../protocol/app-server/v25';
import { type AppServerClient, isOutcomeUncertain } from '../client/app-server';
import { useClientSelector, transportSelection, sameValue } from '../client/selectors';
import { sessionDeletionNotice } from '../bindings/session-deletion';
import { Modal } from '../presentation/primitives/Modal';
import { Button } from '../presentation/primitives/Button';

/** One action owns its confirmation, pending state, and rejection. Durable
 * cleanup/uncertainty remains in the native/client projection after it closes. */
export function SessionDeletion({ client, sessionId, title, close, deleted }: {
  client: AppServerClient; sessionId: string; title: string; close: () => void; deleted: () => void;
}) {
  const transport = useClientSelector(client, transportSelection, sameValue);
  const [preview, setPreview] = useState<RuntimeClientSessionDeletePreview>();
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [uncertain, setUncertain] = useState(false);
  const guard = useRef(false);
  const generation = transport.generation;
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    setPreview(undefined); setError(''); setBusy(true);
    void client.request({ method: 'session/deletePreview', params: { session_id: sessionId } }, 'deletion').then(result => {
      if (!alive.current || client.getSnapshot().generation !== generation) return;
      if (result.result.status === 'preview') setPreview(result.result.preview);
      else setError(sessionDeletionNotice(result.result));
    }).catch(cause => { if (alive.current && client.getSnapshot().generation === generation) setError(String(cause)); })
      .finally(() => { if (alive.current && client.getSnapshot().generation === generation) setBusy(false); });
    return () => { alive.current = false; };
  }, [client, sessionId, generation]);
  const dismiss = () => { if (!guard.current) close(); };
  const confirm = async () => {
    if (!preview || guard.current || uncertain || transport.connection !== 'connected') return;
    guard.current = true; setBusy(true); setError('');
    try {
      const result = await client.deleteSession(sessionId, preview.target_revision);
      if (!alive.current || client.getSnapshot().generation !== generation || !result) return;
      if (result.status === 'deleted' || result.status === 'not_found' || result.status === 'committed_cleanup_pending') {
        deleted(); close();
      } else {
        setError(sessionDeletionNotice(result)); setPreview(undefined);
        if (result.status === 'committed_durability_uncertain') setUncertain(true);
      }
    } catch (cause) {
      if (alive.current) { setError(String(cause)); setUncertain(isOutcomeUncertain(cause)); }
    } finally { guard.current = false; if (alive.current) setBusy(false); }
  };
  return <Modal open title="Confirm Session deletion" closeLabel="Close deletion confirmation" onClose={dismiss}>
    <h3>Delete {title}?</h3>
    <p>This permanently deletes the Session and its saved history.</p>
    {preview && <p>Saved conversations: {preview.owned_conversation_count} · History nodes: {preview.owned_node_count} · Child conversations: {preview.owned_child_count}</p>}
    <p>Any active work will be settled before deletion.</p>
    {error && <p role="alert">{error}</p>}
    {uncertain && <p role="alert">Deletion outcome uncertain. Inspect native recovery before another action. No operation was replayed.</p>}
    <div className="row" aria-busy={busy}>
      <Button disabled={busy} onClick={dismiss}>Keep Session</Button>
      <Button variant="primary" disabled={busy || uncertain || !preview || transport.connection !== 'connected'} onClick={() => void confirm()}>Confirm delete</Button>
    </div>
  </Modal>;
}
