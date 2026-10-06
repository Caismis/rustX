/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness ui-open-in-app/OpenTargetButton.tsx; see PROVENANCE.md. */
import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { AppServerClient } from '../../client/app-server';
import type { WorkspaceAuthority } from '../../workspaces/authority';
import { WorkspaceHostError, type ProductHostWorkspaces } from '../../workspaces/host';
import { sameEndpoint } from '../../workspaces/endpoint';
import type { DesktopAppId, DesktopCatalog, DesktopTarget } from '../../workspaces/desktop';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import css from './OpenWorkspace.module.css';
import { Menu } from '../../presentation/primitives/Menu';
import { IconChevronDownOutline14, IconFolderOpenOutline16 } from '../../presentation/primitives/icons';

export function OpenWorkspace(props: { client: AppServerClient; host: ProductHostWorkspaces; authority: WorkspaceAuthority; target: DesktopTarget; disabled: boolean }) {
  const revision = useSyncExternalStore(props.authority.subscribe, props.authority.getRevision);
  const state = props.client.getSnapshot();
  return <WorkspaceAction key={JSON.stringify([revision, state.generation, state.authorityRevision, props.target])} {...props}/>;
}
function WorkspaceAction({ client, host, authority, target, disabled }: Parameters<typeof OpenWorkspace>[0]) {
  const tx = useTranslation();
  const [catalog, setCatalog] = useState<DesktopCatalog>();
  const [menu, setMenu] = useState(false);
  const [phase, setPhase] = useState<'idle' | 'loading' | 'launching' | 'spawned' | 'failed' | 'uncertain'>('idle');
  const [detail, setDetail] = useState<string>();
  const live = useRef(true), pending = useRef(false);
  useEffect(() => { live.current = true; return () => { live.current = false; }; }, []);
  const perform = async (application?: DesktopAppId, menuOnly = false, refresh = false) => {
    if (pending.current || disabled) return;
    pending.current = true;
    const before = client.getSnapshot();
    const observation = authority.capture();
    const current = () => live.current && observation?.current() && before.generation === client.getSnapshot().generation
      && before.authorityRevision === client.getSnapshot().authorityRevision;
    setPhase('loading'); setDetail(undefined);
    try {
      if (!host.desktopCatalog || !host.openWorkspace || !observation || !before.endpoint || !sameEndpoint(before.endpoint, observation.scope.endpoint)) {
        setCatalog({ available: false, reason: 'mapping' }); setPhase('idle'); return;
      }
      const apps = await host.desktopCatalog(observation.scope, refresh);
      if (!current()) return;
      setCatalog(apps);
      if (!apps.available) { setPhase('idle'); return; }
      if (menuOnly) { setMenu(true); setPhase('idle'); return; }
      const selected = apps.applications.find(app => app.id === application) ?? (application ? undefined : apps.applications[0]);
      if (!selected) throw new Error('Desktop application is no longer available');
      setPhase('launching');
      await host.openWorkspace(observation.scope, target, selected.id);
      if (current()) setPhase('spawned');
    } catch (cause) {
      if (current()) { setPhase(cause instanceof WorkspaceHostError && cause.uncertain ? 'uncertain' : 'failed'); setDetail(String(cause)); }
    } finally { pending.current = false; }
  };
  const busy = phase === 'loading' || phase === 'launching';
  const unavailable = catalog && !catalog.available ? catalog.reason : undefined;
  const label = phase === 'loading' ? tx('agent:desktop.loading') : phase === 'launching' ? tx('agent:desktop.launching') : tx('agent:desktop.open');
  const reason = unavailable === 'mapping' ? tx('agent:desktop.mapping') : unavailable === 'headless' ? tx('agent:desktop.headless')
    : unavailable === 'platform' ? tx('agent:desktop.platform') : tx('agent:desktop.applications');
  return <div className={css.action}>
    <Menu className={css.menuAnchor} open={menu} onClose={() => setMenu(false)} align="end" dense autoFocus
      anchor={<div className={css.split} data-state={busy ? 'busy' : 'idle'}>
        <button type="button" className={css.main} disabled={disabled || !!unavailable} aria-disabled={disabled || busy || !!unavailable} aria-label={label} title={tx('agent:desktop.host')} onClick={() => void perform()}><IconFolderOpenOutline16 size={13}/></button>
        <button type="button" className={css.chevron} disabled={disabled || !!unavailable} aria-disabled={disabled || busy || !!unavailable} aria-label={tx('agent:desktop.choose')} aria-haspopup="menu" aria-expanded={menu}
          onClick={() => menu ? setMenu(false) : void perform(undefined, true)}><IconChevronDownOutline14 size={10}/></button>
      </div>}
      items={catalog?.available ? [...catalog.applications.map(app => ({ id: app.id, label: app.id === 'files' ? tx('agent:desktop.files') : app.label })), { id: 'refresh', label: tx('agent:desktop.retry') }] : []}
      onSelect={id => { setMenu(false); if (id === 'refresh') void perform(undefined, true, true); else void perform(id as DesktopAppId); }}/>
    {unavailable && <p role="status">{reason}</p>}
    {phase === 'spawned' && <p role="status">{tx('agent:desktop.spawned')}</p>}
    {(phase === 'failed' || phase === 'uncertain') && <div role="alert"><p>{phase === 'uncertain' ? tx('agent:desktop.uncertain') : tx('agent:desktop.failed')}</p>
      <details><summary>{tx('agent:desktop.details')}</summary><p>{detail}</p></details></div>}
    {(unavailable || phase === 'failed') && <Button size="sm" disabled={busy || disabled} onClick={() => void perform(undefined, true, true)}>{tx('agent:desktop.retry')}</Button>}
  </div>;
}
