import { useTranslation, useNotice } from '../locale/react';
import { useEffect, useRef, useState } from 'react';
import type { RuntimeClientSessionDeletePreview } from '../../../protocol/app-server/v57';
import type { AppServerClient } from '../client/app-server';
import { isOutcomeUncertain } from '../client/errors';
import { useClientSelector, transportSelection, sameValue } from '../client/selectors';
import { sessionDeletionNotice } from '../bindings/session-deletion';
import { Modal } from '../presentation/primitives/Modal';
import { Button } from '../presentation/primitives/Button';
import css from '../presentation/workspace/WorkspaceBrowser.module.css';

/** One action owns its confirmation, pending state, and rejection. Durable
 * cleanup/uncertainty remains in the native/client projection after it closes. */
export function SessionDeletion({ client, sessionId, title, close, deleted }: {
  client: AppServerClient; sessionId: string; title: string; close: () => void; deleted: () => void;
}) {
  const tx = useTranslation();
  const transport = useClientSelector(client, transportSelection, sameValue);
  const lifecycle = useClientSelector(client, state => state.views[sessionId]?.lifecycle, sameValue);
  const [preview, setPreview] = useState<RuntimeClientSessionDeletePreview>();
  const [busy, setBusy] = useState(true), [error, setError] = useNotice(), [uncertain, setUncertain] = useState(false);
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
  return <Modal open title={tx('common:session-deletion.confirm-session-deletion')} closeLabel={tx('common:session-deletion.close-deletion-confirmation')} onClose={dismiss}
    description={tx('common:session-deletion.description', { title })}
    footer={<><Button variant="outline" disabled={busy && guard.current} onClick={dismiss}>{tx('common:session-deletion.keep-session')}</Button>
      <Button variant="outline" className={css.deleteAction} disabled={busy || uncertain || !preview || lifecycle?.delete === false || transport.connection !== 'connected'} onClick={() => void confirm()}>{tx('common:session-deletion.confirm-delete')}</Button></>}>
    {busy && <div className={css.deleteStatus} role="status">{tx(guard.current ? 'common:session-deletion.deleting' : 'common:session-deletion.reading')}</div>}
    {error && <div className={css.renameError} role="alert">{error}</div>}
    {uncertain && <div className={css.renameError} role="alert">{tx('common:session-deletion.deletion-outcome-uncertain-inspect-native-recovery-before-anothe')}</div>}
  </Modal>;
}
