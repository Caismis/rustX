import type { NodeDefinition, Predicate } from './model';
export type Branch = Extract<NodeDefinition, { type: 'branch' }>;
export type BranchCase = Branch['cases'][number];

export function conditionsOf(condition: Predicate): readonly Predicate[] {
  return condition.type === 'and' || condition.type === 'or' ? condition.predicates : [condition];
}
/** Shared card/handle geometry, so every CASE keeps its own aligned exit. */
export function branchGeometry(node: Branch) {
  let offset = 53; // border + padding + header + body margin
  const cases = node.cases.map(item => {
    const top = offset + 12;
    offset += 24 + conditionsOf(item.condition).length * 26;
    return { id: item.id, top };
  });
  return { cases, elseTop: offset + 12, height: offset + 37 };
}
export function branchPortLabel(node: Branch, port: string) {
  if (port === 'false') return 'ELSE';
  const index = node.cases.findIndex(item => item.id === port);
  return node.cases.length > 1 ? `CASE ${index + 1}` : 'IF';
}
