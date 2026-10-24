import type { AttachmentTarget } from './v57.ts';

/** Whether two attachment targets name the same attachment in every domain:
 * Session, Conversation, runtime incarnation and attachment. An absent
 * target names no attachment, so it never equals another. */
export function sameTarget(left: AttachmentTarget | undefined, right: AttachmentTarget | undefined): boolean {
  return left !== undefined && right !== undefined
    && left.session_id === right.session_id
    && left.conversation_id === right.conversation_id
    && left.runtime_incarnation === right.runtime_incarnation
    && left.attachment_id === right.attachment_id;
}
