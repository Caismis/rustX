/**
 * Runs Save under one umask, in its own process, and prints the mode bits it
 * observed as JSON. The umask is process-global, so the delivery tests change
 * it only here, never in the test runner itself.
 *
 * Usage: node save-under-umask.ts <octal umask> <directory>
 */

import { existsSync, lstatSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { DeliveryUncertainError, SAVE_FILES, saveDelivery, type SaveFiles } from "../../src/app-server/delivery-files.ts";

const [mask, dir] = process.argv.slice(2);
if (mask === undefined || dir === undefined) throw new Error("usage: save-under-umask.ts <umask> <directory>");
process.umask(Number.parseInt(mask, 8));

const BODY = { data: Buffer.alloc(200 * 1024, 0x5a).toString("base64") };
const mode = (path: string) => lstatSync(path).mode & 0o777;

/** Real Save files, observing the staged file's mode at each named boundary. */
function observed(seen: Record<string, number>, over: Partial<SaveFiles> = {}, onSync?: () => void): SaveFiles {
  let staged = "";
  return {
    ...SAVE_FILES,
    open: async (path, flags, requested) => {
      staged = path;
      const handle = await SAVE_FILES.open(path, flags, requested);
      const write = handle.write.bind(handle) as (buffer: Uint8Array, offset: number, length: number) => Promise<{ bytesWritten: number }>;
      const sync = handle.sync.bind(handle);
      Object.assign(handle, {
        write: (buffer: Uint8Array, offset: number, length: number) => {
          seen.midWrite ??= mode(staged);
          return write(buffer, offset, length);
        },
        sync: async () => {
          await sync();
          seen.synced = mode(staged);
          onSync?.();
        },
      });
      return handle;
    },
    link: async (existing, path) => {
      seen.beforeLink = mode(existing);
      return SAVE_FILES.link(existing, path);
    },
    ...over,
  };
}

// The umask in force: what an ordinary 0666 creation gets.
writeFileSync(join(dir, "control"), "", { mode: 0o666 });
const result: Record<string, unknown> = { control: mode(join(dir, "control")) };

const published: Record<string, number> = {};
const saved = await saveDelivery(async () => BODY, join(dir, "published"), undefined, observed(published));
result.published = { ...published, destination: mode(saved.path) };

const cancelled: Record<string, number> = {};
const abort = new AbortController();
await saveDelivery(async () => BODY, join(dir, "cancelled"), abort.signal, observed(cancelled, {}, () => abort.abort()))
  .then(() => { throw new Error("a cancelled save published"); }, (error: unknown) => {
    if (error !== abort.signal.reason) throw error;
  });
result.cancelled = { ...cancelled, destinationExists: existsSync(join(dir, "cancelled")) };

const residue: Record<string, number> = {};
const kept = await saveDelivery(async () => BODY, join(dir, "residue"), undefined, observed(residue, {
  link: async () => { throw Object.assign(new Error("EACCES: injected"), { code: "EACCES" }); },
  unlink: async () => { throw Object.assign(new Error("EIO: injected"), { code: "EIO" }); },
})).then(() => { throw new Error("a failed link published"); }, (error: unknown) => {
  if (!(error instanceof DeliveryUncertainError) || error.residue === undefined) throw error;
  return { residue: error.residue };
});
result.residue = { ...residue, kept: mode(kept.residue), destinationExists: existsSync(join(dir, "residue")) };

process.stdout.write(JSON.stringify(result));
