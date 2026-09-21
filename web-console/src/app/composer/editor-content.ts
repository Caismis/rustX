import type { UserInputBlock } from '../../../../protocol/app-server/v16';

/** The flat Web editor represents uploads* followed by at most one nonempty
 * text block. Reject all other native shapes before decomposing, never reorder
 * or merge them. Retry bypasses this editor and submits native blocks directly. */
export function editableContent(content: readonly UserInputBlock[]): boolean {
  return content.every((block, index) => block.type === 'upload'
    || index === content.length - 1 && block.text.length > 0);
}
