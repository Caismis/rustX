// Decode all native Zstd frames after close, outside runtime measurements.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
const [checkout, path] = process.argv.slice(2);
const { scanZstdFrames, decompressZstdFrame } = await import(pathToFileURL(join(checkout, 'packages/session/session-persistence-jsonl/src/zstd.ts')).href);
const bytes = await readFile(path);
const scan = scanZstdFrames(bytes);
assert.equal(scan.tornStart, undefined, 'closed native log has a torn Zstd frame');
for (const frame of scan.frames) process.stdout.write(await decompressZstdFrame(bytes.subarray(frame.start, frame.end)));
