import clsx from 'clsx';
import type { Translate } from '../../../../locale/translation';
import type { NodeKind, Predicate, Value } from '../../document/model';
import { KindIcon } from '../icons';
import { valueText } from './summary';
import css from './Editor.module.css';

/** What a reference's first segment names where it is read: a node of the
 * block or around it, or (`args`) the root Start. An empty title falls back
 * to the default Start title. */
export interface VariableSource { kind: NodeKind | 'start'; title: string }
export type VariableSources = Readonly<Record<string, VariableSource>>;

function sourceTitle(tx: Translate, source: VariableSource): string {
  return source.title || (source.kind === 'start' ? tx('workflows:canvas.start-title') : '');
}

/** As Dify's VariableLabelInNode: the source node's icon and title, a slash,
 * then `{x}` and the variable name in the variable colour. */
export function VariableLabel({ tx, sources, path, className }: { tx: Translate; sources: VariableSources; path: readonly string[]; className?: string }) {
  const source = sources[path[0]], name = path.length > 1 ? path.slice(1).join('.') : source ? tx('workflows:value.whole') : path[0];
  const title = source && sourceTitle(tx, source);
  return <span className={clsx(css.variableLabel, className)} title={title ? `${title} / ${name}` : name}>
    {source && <><KindIcon kind={source.kind} size={12} className={css.variableLabelIcon} /><span className={css.variableLabelSource}>{title}</span><span className={css.variableLabelSlash}>/</span></>}
    <b><span aria-hidden>{'{x}'}</span>{name}</b>
  </span>;
}

/** A Value on a card: references as labels, strings as written (as Dify),
 * anything else as its JSON text. */
export function ValueView({ tx, sources, value }: { tx: Translate; sources: VariableSources; value: Value }) {
  return value.type === 'reference' ? <VariableLabel tx={tx} sources={sources} path={value.path} /> : <span className={css.variableLabelText}>{value.type === 'literal' && typeof value.value === 'string' ? value.value : valueText(value)}</span>;
}

/** As Dify's ConditionValue on the IF/ELSE card: variable, operator, value. */
export function PredicateView({ tx, sources, predicate }: { tx: Translate; sources: VariableSources; predicate: Predicate }) {
  switch (predicate.type) {
    case 'boolean': return <ValueView tx={tx} sources={sources} value={predicate.value} />;
    case 'empty': case 'not_empty': return <><ValueView tx={tx} sources={sources} value={predicate.value} /><span className={css.variableLabelOperator}>{tx(`workflows:branch.${predicate.type}`)}</span></>;
    case 'equal': case 'not_equal': case 'contains': case 'not_contains': case 'starts_with': case 'ends_with':
      return <><ValueView tx={tx} sources={sources} value={predicate.left} /><span className={css.variableLabelOperator}>{tx(`workflows:branch.${predicate.type}`)}</span><ValueView tx={tx} sources={sources} value={predicate.right} /></>;
    default: return null;
  }
}
