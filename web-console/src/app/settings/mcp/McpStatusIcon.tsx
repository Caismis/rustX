import { useTranslation } from '../../../locale/react';
import { Tooltip } from '../../../presentation/primitives/Tooltip';
import type { ExtensionEntry } from '../extensions/inventory';
import { McpIcon } from './McpIcon';
import css from './McpPage.module.css';

/** Selection is a permission, not evidence of runtime availability. Native
 * currently publishes preparation snapshots, not a connecting lifecycle. */
export function McpStatusIcon({ entry }: { entry: ExtensionEntry }) {
  const tx = useTranslation();
  const state = entry.valid === false ? 'invalid'
    : entry.preparation === 'unavailable' ? 'error'
      : entry.diagnostics.length ? 'warning'
        : entry.preparation === 'ready' ? 'ready'
          : entry.preparation === 'unprepared' ? 'unprepared' : 'unknown';
  const label = [tx(`settings:mcp.status-${state}`), ...entry.diagnostics].join('\n');
  return <Tooltip label={label} side="top" maxWidth={256}>
    <span className={css.icon} role="img" aria-label={label}>
      <McpIcon/><span className={css.dot} data-status={state} aria-hidden="true"/>
    </span>
  </Tooltip>;
}
