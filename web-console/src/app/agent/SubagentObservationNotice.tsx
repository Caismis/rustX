/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-primitives/ConnectionIndicator; see PROVENANCE.md. */
import { useTranslation } from '../../locale/react';
import { IconRefreshOutline14 } from '../../presentation/primitives/icons';
import { StateDot } from '../../presentation/primitives/StateDot';
import css from './SubagentObservationNotice.module.css';

/** A failed read is connection feedback, never a failed model response. */
export function SubagentObservationNotice({ error, loading, disabled, retry }: {
  error: unknown; loading: boolean; disabled: boolean; retry: () => void;
}) {
  const tx = useTranslation();
  const diagnostic = (error instanceof Error ? error.message : String(error)).slice(0, 1024);
  return <div className={css.root} role="alert" data-child-observation-status>
    <div className={css.row}>
      <button type="button" className={css.indicator} disabled={disabled || loading} onClick={retry}>
        <span className={css.icon} aria-hidden>{loading ? <StateDot state="ongoing"/> : <IconRefreshOutline14/>}</span>
        <span>{tx(loading ? 'common:subagents.observation-reconnecting' : 'common:subagents.observation-retry')}</span>
      </button>
      <details className={css.details}><summary>{tx('common:subagents.observation-details')}</summary><pre>{diagnostic}</pre></details>
    </div>
  </div>;
}
