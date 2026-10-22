import { createContext } from 'react';
import type { AttachmentTarget, ChangesSummary } from '../../../protocol/app-server/v52';
import type { AppServerClient } from '../client/app-server';

export interface ChangesReviewRequest {
  client:AppServerClient;
  target:AttachmentTarget;
  attemptId:string;
  summary:ChangesSummary;
  index:number;
  owned:()=>boolean;
}
export const ChangesReviewContext = createContext<((request:ChangesReviewRequest)=>void) | undefined>(undefined);
