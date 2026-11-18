/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-session-panel SessionPanelToggle.tsx; see PROVENANCE.md. */
import { useId, useSyncExternalStore } from 'react';
import type { AppServerClient } from '../../client/app-server';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { IconFlatListOutline16 } from '../../presentation/primitives/icons';
import { JobsRunningIndicator } from './BackgroundJobs';
import { panelElementId, presentationOf, toggleElementId, useSessionPanelState } from './session-panel-state';
import css from './SessionPanelToggle.module.css';

export function SessionPanelToggle({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const tx = useTranslation();
  const state = useSessionPanelState();
  const dockableMap = useSyncExternalStore(state.dockable.subscribe, state.dockable.getSnapshot);
  const visibility = useSyncExternalStore(state.visibility.subscribe, state.visibility.getSnapshot);
  const dockable = dockableMap.get(sessionId);
  const available = dockable !== undefined;
  const open = available && presentationOf(dockable === true, visibility, sessionId) !== 'hidden';
  const label = available
    ? tx(open ? 'common:session-panel.toggle-hide' : 'common:session-panel.toggle-show')
    : tx('common:session-panel.toggle-unavailable');
  const nameId = useId();
  const indicatorsId = useId();

  return (
    <Tooltip label={label} side="bottom" delayMs={500}>
      <Button
        size="sm"
        id={toggleElementId(sessionId)}
        className={css.trigger}
        aria-expanded={open}
        aria-controls={open ? panelElementId(sessionId) : undefined}
        aria-disabled={available ? undefined : true}
        aria-labelledby={`${nameId} ${indicatorsId}`}
        data-session-panel-toggle=""
        onClick={() => { state.toggle(sessionId); }}
      >
        <IconFlatListOutline16 size={16} />
        <span id={nameId} className={css.name}>{tx('common:session-panel.name')}</span>
        <span id={indicatorsId} className={css.indicators}>
          <JobsRunningIndicator client={client} sessionId={sessionId} panelShown={open} />
        </span>
      </Button>
    </Tooltip>
  );
}
