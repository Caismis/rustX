import { createContext } from 'react';
import type { AttachmentTarget, ChangesSummary } from '../../../protocol/app-server/v59';
import type { AppServerClient } from '../client/app-server';
import { sameTarget } from '../../../protocol/app-server/attachment-identity';

export interface ChangesReviewRequest {
  client:AppServerClient;
  target:AttachmentTarget;
  attemptId:string;
  summary:ChangesSummary;
  index:number;
  owned:()=>boolean;
}
/** A sidebar review belongs to its native attachment, independently of which
 * Chat/Trace/child view currently renders the main conversation surface. */
export function changesReviewOwner(client:AppServerClient,target:AttachmentTarget) {
  const observation=client.getSnapshot().views[target.session_id]?.attachmentObservation;
  return ()=>sameTarget(observation?.target,target) && client.isAttachmentObservationCurrent(target.session_id,observation);
}
export const ChangesReviewContext = createContext<((request:ChangesReviewRequest)=>void) | undefined>(undefined);
