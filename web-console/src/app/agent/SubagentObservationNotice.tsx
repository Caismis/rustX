/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-primitives/ConnectionIndicator; see PROVENANCE.md. */
import { useTranslation } from '../../locale/react';
import { IconRefreshOutline14 } from '../../presentation/primitives/icons';
import { StateDot } from '../../presentation/primitives/StateDot';

/** A failed read is connection feedback, never a failed model response. */
export function SubagentObservationNotice({ error, loading, disabled, retry }: {
  error: unknown; loading: boolean; disabled: boolean; retry: () => void;
}) {
  const tx = useTranslation();
  const diagnostic = (error instanceof Error ? error.message : String(error)).slice(0, 1024);
  return <div className="w-full mb-[6px] px-[var(--dsh-composer-side-clearance)] py-0 box-border" role="alert" data-child-observation-status>
    <div className="flex items-center flex-wrap gap-x-[8px] gap-y-[4px] max-w-[var(--dsh-composer-card-max-width)] mx-auto">
      <button type="button" className="inline-flex items-center gap-[4px] min-h-[28px] px-[8px] py-0 border [border-color:color-mix(in_srgb,var(--dsw-alias-state-warn-label)_20%,transparent)] rounded-[var(--dsw-radius-sm)] bg-[var(--dsw-alias-state-warn-tertiary)] [color:var(--dsw-alias-state-warn-label)] [font:inherit] text-[12px] font-medium leading-[18px] text-left cursor-pointer hover:not-disabled:bg-[color-mix(in_srgb,var(--dsw-alias-state-warn-tertiary),var(--dsw-alias-state-warn-primary)_6%)] disabled:cursor-default" disabled={disabled || loading} onClick={retry}>
        <span className="grid place-items-center flex-none w-[14px] h-[14px]" aria-hidden>{loading ? <StateDot state="ongoing"/> : <IconRefreshOutline14/>}</span>
        <span>{tx(loading ? 'common:subagents.observation-reconnecting' : 'common:subagents.observation-retry')}</span>
      </button>
      <details className="min-w-0 [color:var(--dsw-alias-label-tertiary)] text-[12px] leading-[18px] [&_summary]:cursor-pointer open:basis-full [&_pre]:max-h-[140px] [&_pre]:mt-[6px] [&_pre]:mx-0 [&_pre]:mb-0 [&_pre]:overflow-auto [&_pre]:whitespace-pre-wrap [&_pre]:wrap-anywhere [&_pre]:[font:var(--dsw-font-markdown-code-block-small)]"><summary>{tx('common:subagents.observation-details')}</summary><pre>{diagnostic}</pre></details>
    </div>
  </div>;
}
