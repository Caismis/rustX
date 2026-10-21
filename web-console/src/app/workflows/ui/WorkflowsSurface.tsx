import { lazy, Suspense, useSyncExternalStore } from 'react';
import { useSelector } from '@xstate/react';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Modal } from '../../../presentation/primitives/Modal';
import type { WorkspaceAuthority } from '../../../workspaces/authority';
import type { WorkflowSurfaceActor } from '../machines/surface';
import { WorkflowLibrary } from './library/Library';
import css from './Workflows.module.css';

// React Flow and the editor load only when a file is opened.
const WorkflowEditor = lazy(() => import('./editor/WorkflowEditor'));

/** The Workflows content area: Level 1 or Level 2 in place of the
 * conversation, beside the unchanged application sidebar. */
export function WorkflowsSurface({ surface, authority }: { surface: WorkflowSurfaceActor; authority: WorkspaceAuthority }) {
  const tx = useTranslation();
  const editor = useSelector(surface, snapshot => snapshot.context.editor);
  const confirming = useSelector(surface, snapshot => snapshot.matches('confirming'));
  const catalog = useSyncExternalStore(authority.subscribe, authority.getCatalog);
  const target = editor?.getSnapshot().context.target;
  const scopeLabel = target?.scope.kind === 'workspace' ? catalog?.workspaces.find(row => row.id === (target.scope as { workspaceId: string }).workspaceId)?.displayName ?? '' : tx('workflows:scope.global');
  return <div className={css.surface} data-workflows-level={editor ? 'editor' : 'library'}>
    {editor
      ? <Suspense fallback={<p className={css.state} role="status">{tx('workflows:editor.loading')}</p>}>
        <WorkflowEditor key={editor.id + (target ? JSON.stringify(target) : '')} surface={surface} editor={editor} authority={authority} scopeLabel={scopeLabel} />
      </Suspense>
      : <WorkflowLibrary surface={surface} authority={authority} />}
    <Modal open={confirming} onClose={() => surface.send({ type: 'CANCEL' })} title={tx('workflows:editor.leave-title')} closeLabel={tx('workflows:dialog.close')}
      description={tx('workflows:editor.leave-body', { p0: target?.id ?? '' })}
      footer={<><Button variant="outline" onClick={() => surface.send({ type: 'CANCEL' })}>{tx('workflows:editor.leave-cancel')}</Button>
        <Button variant="primary" className={css.danger} onClick={() => surface.send({ type: 'CONFIRM' })}>{tx('workflows:editor.leave-confirm')}</Button></>} />
  </div>;
}
