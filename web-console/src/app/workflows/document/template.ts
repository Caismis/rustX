import { CommandRejected } from './commands';
import { fieldValue, setField, type FieldAddress } from './fields';
import type { NodeDefinition, WorkflowDocument } from './model';

type Template = Extract<NodeDefinition,{type:'template'}>;

export function templateInputName(names:readonly string[],hint='arg1'):string {
  const base=/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(hint)?hint:'arg1';
  if(!names.includes(base))return base;
  if(base==='arg1'){let index=2;while(names.includes(`arg${index}`))index++;return `arg${index}`;}
  let index=1;while(names.includes(`${base.slice(0,60)}_${index}`))index++;
  return `${base.slice(0,60)}_${index}`;
}

/** Dify updates its generated interpolation tokens together with the binding.
 * Other expressions and text follow Dify's direct-token rename behavior. */
export function renameTemplateInput(document:WorkflowDocument,address:FieldAddress,from:string,to:string):WorkflowDocument {
  if(!/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(to))throw new CommandRejected('invalid_identifier');
  const variables=fieldValue(document,address)?.value as Template['variables'];
  if(Object.hasOwn(variables,to))throw new CommandRejected('duplicate_identifier');
  const sourceAddress={...address,path:[...address.path.slice(0,-1),'template']};
  const source=fieldValue(document,sourceAddress)?.value as string;
  return setField(setField(document,address,Object.fromEntries(Object.entries(variables).map(([key,value])=>[key===from?to:key,value]))),sourceAddress,source.replaceAll(`{{ ${from} }}`,`{{ ${to} }}`));
}
