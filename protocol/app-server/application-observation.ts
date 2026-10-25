import type { BoundedSessionApplication, ConfigurationApplication, SessionApplicationRef, SessionApplicationChunk } from './v57.ts';
import { materializeFields } from './field-observation.ts';
/** One configuration-owner cut. Reading details never adopts a configuration. */
export async function materializeApplication(view:BoundedSessionApplication|null|undefined,read:(reference:SessionApplicationRef,offset:number)=>Promise<SessionApplicationChunk>,current:()=>boolean):Promise<ConfigurationApplication|undefined> {
 if(!view)return undefined;
 if(!current()||view.sources.length>32||view.reference&&view.reference.version!==view.version)throw new Error('Configuration application cut changed.');
 if(!view.inline&&!view.reference)throw new Error('Configuration details are unavailable.');
 const application=view.inline??(await materializeFields<{application:ConfigurationApplication},SessionApplicationRef>({revision:view.reference!.revision,fields:[{name:'application',reference:view.reference}]},read,current)).application;
 if(application.scope!==view.scope||application.version!==view.version||JSON.stringify(application.sources)!==JSON.stringify(view.sources))throw new Error('Configuration application identity changed.');
 return application;
}
