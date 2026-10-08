/**
 * Client-local file actions on committed deliveries.
 *
 * This is the one TUI owner of local filesystem effects for `/files`. Neither
 * renderers nor command definitions touch the filesystem; they dispatch to
 * these functions with an explicit destination or an explicit open request.
 *
 * ```text
 * Save   native delivery/read (authorized, ≤ 512 KiB) -> bounded decode
 *        -> new client-local file ('wx': never truncates existing data)
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
import { lstat, open, unlink } from "node:fs/promises";
import { homedir } from "node:os";
import { resolve } from "node:path";

import type { DeliveryLocation } from "../presentation/deliveries.ts";

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
 * A client-local save destination: `~` expands to this user's home, relative
 * paths resolve against the TUI's own cwd. An empty argument means the
 * delivered file's own name in that cwd.
 */
export function deliveryDestination(name: string, argument: string, cwd = process.cwd(), home = homedir()): string {
  const input = argument.trim() || name;
  if (input.includes("\0") || /[\r\n]/.test(input)) throw new DeliveryActionError("Invalid save destination");
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

/**
 * Reads authorized original bytes and writes them to a new client-local file.
 *
 * Existing data is never truncated. A file this function created is removed
 * again when the write fails or the operation is cancelled before it
 * completes, so no partial output is left behind.
 */
export async function saveDelivery(
  read: () => Promise<{ data: string }>,
  destination: string,
  signal?: AbortSignal,
): Promise<string> {
  signal?.throwIfAborted();
  const bytes = decodeDelivery((await read()).data);
  signal?.throwIfAborted();
  let file: Awaited<ReturnType<typeof open>> | undefined;
  try {
    file = await open(destination, "wx", 0o666);
    let offset = 0;
    while (offset < bytes.length) {
      signal?.throwIfAborted();
      const { bytesWritten } = await file.write(bytes, offset, bytes.length - offset);
      if (bytesWritten === 0) throw new DeliveryActionError("Save destination stopped accepting bytes");
      offset += bytesWritten;
    }
    await file.sync();
    signal?.throwIfAborted();
    await file.close();
    file = undefined;
    return destination;
  } catch (error) {
    if (file) {
      await file.close().catch(() => {});
      try { await unlink(destination); }
      catch (cleanup) { throw new AggregateError([error, cleanup], `Save failed; partial output remains at ${destination}`); }
    }
    throw error;
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
  options.signal?.throwIfAborted();
  const status = await (options.launch ?? launchOpener)(opener, location.path);
  if (status !== 0) throw new DeliveryActionError(`${opener} did not accept the open request (exit ${status ?? "signal"})`);
  return { path: location.path, opener };
}
