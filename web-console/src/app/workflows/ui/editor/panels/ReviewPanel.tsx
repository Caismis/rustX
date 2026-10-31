import clsx from 'clsx';
import { useState } from 'react';
import { IconChevronDownOutline14, IconQuestionOutline14, IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { Tooltip } from '../../../../../presentation/primitives/Tooltip';
import { useTranslation } from '../../../../../locale/react';
import { CommandRejected, freshIdentifier, renameExit, replaceNodeExits } from '../../../document/commands';
import { encodeField } from '../../../document/fields';
import { blockAt, nodeAt, type NodeDefinition } from '../../../document/model';
import { IconPlus } from '../../icons';
import { PromptCard } from '../forms/PromptCard';
import { PromptEditor } from '../forms/PromptEditor';
import { promptText } from '../summary';
import { InsertVariable, RouteId } from './InferencePanels';
import { useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

type Review = Extract<NodeDefinition, { type: 'review' }>;
const MINUTE = 60_000, HOUR = 60 * MINUTE, MAX_TIMEOUT = 24 * HOUR;

/** As Dify's Human Input panel: the content shown to the person, the
 * actions they may choose, each its own exit, then the timeout. */
export function ReviewPanel(props: PanelProps) {
  return <>
    <ReviewContent {...props} />
    <ReviewActions {...props} />
    <div className={css.panelDivider} />
    <ReviewTimeout {...props} />
    <div className={css.panelDivider} />
    <ReviewOutputs />
  </>;
}

function Heading({ title, tip, children }: { title: string; tip: string; children?: React.ReactNode }) {
  return <h3 className={css.groupHeading}>
    <span className={css.headingText}>{title}
      <Tooltip label={tip} side="bottom" maxWidth={260}><span className={css.promptHint} tabIndex={0} aria-label={tip}><IconQuestionOutline14 /></span></Tooltip></span>
    {children}
  </h3>;
}

function ReviewContent(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'review');
  const [expanded, setExpanded] = useState(false), [session, setSession] = useState(0);
  const address = at('content'), field = encodeField(address), label = tx('workflows:review.content');
  return <section className={css.group}>
    <PromptEditor parts={node.content} address={address} edit={props.edit} label={label} expanded={expanded} invalid={!node.content.length}
      placeholder={tx('workflows:review.content-placeholder')} variablePlaceholder={tx('workflows:branch.select-variable')}
      onChange={content => update(current => ({ ...current, content }), address, undefined, `${field}:${session}`)}>
      {editable => <PromptCard label={label} hint={tx('workflows:review.content-tip')} text={promptText(node.content)} expanded={expanded} onExpand={() => setExpanded(value => !value)}
        role={<b className={css.promptRoleFixed}>{label}</b>} tools={<InsertVariable />}>
        <div onFocus={() => setSession(value => value + 1)}>{editable}</div>
      </PromptCard>}
    </PromptEditor>
  </section>;
}

/** Replace this node and drop connections on actions it no longer has. */
function useReviewExits(props: PanelProps) {
  return (change: (review: Review) => Review) => props.edit(document => {
    const owner = blockAt(document, props.block), current = owner && nodeAt(owner, props.id);
    if (current?.type !== 'review') throw new CommandRejected('missing_node');
    return replaceNodeExits(document, props.block, props.id, change(current));
  });
}

function ReviewActions(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'review'), exits = useReviewExits(props);
  // As Dify: `action_N` with "Button Text N", N one past the count.
  const add = () => exits(review => {
    const id = freshIdentifier({ input: {}, nodes: Object.fromEntries(review.actions.map(action => [action.id, { type: 'start' }])), edges: [] }, `action_${review.actions.length + 1}`);
    return { ...review, actions: [...review.actions, { id, title: tx('workflows:review.default-title', { p0: String(review.actions.length + 1) }) }] };
  });
  return <section className={css.group} aria-label={tx('workflows:review.actions')}>
    <Heading title={tx('workflows:review.actions')} tip={tx('workflows:review.actions-tip')}>
      <button type="button" className={css.iconButton} disabled={node.actions.length >= 12} aria-label={tx('workflows:review.add-action')} title={tx('workflows:review.add-action')} onClick={add}><IconPlus size={14} /></button>
    </Heading>
    {!node.actions.length && <p className={css.reviewEmpty}>{tx('workflows:review.actions-empty')}</p>}
    <div className={css.reviewActions}>
      {node.actions.map((action, index) => <div key={action.id} className={css.reviewAction}>
        <RouteId value={action.id} label={tx('workflows:review.action-id')} className={css.reviewActionId}
          onCommit={to => props.edit(document => renameExit(document, props.block, props.id, action.id, to))} />
        <RouteId value={action.title} label={tx('workflows:review.action-title')} className={css.reviewActionTitle}
          onCommit={title => update(current => ({ ...current, actions: current.actions.map((item, at) => at === index ? { ...item, title } : item) }), at('actions', index, 'title'))} />
        <button type="button" className={css.iconButton} aria-label={tx('workflows:review.remove-action', { p0: action.id })} title={tx('workflows:review.remove-action', { p0: action.id })}
          onClick={() => exits(review => ({ ...review, actions: review.actions.filter(item => item.id !== action.id) }))}><IconTrashOutline16 /></button>
      </div>)}
    </div>
  </section>;
}

