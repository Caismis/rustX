/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-session-panel ProjectRow.tsx; see PROVENANCE.md. */
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { IconFolderClose16 } from '../../presentation/primitives/icons';
import css from './WorkingDirectory.module.css';

const ROOT = /^(?:[/\\]|[A-Za-z]:[/\\])$/;

/** Split a path for display: the directories through their last separator, and the final segment. Both `/` and `\` separate; trailing separators drop first. A filesystem root keeps its separator as the whole name. */
const pathPartsOf = (path: string): { directory: string; name: string } => {
  if (ROOT.test(path)) return { directory: '', name: path };
  const trimmed = path.replace(/[/\\]+$/, '');
  if (trimmed === '') return { directory: '', name: path };
  const cut = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\')) + 1;
  return { directory: trimmed.slice(0, cut), name: trimmed.slice(cut) };
};

/** Environment row naming the Session's working folder. */
export function WorkingDirectory({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const tx = useTranslation();
  const cwd = useClientSelector(client, state =>
    state.sessions.find(row => row.id === sessionId)?.cwd
    ?? state.views[sessionId]?.summary?.cwd
    ?? state.views[sessionId]?.settings?.cwd);
  if (cwd === undefined) return null;
  const { directory, name } = pathPartsOf(cwd);
  const parent = ROOT.test(directory) ? directory : directory.replace(/[/\\]+$/, '');
  return (
    <div data-session-panel-row="" data-session-panel-project="">
      <span data-row-part="body">
        <span data-row-part="icon" aria-hidden="true"><IconFolderClose16 size={16} /></span>
        <span data-row-part="content">
          <Tooltip label={`${tx('common:activity.working-folder')}\n${cwd}`} side="bottom" delayMs={500}>
            <span className={css.folder} tabIndex={0}>
              <span className={css.name}>{name}</span>
              {parent !== '' && <span className={css.parent}>{parent}</span>}
            </span>
          </Tooltip>
        </span>
      </span>
    </div>
  );
}
