import { pathToFileURL } from 'node:url';
import { join } from 'node:path';
import { mkdtemp, rm } from 'node:fs/promises';
const { openNodeSqliteDatabase } = await import(pathToFileURL(join(process.env.REFERENCE_PI_CHECKOUT, 'packages/durable/src/storage/sqlite/node.ts')).href);
const directory = await mkdtemp('/tmp/reference-growth-pragmas-');
const database = await openNodeSqliteDatabase(join(directory, 'config.db'));
const pragmas = {};
for (const name of ['journal_mode', 'synchronous', 'page_size', 'wal_autocheckpoint', 'busy_timeout']) {
  pragmas[name] = await database.get('PRAGMA ' + name);
}
await database.close();
console.log(JSON.stringify({node: process.version, sqlite: process.versions.sqlite, pragmas}, null, 2));
await rm(directory, {recursive: true});
