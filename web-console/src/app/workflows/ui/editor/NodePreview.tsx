import type { Translate } from '../../../../locale/translation';
import type { NodeDefinition, Value } from '../../document/model';
import { predicateText, promptText, toolText, valueText } from './summary';
import { VariableLabel, type VariableSources } from './VariableLabel';
import css from './Editor.module.css';

type PreviewNode = Exclude<NodeDefinition, { type: 'start' | 'branch' | 'review' }>;
interface Row { label: string; value: string; reference?: readonly string[]; badge?: string }

const valueRow = (label: string, value: Value): Row => ({ label, value: valueText(value), reference: value.type === 'reference' ? value.path : undefined });

// Tool declarations do not reveal which arguments are secrets. Dify masks
// secret parameters on the card; only reference paths are safe to preview here.
function toolArgument(tx: Translate, label: string, value: Value): Row {
  return value.type === 'reference' ? valueRow(label, value) : { label, value: tx(`workflows:value.${value.type}`) };
}

function rows(tx: Translate, node: PreviewNode): Row[] {
  switch (node.type) {
    case 'agent': return [
      { label: tx('workflows:card.profile'), value: node.profile || tx('workflows:summary.agent-unset') },
      ...(node.task.length ? [{ label: tx('workflows:card.task'), value: promptText(node.task) }] : []),
    ];
    case 'model': return [{ label: tx('workflows:card.model'),
      value: node.model?.mode === 'explicit' ? node.model.model || tx('workflows:inspector.model-choose') : tx('workflows:inspector.model-session'),
      badge: tx(node.output.type === 'text' ? 'workflows:inspector.model-text' : 'workflows:card.json') }];
    case 'tool': return [
      { label: tx('workflows:card.tool'), value: node.selector.name ? toolText(node.selector) : tx('workflows:summary.tool-unset') },
      ...(node.arguments.type === 'object' ? Object.entries(node.arguments.fields).slice(0, 2).map(([label, item]) => toolArgument(tx, label, item)) : [toolArgument(tx, tx('workflows:card.arguments'), node.arguments)]),
    ];
    case 'loop': return [
      ...(node.loop_variables ?? []).slice(0, 2).map(variable => valueRow(variable.label, variable.value)),
      { label: tx('workflows:loop.max-count'), value: String(node.loop_count),
        ...(node.break_condition ? { badge: predicateText(tx, node.break_condition) } : {}) },
    ];
    case 'iteration': return [
      valueRow(tx('workflows:iteration.input'), node.iterator),
      { ...valueRow(tx('workflows:iteration.output'), node.output), ...(node.is_parallel ? { badge: tx('workflows:iteration.parallel') } : {}) },
    ];
    case 'assigner': return node.items.length ? node.items.slice(0, 3).map(item => ({ label: item.variable.join('.'), value: tx(`workflows:assigner.op-${item.operation}`) }))
      : [{ label: tx('workflows:assigner.variables'), value: tx('workflows:assigner.unset') }];
    case 'loop_end': return [];
    // As Dify's End card: each chosen variable, without its name.
    case 'return': return node.outputs.filter(output => output.value_selector.length)
      .map(output => ({ label: '', value: output.value_selector.join('.'), reference: output.value_selector }));
  }
}

/** Dify-style compact configuration rows, derived directly from native nodes. */
export function NodePreview({ tx, node, sources }: { tx: Translate; node: PreviewNode; sources: VariableSources }) {
  return <div className={css.nodePreview} data-node-preview>{rows(tx, node).map((row, index) => <div className={css.nodePreviewRow} key={`${row.label}:${index}`} title={row.label ? `${row.label}: ${row.value}` : row.value}>
    {row.label && <span className={css.nodePreviewLabel}>{row.label}</span>}
    {row.reference ? <VariableLabel tx={tx} sources={sources} path={row.reference} className={css.nodePreviewReference} /> : <span className={css.nodePreviewValue}>{row.value}</span>}
    {row.badge && <span className={css.nodePreviewBadge}>{row.badge}</span>}
  </div>)}</div>;
}
