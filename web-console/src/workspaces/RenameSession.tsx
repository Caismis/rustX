/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-workspace/session-actions/RenameSession.tsx; see PROVENANCE.md. */
import { useEffect, useRef, useState } from 'react';
import type { AppServerClient } from '../client/app-server';
import { useTranslation } from '../locale/react';
import { Modal } from '../presentation/primitives/Modal';
import { Button } from '../presentation/primitives/Button';
import { workspaceBrowserCss } from '../presentation/workspace/workspaceBrowserCss';
import { Input as TextInput } from '@/presentation/primitives/Input';

/** DSH rename dialog over one native name mutation; no chooser or list-refresh barrier. */
export function RenameSession({ client, id, title, close, finalFocus }: { client: AppServerClient; id: string; title: string; close: () => void; finalFocus?: () => HTMLElement | false }) {
  const tx = useTranslation(), [draft, setDraft] = useState(title), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const composing = useRef(false), pending = useRef(false), alive = useRef(true);
  const generation = client.getSnapshot().generation;
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const dismiss = () => { if (!pending.current) { alive.current = false; close(); } };
  const confirm = async () => {
    if (pending.current || !draft.trim()) return;
    pending.current = true; setBusy(true); setError('');
    try {
      await client.renameSession(id, draft.trim());
      if (alive.current && client.getSnapshot().generation === generation) close();
    } catch (cause) { if (alive.current && client.getSnapshot().generation === generation) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  };
  return <Modal open finalFocus={finalFocus} title={tx('workspace:rename.session')} closeLabel={tx('workspace:workspace-navigation.close-dialog')} onClose={dismiss}
    footer={<><Button variant="outline" disabled={busy} onClick={dismiss}>{tx('workspace:cancel')}</Button><Button variant="primary" disabled={busy || !draft.trim()} onClick={() => void confirm()}>{tx('workspace:rename')}</Button></>}>
    <TextInput unstyled className={workspaceBrowserCss.renameInput} data-modal-autofocus="" aria-label={tx('workspace:field.sessionName')} value={draft} disabled={busy}
      onFocus={event => event.target.select()} onChange={event => { setDraft(event.target.value); setError(''); }}
      onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }}
      onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); if (!composing.current && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229) void confirm(); } }}/>
    {error && <div className={workspaceBrowserCss.renameError} role="alert">{error}</div>}
  </Modal>;
}
