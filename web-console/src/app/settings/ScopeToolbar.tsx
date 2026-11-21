import { createContext, useContext, useEffect, useState } from 'react';
import type { ProductHostWorkspace, ProductHostWorkspaces } from '../../workspaces/host';
import { useTranslation } from '../../locale/react';
import { Menu } from '../../presentation/primitives/Menu';
import { IconChevronDownOutline14, IconFolderClose16, IconRefreshOutline16 } from '../../presentation/primitives/icons';
import { useSelector } from '@xstate/react';
import { useSettingsActor } from './machines/react';
import { settingsTargetKey, userSettingsTarget, workspaceSettingsTarget, type SettingsTarget } from './projection';
import css from '../../presentation/settings/SettingsPageShell.module.css';

/** What a page's own toolbar needs to switch scope: the active target, the
 * workspace catalog, and the guarded retarget gesture. Provided once per
 * Settings instance; pages render `PageScopeToolbar` and nothing else. */
export const ScopeNavContext = createContext<{
  target: SettingsTarget; host?: ProductHostWorkspaces; open: (target: SettingsTarget) => void;
} | null>(null);

export function PageScopeToolbar({ userLabel, disabled }: { userLabel?: string; disabled?: boolean }) {
  const nav = useContext(ScopeNavContext);
  if (!nav) return null;
  return <ScopeToolbar target={nav.target} host={nav.host} onSelect={nav.open} userLabel={userLabel} disabled={disabled} />;
}

/** The page's authoritative reread, staged through the target actor so the
 * same observation lifecycle serves every page identically. */
export function PageRefreshButton({ disabled = false }: { disabled?: boolean }) {
  const tx = useTranslation();
  const actor = useSettingsActor();
  const busy = useSelector(actor, snapshot => snapshot.matches({ mutation: 'submitting' }));
  return (
    <button
      type="button"
      className={css.refreshButton}
      aria-label={tx('settings:settings.reload-configuration')}
      title={tx('settings:settings.reload-configuration')}
      disabled={disabled || busy}
      onClick={() => actor.send({ type: 'REFRESH' })}
    >
      <IconRefreshOutline16 />
    </button>
  );
}

/** The DSH scope toolbar: a [工作区|全局] capsule plus a workspace chooser
 * that only appears while the workspace segment is active. Scope changes are
 * page-level navigation, so callers run them through the unsaved-changes
 * guard before calling `onSelect`. */
export function ScopeToolbar({ target, host, onSelect, userLabel, disabled = false }: {
  target: SettingsTarget; userLabel?: string; disabled?: boolean;
  host?: ProductHostWorkspaces; onSelect: (target: SettingsTarget) => void;
}) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const [workspaces, setWorkspaces] = useState<ProductHostWorkspace[]>([]);
  useEffect(() => {
    if (!host) return;
    const abort = new AbortController();
    void host.listWorkspaces(abort.signal)
      .then(catalog => { if (!abort.signal.aborted) setWorkspaces(catalog.workspaces); })
      .catch(() => {});
    return () => abort.abort();
  }, [host]);
  const workspaceActive = target.kind === 'workspace';
  const activeWorkspace = workspaceActive ? workspaces.find(row => row.id === target.id) : undefined;
  const pickWorkspace = (workspace: ProductHostWorkspace) => {
    setOpen(false);
    if (`workspace:${workspace.id}` !== settingsTargetKey(target)) onSelect(workspaceSettingsTarget(workspace.id, workspace.displayName));
  };
  return <div className={css.scopeBar}>
    <div className={css.scopeSegment} role="group" aria-label={tx('settings:catalog.scope')}>
      <button type="button" aria-pressed={workspaceActive} disabled={disabled}
        onClick={() => { const next = activeWorkspace ?? workspaces[0]; if (!workspaceActive && next) onSelect(workspaceSettingsTarget(next.id, next.displayName)); }}>
        {tx('settings:catalog.workspaces')}
      </button>
      <button type="button" aria-pressed={!workspaceActive} disabled={disabled}
        onClick={() => { if (workspaceActive) onSelect(userSettingsTarget); }}>
        {userLabel ?? tx('settings:catalog.user')}
      </button>
    </div>
    {workspaceActive && <Menu open={open} onClose={() => setOpen(false)} autoFocus
      selectedId={settingsTargetKey(target)}
      anchor={<button type="button" className={css.scopeChooser} disabled={disabled} aria-haspopup="menu" aria-expanded={open}
        onClick={() => setOpen(value => !value)}>
        <IconFolderClose16 /><span>{activeWorkspace?.displayName ?? target.displayName}</span><IconChevronDownOutline14 />
      </button>}
      items={workspaces.map(workspace => ({ id: `workspace:${workspace.id}`, label: workspace.displayName, icon: <IconFolderClose16 /> }))}
      onSelect={id => { const workspace = workspaces.find(row => `workspace:${row.id}` === id); if (workspace) pickWorkspace(workspace); else setOpen(false); }} />}
  </div>;
}
