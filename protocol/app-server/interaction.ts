import type {CheckpointPage, CheckpointPageItem, RoutedInteraction, RuntimeClientInteractionHeader} from './v57.ts';
import {domainItems, type DomainReader} from './domain-observation.ts';

/** A deferred display header is actionable identity, never a fabricated body. */
export function interactionBody(header:RuntimeClientInteractionHeader):RoutedInteraction|undefined {
  if(!header.body_complete||!header.body)return;
  const result={interaction:header.interaction,source:header.source,request:header.body,...(header.workflow_node == null ? {} : {workflow_node:header.workflow_node})};
  validateInteractionBody(header,result);
  return result;
}
function comparable(value:unknown):unknown {
  if(Array.isArray(value))return value.map(comparable);
  if(value&&typeof value==='object')return Object.fromEntries(Object.entries(value).filter(([,item])=>item!==undefined).sort(([a],[b])=>a.localeCompare(b)).map(([key,item])=>[key,comparable(item)]));
  return value;
}
export function validateInteractionBody(header:RuntimeClientInteractionHeader,body:RoutedInteraction) {
  if(JSON.stringify(comparable(body.interaction))!==JSON.stringify(comparable(header.interaction))||JSON.stringify(comparable(body.source))!==JSON.stringify(comparable(header.source))
    ||body.request.id!==header.request_id||body.request.conversation_id!==header.conversation_id
    ||JSON.stringify(comparable(body.request.execution))!==JSON.stringify(comparable(header.execution))||body.request.kind.type!==header.kind)
    throw new Error('Interaction body identity or revision evidence changed.');
}
/** Load only the selected body; pagination does not materialize every descriptor. */
export async function readInteractionBody(page:CheckpointPage,item:CheckpointPageItem,header:RuntimeClientInteractionHeader,reader:DomainReader) {
  const [body]=await domainItems<RoutedInteraction>({...page,items:[{...item,value:null,deferred:true}],next_offset:null},reader);
  if(!body)throw new Error('Interaction body is unavailable.');
  validateInteractionBody(header,body);
  return body;
}
