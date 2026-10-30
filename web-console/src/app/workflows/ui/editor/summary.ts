import type { Translate } from '../../../../locale/translation';
import type { NodeDefinition, Predicate, Value } from '../../document/model';

const LIMIT = 48;
const clip = (text: string) => text.length > LIMIT ? `${text.slice(0, LIMIT - 1)}…` : text;

/** A presentation label of a value. Paths are shown joined; the document
 * keeps them as explicit components. */
/** The text a prompt is copied and previewed as: variables in Dify's `{{…}}` form. */
export function promptText(parts: readonly Value[]): string {
  return parts.map(part => part.type === 'reference' ? `{{${part.path.join('.')}}}` : part.type === 'literal' && typeof part.value === 'string' ? part.value : '').join('');
}

export function valueText(value: Value): string {
  switch (value.type) {
    case 'reference': return value.path.join('.');
    case 'literal': return clip(JSON.stringify(value.value) ?? 'null');
    case 'object': return `{${Object.keys(value.fields).join(', ')}}`;
    case 'array': return `[${value.items.map(valueText).join(', ')}]`;
    case 'template': return clip(promptText(value.parts));
  }
}

export function predicateText(tx: Translate, predicate: Predicate): string {
  switch (predicate.type) {
    case 'boolean': return valueText(predicate.value);
    case 'equal': case 'not_equal': case 'contains': case 'not_contains': case 'starts_with': case 'ends_with': {
      const left = valueText(predicate.left), right = valueText(predicate.right);
      return left && right ? `${left} ${tx(`workflows:branch.${predicate.type}`)} ${right}` : tx('workflows:branch.unset');
    }
    case 'empty': case 'not_empty': return `${valueText(predicate.value)} ${tx(`workflows:branch.${predicate.type}`)}`;
    case 'not': return `${tx('workflows:summary.not')} (${predicateText(tx, predicate.predicate)})`;
    case 'and': return predicate.predicates.map(item => predicateText(tx, item)).join(` ${tx('workflows:summary.and')} `);
    case 'or': return predicate.predicates.map(item => predicateText(tx, item)).join(` ${tx('workflows:summary.or')} `);
  }
}

export function toolText(selector: Extract<NodeDefinition, { type: 'tool' }>['selector']): string {
  return selector.origin === 'builtin' ? selector.name : `${selector.source_id}/${selector.name}`;
}

/** Number of compact configuration rows a native node renders on its card. */
export function previewRowCount(node: NodeDefinition): number {
  switch (node.type) {
    case 'agent': return node.task.length ? 2 : 1;
    case 'model': return 1;
    case 'tool': return 1 + (node.arguments.type === 'object' ? Math.min(Object.keys(node.arguments.fields).length, 2) : 1);
    case 'review': return node.context.length ? 2 : 1;
    case 'loop': return 3;
    case 'return': return Math.max(1, Math.min(node.output.type === 'object' ? Object.keys(node.output.fields).length : node.output.type === 'literal' && node.output.value && typeof node.output.value === 'object' && !Array.isArray(node.output.value) ? Object.keys(node.output.value).length : 1, 3));
    case 'start': case 'branch': return 0;
  }
}
