import type { AttemptIdentitySource } from './v57.ts';

/** Closed immutable evidence, independent of object property order or transcript placement. */
export function attemptIdentitySourceKey(source: AttemptIdentitySource): string {
  switch(source.type){
    case 'journal': return JSON.stringify([source.type,source.domain,source.field.sequence,source.field.path,source.field.encoded_bytes]);
    case 'inherited': return JSON.stringify([source.type,source.kind,source.ordinal,source.domain,source.encoded_bytes]);
  }
}

/** Original owner CID is independent of the selected read source's CID. */
export function attemptOwnerKey(owner: {conversation_id:string;attempt_id?:string|null;attempt_id_source?:AttemptIdentitySource|null}): string {
  if(owner.attempt_id!=null)return JSON.stringify([owner.conversation_id,owner.attempt_id]);
  if(owner.attempt_id_source)return JSON.stringify([owner.conversation_id,['source',attemptIdentitySourceKey(owner.attempt_id_source)]]);
  throw new Error('Native attempt owner identity is unavailable.');
}

export function attemptIdentityField(source: AttemptIdentitySource) {
  return {domain:source.domain,path:source.type==='journal'?source.field.path:['attempt_id'],
    encodedBytes:source.type==='journal'?source.field.encoded_bytes:source.encoded_bytes};
}
