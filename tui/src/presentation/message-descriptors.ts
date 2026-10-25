import { attemptOwnerKey } from '../../../protocol/app-server/attempt-identity.ts';
import { canonicalLocatorKey } from '../../../protocol/app-server/content-identity.ts';
import { messageDescriptor } from '../../../protocol/app-server/message-descriptor.ts';
import type { AttemptIdentitySource, RuntimeClientMessageDescriptor } from '../../../protocol/app-server/v57.ts';
import type { PresentationState } from './state.ts';

/** Original deferred rows retain their native facts; an exact current head replaces its cache. */
export function messageDescriptors(state: Pick<PresentationState,'messageDescriptors'|'deferredTranscriptEntries'>) {
  const descriptors=new Map<string,RuntimeClientMessageDescriptor>();
  for(const row of state.deferredTranscriptEntries){
    const descriptor=messageDescriptor(row.value);
    if(descriptor)descriptors.set(canonicalLocatorKey(descriptor.locator),descriptor);
  }
  for(const descriptor of state.messageDescriptors)descriptors.set(canonicalLocatorKey(descriptor.locator),descriptor);
  return [...descriptors.values()];
}


/** A finite set of original selected identity fields, never a semantic owner mirror. */
export function deferredAttemptIdentities(state: PresentationState) {
  const fields=new Map<string,{owner:string;source:AttemptIdentitySource}>();
  const owners=[...(state.attempt?[{conversation_id:state.conversationId,attempt_id:state.attempt.attemptId,attempt_id_source:state.attempt.attemptSource}]:[]),...state.transcript.flatMap(entry=>entry.kind==='committed'?[entry.turnProcess,entry.completedResponse?.origin]:entry.kind==='attempt_terminal'?[entry.turn]:[]),
    ...messageDescriptors(state).flatMap(descriptor=>[descriptor.turn_process,descriptor.completed_response?.origin])];
  for(const owner of owners)if(owner?.attempt_id_source){
    const key=attemptOwnerKey(owner);fields.set(key,{owner:key,source:owner.attempt_id_source});
  }
  return [...fields.values()];
}
