import { parentPort, workerData } from 'node:worker_threads';
import { inspectOoxml } from './ooxml.ts';
try { parentPort!.postMessage({ result: inspectOoxml(workerData.bytes, workerData.extension) }); }
catch (error) {
  const code = error instanceof Error ? error.message : '';
  parentPort!.postMessage({ error: ['too_large', 'archive_rejected', 'parser_limit'].includes(code) ? code : 'malformed' });
}
