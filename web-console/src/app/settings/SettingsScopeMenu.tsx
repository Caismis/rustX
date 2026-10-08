import { useEffect, useState } from 'react';
import type { ProductHostWorkspace, ProductHostWorkspaces } from '../../workspaces/host';
import { useTranslation } from '../../locale/react';
import { Menu } from '../../presentation/primitives/Menu';
import { IconChevronDownOutline14, IconBrowseOutline16 } from '../../presentation/primitives/icons';
import { settingsTargetKey, userSettingsTarget, workspaceSettingsTarget, type SettingsTarget } from './projection';
import css from './extensions/ResourceCatalog.module.css';

/** Scope changes select an exact registered owner; they never retarget a draft. */
export function SettingsScopeMenu({ target, host, onSelect }: {target: SettingsTarget; host?: ProductHostWorkspaces; onSelect: (target: SettingsTarget) => void}) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const [workspaces, setWorkspaces] = useState<ProductHostWorkspace[]>([]);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    if (!open || !host) return;
    const abort = new AbortController();
    setLoading(true); setError('');
    void host.listWorkspaces(abort.signal).then(catalog => { if (!abort.signal.aborted) setWorkspaces(catalog.workspaces); })
      .catch(cause => { if (!abort.signal.aborted) setError(String(cause)); })
      .finally(() => { if (!abort.signal.aborted) setLoading(false); });
    return () => abort.abort();
  }, [open, host]);
  const label = target.kind === 'user' ? tx('settings:catalog.user') : target.displayName;
  return <div>
    <Menu open={open} onClose={() => setOpen(false)} autoFocus selectedId={settingsTargetKey(target)}
      anchor={<button type="button" className={css.scope} aria-label={tx('settings:catalog.scope')} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(value => !value)}><IconBrowseOutline16/><span>{label}</span><IconChevronDownOutline14/></button>}
      items={[
        {id:'user', label:tx('settings:catalog.user')},
        ...workspaces.map(workspace => ({id:`workspace:${workspace.id}`, label:workspace.displayName})),
        ...(loading ? [{id:'loading',label:tx('settings:catalog.loading-workspaces'),disabled:true}] : []),
      ]}
      onSelect={id => { const workspace = workspaces.find(row => `workspace:${row.id}` === id); setOpen(false); if (id === settingsTargetKey(target)) return; if (id === 'user') onSelect(userSettingsTarget); else if (workspace) onSelect(workspaceSettingsTarget(workspace.id, workspace.displayName)); }}/>
    {error && <p role="alert">{error}</p>}
  </div>;
}
