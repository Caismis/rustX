import { FILE_PREVIEW_LIMITS as L, isFileData, type FileBytes, type FileReadOptions } from '../../shared/file-preview';
import { WorkspaceHostError } from '../workspaces/host';
import { hasRoughArea, isStepSource, stepPointSamples, type GeometryRoughPreview } from './geometry-rough';

/** Reuse authorized range reads. A complete, single-version copy is published
 * only after every bounded physical read has settled. No whole-file HTTP body,
 * array-of-chunks retention or final concatenation is needed. */
export async function readGeometrySource(read: (options: FileReadOptions, signal: AbortSignal) => Promise<FileBytes>, signal: AbortSignal, rough?: (value: GeometryRoughPreview) => void) {
  let data: Uint8Array<ArrayBuffer> | undefined, version = '', total = 0, offset = 0;
  let lastYield = performance.now();
  const entered=lastYield;
  const measure=(name:string)=>{performance.clearMeasures(name);performance.measure(name,{start:entered});};
  const yieldTask = () => new Promise<void>(resolve => {
    const channel = new MessageChannel();let done=false;
    const finish=()=>{if(done)return;done=true;signal.removeEventListener('abort',finish);channel.port1.close();channel.port2.close();resolve();};
    signal.addEventListener('abort',finish,{once:true});channel.port1.onmessage=finish;
    if(signal.aborted)finish();else channel.port2.postMessage(null);
  });
  const yieldPaint = () => new Promise<void>(resolve => {
    let frame=0,done=false;
    const finish=()=>{if(done)return;done=true;signal.removeEventListener('abort',finish);cancelAnimationFrame(frame);resolve();};
    signal.addEventListener('abort',finish,{once:true});
    if(signal.aborted)finish();else frame=requestAnimationFrame(()=>{void yieldTask().then(finish);});
  });
  const validatePage = (page: FileBytes, offset: number, length: number) => {
    if (!isFileData(page.data) || !Number.isSafeInteger(page.bytes) || page.bytes < 0 || page.bytes > L.geometryBytes
      || page.lines !== undefined || page.offset !== offset || typeof page.version !== 'string' || !page.version
      || page.data.length !== Math.min(length, Math.max(0, page.bytes - offset))
      || page.eof !== (offset + page.data.length === page.bytes)) throw new Error(page.bytes > L.geometryBytes ? 'too_large' : 'malformed');
  };
  do {
    signal.throwIfAborted();
    const length = data ? Math.min(L.pageBytes, total - offset) : rough ? 32768 : L.pageBytes;
    const page = await read({ kind: 'bytes', range: { offset, length } }, signal);
    signal.throwIfAborted();
    validatePage(page, offset, length);
    if (!data) {
      total = page.bytes; version = page.version;
      let completed:Uint8Array<ArrayBuffer>|undefined;
      if (rough && isStepSource(page.data)) {
        measure('geometry:rough-prefix');
        const points = stepPointSamples(page.data);
        let displayed=false,publishedCount=0;
        const publish=async()=>{
          if(points.length===publishedCount||!hasRoughArea(points))return;
          publishedCount=points.length;
          rough({version,positions:new Float32Array(points)});
          if(!displayed){displayed=true;measure('geometry:rough-source-ready');if(typeof requestAnimationFrame==='function')await yieldPaint();else await yieldTask();signal.throwIfAborted();lastYield=performance.now();}
        };
        await publish();
        if(!page.eof && total<=L.pageBytes){
          const at=page.data.length,length=total-at,sample=await read({kind:'bytes',range:{offset:at,length}},signal);signal.throwIfAborted();validatePage(sample,at,length);
          if(sample.version!==version||sample.bytes!==total)throw new WorkspaceHostError('Geometry source changed during sampling','source_changed');
          completed=new Uint8Array(total);completed.set(page.data);completed.set(sample.data,at);points.push(...stepPointSamples(sample.data));
        }
        else if (!page.eof) for (const ratio of [.18, .44, .7, .98]) {
          signal.throwIfAborted(); const at = Math.min(total - 1, Math.floor(total * ratio)), length = Math.min(32768, total - at);
          const sample = await read({ kind: 'bytes', range: { offset: at, length } }, signal); signal.throwIfAborted(); validatePage(sample, at, length);
          if (sample.version !== version || sample.bytes !== total) throw new WorkspaceHostError('Geometry source changed during sampling', 'source_changed');
          points.push(...stepPointSamples(sample.data));await publish();
        }
        await publish();
      }
      if(completed)return{data:completed,version};
      if (page.eof) return { data: page.data, version };
      data = new Uint8Array(total);
    } else if (version !== page.version || total !== page.bytes) throw new WorkspaceHostError('Geometry source changed during read', 'source_changed');
    data.set(page.data, offset); offset += page.data.length;
    if (rough && performance.now() - lastYield >= 4) { await yieldTask(); signal.throwIfAborted(); lastYield = performance.now(); }
  } while (offset < total);
  return { data: data!, version };
}
