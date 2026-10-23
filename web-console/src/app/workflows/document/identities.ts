import { childSteps, type Block, type BlockPath, type WorkflowDocument } from './model';

/** A field holding a typed native identity the protocol cannot even carry
 * yet (an unchosen Agent profile, an empty Tool source). Such a document is
 * not sent: native would refuse it as malformed parameters before checking
 * anything, which would hide the field. Native rules decide everything else. */
export interface IncompleteIdentity {
  readonly block: BlockPath;
  readonly node: string;
  readonly field: 'profile' | 'selector.source_id';
}

/** The native named-Agent identity rule (`SubagentName`). */
const PROFILE = /^[a-z][a-z0-9_-]{0,63}$/;

export function incompleteIdentities(document: WorkflowDocument): IncompleteIdentity[] {
  const found: IncompleteIdentity[] = [];
  const walk = (block: Block, path: BlockPath) => {
    for (const [id, node] of Object.entries(block.nodes)) {
      if (node.type === 'agent' && !PROFILE.test(node.profile)) found.push({ block: path, node: id, field: 'profile' });
      if (node.type === 'tool' && node.selector.origin === 'source' && !node.selector.source_id) found.push({ block: path, node: id, field: 'selector.source_id' });
      for (const step of childSteps(id, node)) {
        const child = step.branch === undefined
          ? (node.type === 'loop' ? node.body : undefined)
          : (node.type === 'parallel' ? node.branches[step.branch].block : undefined);
        if (child) walk(child, [...path, step]);
      }
    }
  };
  walk(document.block, []);
  return found;
}
