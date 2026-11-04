import { CommandRejected } from './commands';
import { blockAt, nodeAt, updateBlock, type BlockPath, type WorkflowDocument } from './model';
import { rewriteBlockReferences } from './references';

export function aggregateGroupUsed(document: WorkflowDocument, block: BlockPath, id: string, name: string): boolean {
  let used = false;
  rewriteBlockReferences(blockAt(document, block)!, (path) => {
    if (path[0] === id && path[1] === name) used = true;
    return undefined;
  });
  return used;
}
export function toggleAggregateGroups(
  document: WorkflowDocument,
  block: BlockPath,
  id: string,
  grouped: boolean,
): WorkflowDocument {
  return updateBlock(document, block, (owner) => {
    const node = nodeAt(owner, id);
    if (node?.type !== 'aggregate' || node.grouped === grouped) throw new CommandRejected('missing_node');
    const first = node.groups[0];
    if (!first) throw new CommandRejected('missing_field');
    const name = first.name;
    const oldPrefix = node.grouped ? [id, first.name, 'output'] : [id, 'output'],
      newPrefix = grouped ? [id, name, 'output'] : [id, 'output'];
    const next = {
      ...owner,
      nodes: {
        ...owner.nodes,
        [id]: {
          ...node,
          grouped,
          groups: node.groups.map((group, index) => (index === 0 ? { ...group, name } : group)),
        },
      },
    };
    return rewriteBlockReferences(next, (path) => {
      if (path[0] !== id) return undefined;
      if (oldPrefix.every((part, index) => path[index] === part))
        return [...newPrefix, ...path.slice(oldPrefix.length)];
      return node.grouped && node.groups.slice(1).some((group) => group.name === path[1]) ? [] : undefined;
    });
  });
}
export function removeAggregateGroup(
  document: WorkflowDocument,
  block: BlockPath,
  id: string,
  name: string,
): WorkflowDocument {
  return updateBlock(document, block, (owner) => {
    const node = nodeAt(owner, id);
    if (node?.type !== 'aggregate' || node.groups.length < 2) throw new CommandRejected('missing_field');
    return rewriteBlockReferences(
      {
        ...owner,
        nodes: { ...owner.nodes, [id]: { ...node, groups: node.groups.filter((group) => group.name !== name) } },
      },
      (path) => (path[0] === id && path[1] === name ? [] : undefined),
    );
  });
}
