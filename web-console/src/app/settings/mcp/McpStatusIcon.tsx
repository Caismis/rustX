import { useTranslation } from '../../../locale/react';
import { Tooltip } from '../../../presentation/primitives/Tooltip';
import { type ExtensionEntry } from '../extensions/inventory';
import type { ProbeState } from './useMcpProbes';
import { McpIcon } from './McpIcon';
import css from './McpPage.module.css';

/** Configuration observations never claim a live Agent connection. */
export function McpStatusIcon({ entry, probe }: { entry: ExtensionEntry; probe?: ProbeState }) {
  const tx = useTranslation();
  const state = entry.valid === false ? 'invalid' : probe ?? 'unknown';
  const label = [tx(state === 'unknown' || state === 'invalid' ? `settings:mcp.status-${state}` : `settings:mcp.probe-${state}`), ...entry.diagnostics].filter(Boolean).join('\n');
  return <Tooltip label={label} side="top" maxWidth={256}>
    <span className={css.icon} role="img" aria-label={label}>
      <McpIcon/><span className={css.dot} data-status={state} aria-hidden="true"/>
    </span>
  </Tooltip>;
}
