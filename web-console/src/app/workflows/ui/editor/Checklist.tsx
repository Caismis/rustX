import { Popover } from '@base-ui/react/popover';
import clsx from 'clsx';
import { useRef, useState } from 'react';
import { useTranslation } from '../../../../locale/react';
import { IconChecklistOutline14, IconCloseOutline16 } from '../../../../presentation/primitives/icons';
import type { WorkflowDependencyFailure } from '../../../../../../protocol/app-server/v71';
import type { ChecklistItem, ChecklistMessage } from '../../document/checklist';
import { IconArrowLeft, IconWorkflow, KindIcon } from '../icons';
import css from './Editor.module.css';

/** Dify's checklist: the header button counts the nodes with problems, and
 * its popover lists each with what is wrong; choosing one goes to it. It
 * follows every edit, so there is nothing to run. */
export function Checklist({ items, onGo }: { items: readonly ChecklistItem[]; onGo: (item: ChecklistItem, message: ChecklistMessage) => void }) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  // Going to a problem focuses its field; focus then stays there.
  const going = useRef(false);
  const label = tx('workflows:checklist.title');
  const text = ({ text }: ChecklistMessage) => text.kind === 'required'
    ? tx('workflows:checklist.required', { p0: tx(text.field === 'if' ? 'workflows:branch.if' : text.field === 'elif' ? 'workflows:branch.elif' : `workflows:checklist.field-${text.field}`) })
    : text.kind === 'unconnected' ? tx('workflows:checklist.unconnected') : text.kind === 'dependency' ? dependency(tx, text.failure) : text.reason;
  const title = (item: ChecklistItem) => item.node === undefined ? tx('workflows:checklist.workflow')
    : item.type === 'start' ? item.title || tx(item.block.length ? 'workflows:canvas.nested-start' : 'workflows:canvas.start-title') : item.title || item.node;
  return <Popover.Root open={open} onOpenChange={next => { if (next) going.current = false; setOpen(next); }}>
    <Popover.Trigger className={css.checklistButton} aria-label={items.length ? `${label} (${items.length})` : label} title={label}>
      <IconChecklistOutline14 />{!!items.length && <span className={css.checklistCount} aria-hidden>{items.length}</span>}
    </Popover.Trigger>
    <Popover.Portal><Popover.Positioner side="bottom" align="end" sideOffset={10} className={css.checklistPositioner}><Popover.Popup className={css.checklist} finalFocus={() => !going.current}>
      <div className={css.checklistHead}>
        <Popover.Title className={css.checklistTitle}>{label}{!!items.length && `(${items.length})`}</Popover.Title>
        <Popover.Close className={css.iconButton} aria-label={tx('workflows:dialog.close')}><IconCloseOutline16 /></Popover.Close>
      </div>
      {items.length ? <>
        <Popover.Description className={css.checklistDescription}>{tx('workflows:checklist.description')}</Popover.Description>
        <ul className={css.checklistItems}>{items.map(item => <li key={`${JSON.stringify(item.block)}:${item.node ?? ''}`} className={css.checklistNode}>
          <div className={css.checklistNodeHead}>
            <span className={clsx(css.checklistIcon, item.type && item.type !== 'start' ? css[`kind_${item.type}`] : css.startIcon)}>
              {item.type ? <KindIcon kind={item.type} size={13} /> : <IconWorkflow size={13} />}
            </span>
            <span className={css.checklistNodeTitle}>{title(item)}</span>
          </div>
          {item.messages.map((message, index) => <button key={index} type="button" className={css.checklistMessage} title={text(message)} onClick={() => { going.current = true; setOpen(false); onGo(item, message); }}>
            <span className={css.checklistDot} aria-hidden /><span className={css.checklistText}>{text(message)}</span>
            <span className={css.checklistGo}>{tx('workflows:checklist.go')}<IconArrowLeft size={13} className={css.checklistArrow} /></span>
          </button>)}
        </li>)}</ul>
      </> : <p className={css.checklistResolved}><IconChecklistOutline14 />{tx('workflows:checklist.resolved')}</p>}
    </Popover.Popup></Popover.Positioner></Popover.Portal>
  </Popover.Root>;
}

/** Why the native admission blocks a node, in the author's words. */
function dependency(tx: ReturnType<typeof useTranslation>, failure: WorkflowDependencyFailure): string {
  const tool = (error: Extract<WorkflowDependencyFailure, { kind: 'tool' }>['detail']) => {
    switch (error.kind) {
      case 'source_unavailable': return tx(`workflows:checklist.dependency-source-${error.reason.kind}`, { p0: error.source });
      case 'exact_tool_absent': return tx('workflows:checklist.dependency-tool-absent', { p0: error.source, p1: error.name });
      case 'unknown_capability': return tx('workflows:checklist.dependency-unknown-tool', { p0: error.selector });
    }
  };
  switch (failure.kind) {
    case 'not_admitted': return tx('workflows:checklist.dependency-not-admitted');
    case 'model': return tx('workflows:checklist.dependency-model', { p0: failure.detail.reason });
    case 'materialization': return tx('workflows:checklist.dependency-materialization');
    case 'tool': return tool(failure.detail);
    case 'ineligible_tool': return tx('workflows:checklist.dependency-ineligible-tool', { p0: failure.detail.origin === 'source' ? `${failure.detail.source_id}/${failure.detail.name}` : failure.detail.name });
    case 'agent': {
      const agent = failure.detail;
      switch (agent.kind) {
        case 'host_tool_suppressed': return tx('workflows:checklist.dependency-host-tool', { p0: agent.detail.name });
        case 'tool': return tool(agent.detail);
        case 'skill_unavailable': return tx('workflows:checklist.dependency-skill', { p0: agent.detail.name });
        case 'agent_unavailable': return tx('workflows:checklist.dependency-agent', { p0: agent.detail.name });
        case 'workflow_unavailable': return tx('workflows:checklist.dependency-workflow', { p0: agent.detail.id });
        case 'scope_unsupported': {
          const capability = agent.detail.capability;
          return tx('workflows:checklist.dependency-scope', { p0: capability.kind === 'goal' ? tx('workflows:checklist.dependency-goal') : capability.identity });
        }
      }
    }
  }
}
