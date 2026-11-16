import { getNodeValue, parseTree, type ParseError } from 'jsonc-parser';
import { modelToolCall } from '../../../../protocol/app-server/model-tool-call';
import type { TracePreview } from '../../../../protocol/app-server/v78';

/** Business input for a semantic ledger cell, from the bounded native preview.
 * Slice the original node so opaque number tokens and truncated prefixes stay
 * exact. Raw inspection continues to read the unchanged complete envelope. */
export function modelToolArgumentPreview(preview: TracePreview | null | undefined): string | undefined {
  if (!preview) return;
  const errors: ParseError[] = [];
  const tree = parseTree(preview.text, errors, { disallowComments: true });
  if (!tree || tree.type !== 'object' || errors.length && !preview.truncated) return;
  // A duplicate outer arguments key has no unambiguous display projection.
  // Validate and select the same single property, never first/last key mixes.
  const fields = tree.children?.filter(property => property.children?.[0]?.value === 'arguments');
  if (fields?.length !== 1 || !modelToolCall(getNodeValue(tree))) return;
  const business = fields[0]?.children?.[1];
  if (business?.type !== 'object') return;
  return preview.text.slice(business.offset, business.offset + business.length);
}
