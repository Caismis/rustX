import { useRef, useState } from 'react';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Modal } from '../../../presentation/primitives/Modal';
import css from './SkillDisplayDialog.module.css';

export function SkillDeleteDialog({ name, scope, path, remove, close }: {
  name: string; scope: string; path: string; remove: () => Promise<void>; close: () => void;
}) {
  const tx = useTranslation();
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const submitting = useRef(false);
  const dismiss = () => { if (!submitting.current) close(); };
  const submit = async () => {
    if (submitting.current || failed) return;
    submitting.current = true;
    setBusy(true);
    try {
      await remove();
      close();
    } catch {
      setFailed(true);
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  return <Modal open onClose={dismiss} title={tx('resources:skills.delete-title')} closeLabel={tx('resources:close')}
    description={tx('resources:skills.delete-body', { p0: name, p1: scope })}
    footer={<>
      <Button variant="outline" disabled={busy} onClick={dismiss}>{tx('resources:cancel')}</Button>
      <Button variant="primary" className={css.danger} disabled={busy || failed} onClick={() => void submit()}>{tx('resources:skills.delete')}</Button>
    </>}>
    <code>{path}</code>
    {failed && <p role="alert">{tx('resources:skills.delete-failed')}</p>}
  </Modal>;
}
