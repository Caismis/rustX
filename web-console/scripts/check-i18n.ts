import { readdirSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { findCopy } from './i18n-audit.ts';
const root = resolve(import.meta.dirname, '../src');
let count = 0;
for (const file of readdirSync(root, { recursive: true }).map(String).filter(file => /\.tsx?$/.test(file) && !file.startsWith('locale/'))) {
  for (const violation of findCopy(file, readFileSync(resolve(root, file), 'utf8'))) {
    console.error(`${file}:${violation.line}: untranslated ${violation.kind}: ${violation.text}`);
    count++;
  }
}
if (count) { console.error(`${count} untranslated presentation literals. Move product copy to a typed feature dictionary; exempt only exact opaque tokens with an adjacent i18n-raw reason.`); process.exitCode = 1; }
else console.log('Verified product copy ownership in WebUI TS/TSX presentation positions');
