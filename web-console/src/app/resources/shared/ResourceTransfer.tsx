import { useId, useRef, useState } from 'react';
import type { ResourceTransfer, SourceScope, TransferOperation } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Modal } from '../../../presentation/primitives/Modal';
import { IconCopyOutline16, IconFolderClose16 } from '../../../presentation/primitives/icons';
import type { CardMenuItems } from '../../../presentation/catalog/CardMenu';
import { WorkspaceHostError } from '../../../workspaces/host';
import type { TransferResource } from '../../../workspaces/resource-transfer';
import css from '../../../presentation/settings/SettingsContent.module.css';

export interface TransferIntent {
  operation: TransferOperation;
  family: ResourceTransfer['family'];
  from: SourceScope;
  name: string;
  revision: string;
  directory: string;
  destinationLabel: string;
  taken: ReadonlySet<string>;
  transfer: TransferResource;
}

export function transferActions(tx: ReturnType<typeof useTranslation>, from: SourceScope, disabled: boolean): CardMenuItems {
  return [
    { id: 'copy-scope', label: tx(from === 'user' ? 'resources:transfer.copy-workspace' : 'resources:transfer.copy-global'), icon: <IconCopyOutline16 />, disabled },
    { id: 'move-scope', label: tx(from === 'user' ? 'resources:transfer.move-workspace' : 'resources:transfer.move-global'), icon: <IconFolderClose16 />, disabled },
  ];
}

function copyName(name: string, taken: ReadonlySet<string>, bounded: boolean) {
  let index = 1;
  while (true) {
    const suffix = index === 1 ? '-copy' : `-copy-${index}`;
    const candidate = (bounded ? name.slice(0, 64 - suffix.length) : name) + suffix;
    if (!taken.has(candidate)) return candidate;
    index += 1;
  }
}

export function TransferDialog({ intent, close, refresh }: { intent: TransferIntent; close: () => void; refresh: () => void }) {
  const tx = useTranslation();
  const moving = intent.operation === 'move';
  const [name, setName] = useState(() => moving ? intent.name : copyName(intent.name, intent.taken, intent.family !== 'mcp'));
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string>();
  const [reopen, setReopen] = useState(false);
  const submitting = useRef(false);
  const input = useId();
  const destination = name.trim();
  const invalid = !destination || (intent.family !== 'mcp' && (!/^[a-z][a-z0-9_-]{0,63}$/.test(destination) || intent.family === 'workflow' && destination === 'workflow_output'))
    || intent.family === 'mcp' && destination.startsWith('python:');
  const taken = intent.taken.has(destination) || !moving && destination === intent.name;
  const submit = async () => {
    if (submitting.current || reopen || invalid || taken) return;
    submitting.current = true;
    setBusy(true);
    setFailure(undefined);
    try {
      await intent.transfer(intent.directory, intent.revision, {
        kind: 'resource_transfer', from: intent.from, operation: intent.operation,
        resource: { family: intent.family, source: intent.name, destination },
      });
      refresh();
      close();
    } catch (cause) {
      const error = cause instanceof WorkspaceHostError ? cause : undefined;
      const native = error?.native;
      if (error?.uncertain || native?.kind === 'committed_durability_uncertain' || native?.kind === 'outcome_unknown') {
        setFailure(tx('resources:transfer.uncertain'));
        setReopen(true);
        refresh();
      } else if (native?.kind === 'source_conflict' || error?.kind === 'workspace_unavailable' || error?.kind === 'authority_replaced' || error?.kind === 'native_replaced') {
        setFailure(tx('resources:transfer.changed'));
        setReopen(true);
        refresh();
      } else {
        setFailure(native?.kind === 'resource_transfer_rejected' ? tx(`resources:transfer.${native.reason}`)
          : native?.kind === 'workflow_invalid' ? native.diagnostic.reason : tx('resources:transfer.failed'));
      }
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  const dismiss = () => { if (!submitting.current) close(); };
  return <Modal open onClose={dismiss} title={tx(moving ? 'resources:transfer.move-title' : 'resources:transfer.copy-title')}
    closeLabel={tx('workflows:dialog.close')} description={tx(moving ? 'resources:transfer.move-body' : 'resources:transfer.copy-body', { p0: intent.name, p1: intent.destinationLabel })}
    footer={<><Button variant="outline" onClick={dismiss} disabled={busy}>{tx('resources:cancel')}</Button>
      <Button variant="primary" onClick={() => void submit()} disabled={busy || reopen || invalid || taken}>
        {tx(moving ? 'resources:transfer.move' : 'resources:transfer.copy')}
      </Button></>}>
    <form className={css.settings} onSubmit={event => { event.preventDefault(); void submit(); }}>
      <label htmlFor={input}>{tx('resources:transfer.destination-name')}</label>
      <input id={input} value={name} autoFocus spellCheck={false} disabled={busy || reopen}
        aria-invalid={invalid || taken} onChange={event => { setName(event.target.value); setFailure(undefined); }} />
      {(taken || failure) && <p role="alert">{taken ? tx('resources:transfer.name_taken') : failure}</p>}
    </form>
  </Modal>;
}
