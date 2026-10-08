/**
 * Client-local file actions on committed deliveries.
 *
 * This is the one TUI owner of local filesystem effects for `/files`. Neither
 * renderers nor command definitions touch the filesystem; they dispatch to
 * these functions with an explicit destination or an explicit open request.
 *
 * ```text
 * Save   native delivery/read (authorized, ≤ 512 KiB, cancellable) -> bounded decode
 *        -> new client-local file ('wx': never truncates existing data)
 *        -> local publication commit after sync + close
 * Open   native delivery/locate (authorized, verified leaf identity)
 *        -> this TUI spawned the server  AND  local lstat(dev, ino) matches
 *        -> argv-only OS opener; success means the opener accepted the request
 * ```
 *
 * A remote server's path is never interpreted as a client path: Open is
 * offered only for the stdio child this TUI spawned, and even then the leaf
 * device/inode must match before anything is launched. Path spelling and
 * loopback addresses are not evidence of a shared filesystem.
 */

import { spawn } from "node:child_process";
import { lstat, open, unlink, type FileHandle } from "node:fs/promises";
import { homedir } from "node:os";
import { resolve } from "node:path";

import type { DeliveryLocation } from "../presentation/deliveries.ts";
import { isRenderableField } from "../sanitize.ts";

/** The native Session-file read limit, inclusive. */
export const DELIVERY_MAX_BYTES = 512 * 1024;
/** The base64 length of {@link DELIVERY_MAX_BYTES}. Checked before decoding. */
export const DELIVERY_MAX_BASE64 = 699_052;

/** A typed local failure; native failures arrive as AppServerRequestError. */
export class DeliveryActionError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DeliveryActionError";
  }
}

/**
 * A client-local save destination the user typed. `~` expands to this
 * user's home and relative paths resolve against the TUI's own cwd. There is
 * no default: the delivered name is the server's identity for the file, not
 * a path this client chose.
 */
export function deliveryDestination(argument: string, cwd = process.cwd(), home = homedir()): string {
  const input = argument.trim();
  if (input === "" || !isRenderableField(input)) throw new DeliveryActionError("Invalid save destination");
  return resolve(cwd, input === "~" ? home : input.startsWith("~/") ? `${home}/${input.slice(2)}` : input);
}

/** Strict, bounded base64 decoding of one native delivery body. */
export function decodeDelivery(data: string): Uint8Array {
  if (data.length > DELIVERY_MAX_BASE64 || data.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(data)) {
    throw new DeliveryActionError("Delivery body is not a bounded base64 payload");
  }
  const bytes = Buffer.from(data, "base64");
  if (bytes.length > DELIVERY_MAX_BYTES) throw new DeliveryActionError("Delivered file exceeds 512 KiB");
  return new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.length);
}

/** A save failure that left this operation's partial output in place. */
export class DeliveryResidueError extends DeliveryActionError {
  readonly path: string;
  constructor(path: string, cause: unknown) {
    super(`Save failed and its partial output remains at ${path}`);
    this.path = path;
    this.name = "DeliveryResidueError";
    this.cause = cause;
  }
}

/** Bytes per write; cancellation is observed between chunks. */
export const SAVE_CHUNK_BYTES = 64 * 1024;

/** The filesystem seam of {@link saveDelivery}; tests interpose on writes. */
export interface SaveFiles {
  open: (path: string, flags: "wx", mode: number) => Promise<FileHandle>;
}
const NODE_FILES: SaveFiles = { open };

/**
 * Reads authorized original bytes and writes them to a new client-local file.
 *
 * ```text
 * read (cancellable on the server) -> abort? -> create 'wx' (never truncates)
 *   -> chunked writes, abort? between chunks -> sync -> close
 *   -> abort?  <- local publication commit: resolving means saved
 * ```
 *
 * Cancellation observed before the commit leaves no file of this operation
 * behind: the created file is removed only while the destination is still
 * the device/inode this operation created, never a file that replaced it.
 * After the commit the save stands, whatever is cancelled later.
 */
