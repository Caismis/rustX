import type { Translate } from '../../../../locale/translation';
import type { NodeDefinition, Value } from '../../document/model';
import { predicateText, toolText, valueText } from './summary';
import { VariableLabel, type VariableSources } from './VariableLabel';
import css from './Editor.module.css';

type PreviewNode = Exclude<NodeDefinition, { type: 'start' | 'branch' }>;
interface Row { label: string; value: string; reference?: readonly string[]; badge?: string }

const valueRow = (label: string, value: Value): Row => ({ label, value: valueText(value), reference: value.type === 'reference' ? value.path : undefined });

function objectRows(value: Value, fallback: string): Row[] {
  if (value.type === 'object' && Object.keys(value.fields).length) return Object.entries(value.fields).slice(0, 3).map(([label, item]) => valueRow(label, item));
  if (value.type === 'literal' && value.value && typeof value.value === 'object' && !Array.isArray(value.value))
    return Object.entries(value.value).length ? Object.entries(value.value).slice(0, 3).map(([label, item]) => ({ label, value: valueText({ type: 'literal', value: item }) })) : [{ label: fallback, value: '{}' }];
  return [valueRow(fallback, value)];
}

// Tool declarations do not reveal which arguments are secrets. Dify masks
// secret parameters on the card; only reference paths are safe to preview here.
function toolArgument(tx: Translate, label: string, value: Value): Row {
  return value.type === 'reference' ? valueRow(label, value) : { label, value: tx(`workflows:value.${value.type}`) };
}

function rows(tx: Translate, node: PreviewNode): Row[] {
  switch (node.type) {
    case 'agent': return [
      { label: tx('workflows:card.profile'), value: node.profile || tx('workflows:summary.agent-unset') },
      ...(node.task.trim() ? [{ label: tx('workflows:card.task'), value: node.task }] : []),
    ];
    case 'model': return [{ label: tx('workflows:card.model'),
      value: node.model?.mode === 'explicit' ? node.model.model || tx('workflows:inspector.model-choose') : tx('workflows:inspector.model-session'),
      badge: tx(node.output.type === 'text' ? 'workflows:inspector.model-text' : 'workflows:card.json') }];
    case 'tool': return [
      { label: tx('workflows:card.tool'), value: node.selector.name ? toolText(node.selector) : tx('workflows:summary.tool-unset') },
      ...(node.arguments.type === 'object' ? Object.entries(node.arguments.fields).slice(0, 2).map(([label, item]) => toolArgument(tx, label, item)) : [toolArgument(tx, tx('workflows:card.arguments'), node.arguments)]),
    ];
    case 'review': return [
      valueRow(tx('workflows:card.subject'), node.subject.value),
      ...(node.context.length ? [{ label: tx('workflows:card.context'), value: String(node.context.length) }] : []),
    ];
    case 'loop': return [
      valueRow(tx('workflows:card.input'), node.input),
      { label: tx('workflows:card.until'), value: predicateText(tx, node.until) },
      { label: tx('workflows:card.limit'), value: String(node.max_iterations) },
    ];
    case 'return': return objectRows(node.output, tx('workflows:card.output'));
  }
}

/** Dify-style compact configuration rows, derived directly from native nodes. */
export function NodePreview({ tx, node, sources }: { tx: Translate; node: PreviewNode; sources: VariableSources }) {
  return <div className={css.nodePreview} data-node-preview>{rows(tx, node).map((row, index) => <div className={css.nodePreviewRow} key={`${row.label}:${index}`} title={`${row.label}: ${row.value}`}>
    <span className={css.nodePreviewLabel}>{row.label}</span>
    {row.reference ? <VariableLabel tx={tx} sources={sources} path={row.reference} className={css.nodePreviewReference} /> : <span className={css.nodePreviewValue}>{row.value}</span>}
    {row.badge && <span className={css.nodePreviewBadge}>{row.badge}</span>}
  </div>)}</div>;
}
