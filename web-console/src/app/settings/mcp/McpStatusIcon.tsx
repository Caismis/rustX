import { useTranslation } from '../../../locale/react';
import { Tooltip } from '../../../presentation/primitives/Tooltip';
import { preparationLabel, selectionLabel, type ExtensionEntry } from '../extensions/inventory';
import { McpIcon } from './McpIcon';
import css from './McpPage.module.css';

/** Configuration observations never claim a live Agent connection. */
export function McpStatusIcon({ entry }: { entry: ExtensionEntry }) {
  const tx = useTranslation();
  const state = entry.valid === false ? 'invalid' : 'unknown';
  const label = [tx(`settings:mcp.status-${state}`), preparationLabel(tx, entry), selectionLabel(tx, entry), ...entry.diagnostics].filter(Boolean).join('\n');
  return <Tooltip label={label} side="top" maxWidth={256}>
    <span className={css.icon} role="img" aria-label={label}>
      <McpIcon/><span className={css.dot} data-status={state} aria-hidden="true"/>
    </span>
  </Tooltip>;
}
