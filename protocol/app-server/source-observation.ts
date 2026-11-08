import type { BoundedSourceSettings, SourceSettings, SourceFieldRef, SourceValueChunk } from './v77.ts';
import { materializeFields } from './field-observation.ts';
const sourceCuts = new WeakMap<SourceSettings, BoundedSourceSettings>();
export function sourceObservation(source: SourceSettings) { return sourceCuts.get(source); }
/** Application settlement may advance between a bounded projection and its
 * deferred fields. Restart the complete read cut, never mix revisions or
 * replay a mutation. Continuous changes still surface as a read failure. */
export async function readSourceSettings(capture:()=>Promise<BoundedSourceSettings>,read:(reference:SourceFieldRef,offset:number)=>Promise<SourceValueChunk>,current:()=>boolean):Promise<SourceSettings> {
  for(let attempt=0;;attempt++) {
    if(!current())throw new Error('Configuration view was replaced.');
    try { return await materializeSource(await capture(),read,current); }
    catch(error) {
      const kind=(error as {error?:{data?:{kind?:string}}}|null)?.error?.data?.kind;
      if(kind!=='read_cut_expired'||attempt===2||!current())throw error;
    }
  }
}
/** Finite configuration presentation cache; source revisions remain native-owned. */
export async function materializeSource(view: BoundedSourceSettings, read: (reference: SourceFieldRef, offset: number) => Promise<SourceValueChunk>, current: () => boolean): Promise<SourceSettings> {
  const source=await materializeFields<SourceSettings,SourceFieldRef>(view,read,current);
  if(JSON.stringify(source.target)!==JSON.stringify(view.target))throw new Error('Configuration target identity changed.');
  sourceCuts.set(source,view);
  return source;
}

export type SourceRepairRead = Pick<SourceSettings, 'target' | 'user_repair' | 'workspace_repair' | 'repair_sequence'>;
/** Repair recovery reads only the finite repair owners, never unrelated configuration bodies. */
export async function readSourceRepairFields(view: BoundedSourceSettings, read: (reference: SourceFieldRef, offset: number) => Promise<SourceValueChunk>, current: () => boolean): Promise<SourceRepairRead> {
  if (view.inline && (view.inline.target.kind !== view.target.kind || view.inline.target.kind === 'workspace' && (view.target.kind !== 'workspace' || view.inline.target.directory !== view.target.directory))) throw new Error('Configuration target identity changed.');
  const names = new Set(['user_repair', 'workspace_repair', 'repair_sequence']);
  const fields = view.inline
    ? { user_repair: view.inline.user_repair, workspace_repair: view.inline.workspace_repair, repair_sequence: view.inline.repair_sequence }
    : await materializeFields<Omit<SourceRepairRead, 'target'>, SourceFieldRef>({ revision: view.revision, fields: view.fields.filter(field => names.has(field.name)) }, read, current);
  if (!current()) throw new Error('Configuration view was replaced.');
  if (typeof fields.repair_sequence !== 'string' || !/^(0|[1-9][0-9]*)$/.test(fields.repair_sequence)) throw new Error('Configuration repair sequence is unavailable.');
  return { target: view.target, ...fields };
}
