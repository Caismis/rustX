import type { McpConnectionSnapshot } from '../../../../../protocol/app-server/v37';
import { useTranslation } from '../../../locale/react';
import { Tooltip } from '../../../presentation/primitives/Tooltip';
import type { ExtensionEntry } from '../extensions/inventory';
import { McpIcon } from './McpIcon';
import css from './McpPage.module.css';

/** Displays native settings-connection state, separate from Agent permissions. */
export function McpStatusIcon({ entry, snapshot, enabled }: { entry: ExtensionEntry; snapshot?: McpConnectionSnapshot; enabled: boolean }) {
  const tx = useTranslation();
  const state = entry.valid === false ? 'invalid' : !enabled ? 'disabled'
    : snapshot?.state.status === 'connected' ? 'connected'
      : snapshot?.state.status === 'connecting' ? 'connecting'
        : snapshot?.state.status === 'failed' ? 'error'
          : snapshot?.state.status === 'timed_out' ? 'timeout'
            : snapshot?.state.status === 'disconnected' ? 'disconnected'
              : entry.diagnostics.length ? 'warning' : 'unknown';
  const label = [tx(`settings:mcp.status-${state}`), ...entry.diagnostics].join('\n');
  return <Tooltip label={label} side="top" maxWidth={256}>
    <span className={css.icon} role="img" aria-label={label}>
      <McpIcon/><span className={css.dot} data-status={state} aria-hidden="true"/>
    </span>
  </Tooltip>;
}