/** As Dify's timeout: a number and its unit. A run lasts at most a day, so
 * the units are hours and minutes. */
function ReviewTimeout(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'review');
  const milliseconds = Number(node.timeout_ms);
  const [unit, setUnit] = useState(() => milliseconds % HOUR === 0 ? HOUR : MINUTE);
  const value = Math.max(1, Math.round(milliseconds / unit));
  const commit = (count: number, per: number) => {
    const next = Math.min(MAX_TIMEOUT, Math.max(per, Math.round(count) * per));
    if (Number.isFinite(next) && next !== milliseconds) update(current => ({ ...current, timeout_ms: String(next) }), at('timeout_ms'));
  };
  return <section className={clsx(css.group, css.reviewTimeout)}>
    <h3 className={css.groupHeading}><span className={css.headingText}>{tx('workflows:review.timeout')}</span></h3>
    <input className={css.input} type="number" min={1} max={MAX_TIMEOUT / unit} aria-label={tx('workflows:review.timeout')} value={value}
      onChange={event => { if (event.target.value !== '') commit(Number(event.target.value), unit); }} />
    <div className={css.typeSwitch} role="radiogroup" aria-label={tx('workflows:review.timeout')}>
      {([HOUR, MINUTE] as const).map(per => <button key={per} type="button" role="radio" aria-checked={unit === per} className={clsx(unit === per && css.typeSwitchActive)}
        onClick={() => { setUnit(per); commit(value, per); }}>{tx(per === HOUR ? 'workflows:review.hours' : 'workflows:review.minutes')}</button>)}
    </div>
  </section>;
}

/** As Dify's output variables: the action chosen, then what was shown. */
function ReviewOutputs() {
  const tx = useTranslation(), [open, setOpen] = useState(true);
  return <section className={css.group} aria-label={tx('workflows:prompt.output-variables')}>
    <h3 className={css.groupHeading}>
      <button type="button" className={css.outputToggle} aria-expanded={open} onClick={() => setOpen(value => !value)}>{tx('workflows:prompt.output-variables')}<IconChevronDownOutline14 /></button>
    </h3>
    {open && <dl className={css.outputVars}>
      <div><dt><b>{/* i18n-raw: native output field name */ 'action'}</b><code>{/* i18n-raw: JSON Schema type name */ 'string'}</code></dt><dd>{tx('workflows:review.output-action')}</dd></div>
      <div><dt><b>{/* i18n-raw: native output field name */ 'content'}</b><code>{/* i18n-raw: JSON Schema type name */ 'string'}</code></dt><dd>{tx('workflows:review.output-content')}</dd></div>
    </dl>}
  </section>;
}
