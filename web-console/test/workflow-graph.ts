import { connect, disconnect } from '../src/app/workflows/document/commands';
import { blockAt, edgesOf, startOf, type BlockPath, type WorkflowDocument } from '../src/app/workflows/document/model';

/** Author the first connection in a fixture using the real Start node. */
export function connectStart(document: WorkflowDocument, path: BlockPath, target: string): WorkflowDocument {
  const block = blockAt(document, path)!, from = startOf(block)!;
  const indexes = edgesOf(block).flatMap((edge, index) => edge.from === from ? [index] : []);
  return connect(disconnect(document, path, indexes), path, from, 'next', target);
}
