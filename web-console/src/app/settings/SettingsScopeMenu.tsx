import { useEffect, useState } from 'react';
import type { ProductHostWorkspace, ProductHostWorkspaces } from '../../workspaces/host';
import { useTranslation } from '../../locale/react';
import { Menu } from '../../presentation/primitives/Menu';
import { IconChevronDownOutline14, IconFolderClose16 } from '../../presentation/primitives/icons';
import { settingsTargetKey, userSettingsTarget, workspaceSettingsTarget, type SettingsTarget } from './projection';
import css from './extensions/ResourceCatalog.module.css';

function UserScopeIcon() {
  return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="3" y="3" width="18" height="13" rx="2"/><path d="M12 16v5m-4 0h8"/></svg>;
}

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
      anchor={<button type="button" className={css.scope} aria-label={tx('settings:catalog.scope')} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(value => !value)}>{target.kind === 'user' ? <UserScopeIcon/> : <IconFolderClose16/>}<span>{label}</span><IconChevronDownOutline14/></button>}
      items={[
        {id:'user', label:tx('settings:catalog.user'), icon:<UserScopeIcon/>},
        {id:'workspace-divider',type:'separator'},
        {id:'workspace-heading',type:'label',text:tx('settings:catalog.workspaces')},
        ...workspaces.map(workspace => ({id:`workspace:${workspace.id}`, label:workspace.displayName, icon:<IconFolderClose16/>})),
        ...(loading ? [{id:'loading',label:tx('settings:catalog.loading-workspaces'),disabled:true}] : []),
      ]}
      onSelect={id => { const workspace = workspaces.find(row => `workspace:${row.id}` === id); setOpen(false); if (id === settingsTargetKey(target)) return; if (id === 'user') onSelect(userSettingsTarget); else if (workspace) onSelect(workspaceSettingsTarget(workspace.id, workspace.displayName)); }}/>
    {error && <p role="alert">{error}</p>}
  </div>;
}
