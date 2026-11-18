import { useEffect, useState } from 'react';
import { useTranslation } from '../locale/react';
import { actionError } from '../bindings/action-error';
import { Button } from '../presentation/primitives/Button';
import { Modal } from '../presentation/primitives/Modal';
import { IconFolderClose16 } from '../presentation/primitives/icons';
import type { ProductHostWorkspaces, WorkspaceAuthorityScope, WorkspaceFolderListing, WorkspaceFolderSelection } from './host';
import css from './WorkspaceFolderDialog.module.css';

interface Cursor { location: string; path: string }

export function WorkspaceFolderDialog({ host, scope, locations, busy, error, close, add }: {
  host: ProductHostWorkspaces; scope: WorkspaceAuthorityScope; locations: readonly { id: string; displayName: string }[];
  busy: boolean; error?: string; close: () => void; add: (location: string, selection: WorkspaceFolderSelection) => void;
}) {
  const tx = useTranslation();
  const [cursor, setCursor] = useState<Cursor>();
  const [result, setResult] = useState<{ cursor: Cursor; listing?: WorkspaceFolderListing; error?: string }>();
  useEffect(() => {
    if (!cursor || !host.browseWorkspace) return;
    const controller = new AbortController();
    void host.browseWorkspace(scope, cursor.location, cursor.path, controller.signal).then(listing => {
      if (!controller.signal.aborted) setResult({ cursor, listing });
    }, cause => {
      if (!controller.signal.aborted) setResult({ cursor, error: actionError(cause) });
    });
    return () => controller.abort();
  }, [host, scope, cursor]);
  const current = result?.cursor === cursor ? result : undefined;
  const listing = current?.listing;
  const up = () => {
    if (!cursor) return;
    setCursor(cursor.path ? { ...cursor, path: cursor.path.split('/').slice(0, -1).join('/') } : undefined);
  };
  return <Modal open title={tx('workspace:workspace-navigation.add-workspace')}
    closeLabel={tx('workspace:workspace-navigation.close-dialog')} onClose={() => { if (!busy) close(); }}
    className={css.dialog} description={tx('workspace:folder-browser.description')}
    footer={<>
      <Button variant="outline" disabled={busy} onClick={close}>{tx('common:conversation-composer.cancel')}</Button>
      <Button variant="primary" disabled={busy || !cursor || !listing} onClick={() => {
        if (cursor && listing) add(cursor.location, { path: cursor.path, revision: listing.revision });
      }}>{tx('workspace:folder-browser.add-folder')}</Button>
    </>}>
    {error && <p className={css.notice} role="alert">{error}</p>}
    {cursor && <div className={css.path}>
      <Button variant="outline" size="sm" disabled={busy} onClick={up}>{tx(cursor.path ? 'workspace:folder-browser.up' : 'workspace:folder-browser.roots')}</Button>
      {listing && <code>{listing.displayPath}</code>}
    </div>}
    <div className={css.folders} aria-busy={!!cursor && !current || undefined}>
      {!cursor ? locations.map(location => <button key={location.id} type="button" className={css.row}
        onClick={() => setCursor({ location: location.id, path: '' })}><IconFolderClose16 />{location.displayName}</button>)
        : listing ? listing.folders.length ? listing.folders.map(folder => <button key={folder} type="button" className={css.row} disabled={busy}
          onClick={() => setCursor({ ...cursor, path: cursor.path ? `${cursor.path}/${folder}` : folder })}><IconFolderClose16 />{folder}</button>)
          : <p className={css.notice} role="status">{tx('workspace:folder-browser.empty')}</p>
          : current?.error ? <p className={css.notice} role="alert">{current.error}</p>
            : <p className={css.notice} role="status">{tx('workspace:folder-browser.loading')}</p>}
    </div>
  </Modal>;
}