export async function saveDelivery(
  read: () => Promise<{ data: string }>,
  destination: string,
  signal?: AbortSignal,
  files: SaveFiles = NODE_FILES,
): Promise<string> {
  signal?.throwIfAborted();
  const bytes = decodeDelivery((await read()).data);
  signal?.throwIfAborted();
  const file = await files.open(destination, "wx", 0o666);
  let unclosed = true;
  let created: { dev: bigint; ino: bigint } | undefined;
  try {
    created = await file.stat({ bigint: true });
    for (let offset = 0; offset < bytes.length;) {
      signal?.throwIfAborted();
      const length = Math.min(SAVE_CHUNK_BYTES, bytes.length - offset);
      const { bytesWritten } = await file.write(bytes, offset, length);
      if (bytesWritten === 0) throw new DeliveryActionError("Save destination stopped accepting bytes");
      offset += bytesWritten;
    }
    await file.sync();
    unclosed = false;
    await file.close();
    signal?.throwIfAborted();
    return destination;
  } catch (error) {
    if (unclosed) await file.close().catch(() => {});
    if (!await removeOwned(destination, created)) throw new DeliveryResidueError(destination, error);
    throw error;
  }
}

/**
 * Removes `path` only while it is still the file this operation created.
 * True when nothing of this operation remains under that name.
 */
async function removeOwned(path: string, created: { dev: bigint; ino: bigint } | undefined): Promise<boolean> {
  const current = await lstat(path, { bigint: true }).catch(() => undefined);
  if (current === undefined) return true;
  if (created === undefined) return false;
  if (current.dev !== created.dev || current.ino !== created.ino) return true;
  try {
    await unlink(path);
    return true;
  } catch {
    return false;
  }
}

/** The OS opener this platform provides, or undefined. */
export function systemOpener(platform = process.platform): string | undefined {
  return platform === "darwin" ? "open" : platform === "linux" || platform === "freebsd" ? "xdg-open" : undefined;
}

/** Launches one opener with argv only. Resolves its exit status; never a shell. */
export type Opener = (command: string, path: string) => Promise<number | null>;
export const launchOpener: Opener = (command, path) => new Promise((resolveExit, reject) => {
  const child = spawn(command, [path], { stdio: "ignore", shell: false });
  child.once("error", reject);
  child.once("exit", (code) => resolveExit(code));
});

/** What an open request established. Never "the application opened". */
export interface OpenRequested {
  path: string;
  opener: string;
}

/**
 * Opens a delivery's original file locally, only on a demonstrably shared
 * filesystem: the caller asserts it spawned the server (`sharedHost`), and
 * the leaf the native side verified must be this client's own `lstat` leaf.
 */
export async function openDelivery(
  locate: () => Promise<DeliveryLocation>,
  options: { sharedHost: boolean; opener?: string; launch?: Opener; signal?: AbortSignal },
): Promise<OpenRequested> {
  if (!options.sharedHost) {
    throw new DeliveryActionError("Open is unavailable: this App Server's files are not on this machine; use Save");
  }
  const opener = options.opener;
  if (opener === undefined) throw new DeliveryActionError("Open is unavailable: no system opener on this platform");
  options.signal?.throwIfAborted();
  const location = await locate();
  options.signal?.throwIfAborted();
  const local = await lstat(location.path, { bigint: true }).catch(() => undefined);
  if (
    local === undefined ||
    !local.isFile() ||
    local.dev.toString() !== location.device ||
    local.ino.toString() !== location.inode
  ) {
    throw new DeliveryActionError("Open is unavailable: the local path is not the delivered file");
  }
  // The launch commit: once the opener is spawned the request belongs to the
  // OS, and a later cancellation does not withdraw or misreport it.
  options.signal?.throwIfAborted();
  const status = await (options.launch ?? launchOpener)(opener, location.path);
  if (status !== 0) throw new DeliveryActionError(`${opener} did not accept the open request (exit ${status ?? "signal"})`);
  return { path: location.path, opener };
}
