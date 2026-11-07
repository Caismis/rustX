/** One Seatbelt-owned kit converter; reused by every document in its Host slot. */
import { createReadStream } from 'node:fs';
import { stat, unlink } from 'node:fs/promises';
import { createInterface } from 'node:readline';
import { once } from 'node:events';
import { pathToFileURL } from 'node:url';
const [entry, directory] = process.argv.slice(2);
const { createConverter } = await import(pathToFileURL(entry).href);
const converter = await createConverter({
  fontMetadataCacheDirectory: false,
  fontDirectories: ['/System/Library/Fonts', '/Library/Fonts'],
  timeoutMs: 60000, maxInputBytes: 50 * 1024 * 1024, maxOutputBytes: 100 * 1024 * 1024,
  maxArchiveEntries: 10000, maxUncompressedBytes: 250 * 1024 * 1024, maxImageResolution: 192,
});
const send = value => process.stdout.write(JSON.stringify(value) + '\n');
try {
  send({ ready: true, engine_pid: process.pid });
  for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
    if (line.length > 4096) throw Error('invalid request');
    const { id, extension } = JSON.parse(line);
    if (!/^[a-f0-9-]{36}$/.test(id) || !['docx', 'pptx'].includes(extension)) throw Error('invalid request');
    const output = `${directory}/${id}.pdf`;
    try {
      await converter.render({ inputPath: `${directory}/${id}.${extension}`, outputPath: output });
      const { size } = await stat(output);
      if (size < 5 || size > 100 * 1024 * 1024) throw Error('too_large');
      send({ id, length: size });
      for await (const chunk of createReadStream(output)) if (!process.stdout.write(chunk)) await once(process.stdout, 'drain');
    } finally { await unlink(output).catch(error => { if (error.code !== 'ENOENT') throw error; }); }
    send({ id, settled: true, engine_pid: process.pid });
  }
} finally { await converter.dispose(); }
