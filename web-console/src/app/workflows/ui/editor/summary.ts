import type { Translate } from '../../../../locale/translation';
import type { NodeDefinition, Predicate, Value } from '../../document/model';

const LIMIT = 48;
const clip = (text: string) => text.length > LIMIT ? `${text.slice(0, LIMIT - 1)}…` : text;

/** A presentation label of a value. Paths are shown joined; the document
 * keeps them as explicit components. */
export function valueText(value: Value): string {
  switch (value.type) {
    case 'reference': return value.path.join('.');
    case 'literal': return clip(JSON.stringify(value.value) ?? 'null');
    case 'object': return `{${Object.keys(value.fields).join(', ')}}`;
    case 'array': return `[${value.items.map(valueText).join(', ')}]`;
  }
}

export function predicateText(tx: Translate, predicate: Predicate): string {
  switch (predicate.type) {
    case 'boolean': return valueText(predicate.value);
    case 'equal': return `${valueText(predicate.left)} = ${valueText(predicate.right)}`;
    case 'not_equal': return `${valueText(predicate.left)} ≠ ${valueText(predicate.right)}`;
    case 'not': return `${tx('workflows:summary.not')} (${predicateText(tx, predicate.predicate)})`;
    case 'and': return predicate.predicates.map(item => predicateText(tx, item)).join(` ${tx('workflows:summary.and')} `);
    case 'or': return predicate.predicates.map(item => predicateText(tx, item)).join(` ${tx('workflows:summary.or')} `);
  }
}

export function toolText(selector: Extract<NodeDefinition, { type: 'tool' }>['selector']): string {
  return selector.origin === 'builtin' ? selector.name : `${selector.source_id}/${selector.name}`;
}

/** One line describing a node's configured behavior. */
export function nodeSummary(tx: Translate, node: NodeDefinition): string {
  switch (node.type) {
    case 'start': return '';
    case 'agent': return node.profile ? tx('workflows:summary.agent', { p0: node.profile }) : tx('workflows:summary.agent-unset');
    case 'model': return clip(tx('workflows:summary.model', { p0: node.model?.mode === 'explicit' ? node.model.model || tx('workflows:inspector.model-choose') : tx('workflows:inspector.model-session'), p1: tx(node.output.type === 'text' ? 'workflows:inspector.model-text' : 'workflows:inspector.model-json-schema') }));
    case 'tool': return node.selector.name ? tx('workflows:summary.tool', { p0: toolText(node.selector) }) : tx('workflows:summary.tool-unset');
    case 'branch': return clip(predicateText(tx, node.condition));
    case 'review': return clip(tx('workflows:summary.review', { p0: valueText(node.subject.value) }));
    case 'loop': return tx('workflows:summary.loop', { p0: node.max_iterations });
    case 'return': return clip(tx('workflows:summary.return', { p0: valueText(node.output) }));
  }
}
