import type { BoundedEffectiveConfiguration, EffectiveConfiguration, EffectiveFieldRef, EffectiveValueChunk } from './v56.ts';
import { materializeFields } from './field-observation.ts';
export function materializeEffective(view:BoundedEffectiveConfiguration,read:(reference:EffectiveFieldRef,offset:number)=>Promise<EffectiveValueChunk>,current:()=>boolean):Promise<EffectiveConfiguration> {
  return materializeFields(view,read,current);
}
