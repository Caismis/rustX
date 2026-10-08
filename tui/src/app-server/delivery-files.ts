/**
 * Client-local file actions on committed deliveries.
 *
 * This is the one TUI owner of local filesystem effects for `/files`. Neither
 * renderers nor command definitions touch the filesystem; they dispatch to
 * these functions with an explicit destination or an explicit open request.
 *
 * ```text
 * Save   native delivery/read (authorized, ≤ 512 KiB, cancellable) -> bounded decode
 *        -> private staging beside the destination -> sync + close
 *        -> link(staged, destination): atomic, never replaces an entry
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
import type { BigIntStats } from "node:fs";
import { link, lstat, mkdtemp, open, rmdir, unlink, type FileHandle } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, isAbsolute, sep } from "node:path";

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
 * A client-local save destination the user typed, spelled exactly as typed:
 * leading, trailing and inner spaces are part of a file name, so blankness
 * is the only thing whitespace decides. `~` expands to this user's home and
 * a relative path is taken relative to the TUI's own cwd, with `.` and `..`
 * left for the OS to resolve. There is no default: the delivered name is the
 * server's identity for the file, not a path this client chose.
 */
export function deliveryDestination(argument: string, cwd = process.cwd(), home = homedir()): string {
  if (argument.trim() === "" || !isRenderableField(argument)) throw new DeliveryActionError("Invalid save destination");
  const path = argument === "~" ? home : argument.startsWith("~/") ? childPath(home, argument.slice(2)) : argument;
  return isAbsolute(path) ? path : childPath(cwd, path);
}

/**
 * `name` inside `directory`, by concatenation only. `path.join`/`resolve`
 * fold `..` lexically, which names a different directory than the OS does
 * when the preceding component is a symlink; every Save path is built here.
 */
function childPath(directory: string, name: string): string {
  return directory.endsWith(sep) ? directory + name : directory + sep + name;
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

/** What a committed Save established. */
export interface SavedDelivery {
  /** The exact destination that now names the complete file. */
  path: string;
  /** This Save's private staging directory, when it could not be removed. */
  residue?: { path: string; cause: unknown };
}

/** A Save that published nothing but could not remove its private staging. */
export class DeliveryResidueError extends DeliveryActionError {
  readonly residue: string;
  readonly cleanup: unknown;
  constructor(residue: string, cause: unknown, cleanup: unknown) {
    super(`Nothing was saved, but this save's staging directory remains at ${residue}`);
    this.name = "DeliveryResidueError";
    this.residue = residue;
    this.cleanup = cleanup;
    this.cause = cause;
  }
}

/**
 * What the destination showed after an ambiguous link failure, none of
 * which proves this save did not publish: its entry may have been created and
 * then removed or replaced by someone else.
 */
export type DestinationObservation =
  | { kind: "absent" }
  | { kind: "foreign" }
  | { kind: "uninspectable"; error: unknown };

/**
 * A Save whose dispatched link failed ambiguously, with no evidence either
 * way: the destination may or may not have been created by this save. The
 * link error is the cause; nothing retries it or touches the destination.
 */
export class DeliveryUncertainError extends DeliveryActionError {
  readonly path: string;
  readonly observed: DestinationObservation;
  readonly residue: string | undefined;
  constructor(path: string, cause: unknown, observed: DestinationObservation, residue: string | undefined) {
    const now = observed.kind === "absent" ? "it is now absent"
      : observed.kind === "foreign" ? "it now names another file"
        : `it cannot be inspected (${errorCode(observed.error) ?? "error"})`;
    super(
      `Save outcome unknown: linking ${path} failed (${errorCode(cause) ?? "error"}) and ${now}; ` +
        "this save may have created it" +
        (residue === undefined ? "" : `; its staging directory remains at ${residue}`),
    );
    this.name = "DeliveryUncertainError";
    this.path = path;
    this.observed = observed;
    this.residue = residue;
    this.cause = cause;
  }
}

/** Bytes per write; cancellation is observed between chunks. */
export const SAVE_CHUNK_BYTES = 64 * 1024;

/** The filesystem operations of {@link saveDelivery}; tests interpose on them. */
export interface SaveFiles {
  mkdtemp: (prefix: string) => Promise<string>;
  open: (path: string, flags: "wx", mode: number) => Promise<FileHandle>;
  link: (existing: string, path: string) => Promise<void>;
  lstat: (path: string, options: { bigint: true }) => Promise<BigIntStats>;
  unlink: (path: string) => Promise<void>;
  rmdir: (path: string) => Promise<void>;
}
export const SAVE_FILES: SaveFiles = { mkdtemp, open, link, lstat, unlink, rmdir };

/** Link errors meaning this filesystem cannot create the name by a hard link. */
const UNSUPPORTED_LINK = new Set(["EPERM", "ENOTSUP", "EOPNOTSUPP", "ENOSYS"]);
/**
 * Link errors that are rejections of the request itself (the name exists, a
 * path or permission check failed, the filesystem or quota refused, hard
 * links are unsupported): `link(2)` reports them without creating the entry.
 * Any other error, such as EIO, or no code at all, leaves the outcome open.
 */
const DEFINITE_REFUSAL = new Set([
  "EEXIST", "ENOENT", "ENOTDIR", "EACCES", "EROFS", "EXDEV", "ELOOP", "ENAMETOOLONG",
  "EMLINK", "ENOSPC", "EDQUOT", "EINVAL", ...UNSUPPORTED_LINK,
]);

type Publication =
  | { kind: "published" }
  | { kind: "refused"; error: unknown }
  | { kind: "uncertain"; error: unknown; observed: DestinationObservation };

/**
 * Reads authorized original bytes and publishes them as a new client-local
 * file at exactly `destination`, or leaves `destination` untouched.
 *
 * ```text
 * read (cancellable on the server) -> abort?
 *   -> mkdtemp in the destination's parent as spelled (0700, unique, owned)
 *   -> staged file 'wx' -> chunked writes, abort? between chunks -> sync -> close
 *   -> abort?   <- publication admission: the last cancellation point
 *   -> link(staged, destination)   <- publication commit (atomic, no clobber)
 *   -> unlink staged, rmdir staging   (only this save's own names)
 * ```
 *
 * The complete file appears at the destination in one step, or not at all:
 * `link` creates a new name and fails with EEXIST for any existing entry,
 * including a symlink or a directory, so an existing file, a concurrent save
 * or an external writer is never overwritten. A filesystem that cannot link
 * fails explicitly; there is no copying or rename fallback. Nothing ever
 * removes the destination: cleanup touches only the staged name and the
 * staging directory this save created, and a staging directory that is not
 * empty or cannot be removed is reported as residue, never deleted
 * recursively.
 *
 * Cancellation observed at or before admission prevents publication. The
 * admission check and the call that dispatches `link` run in one synchronous
 * step, so no abort can be observed in between; once dispatched, the link's
 * own result decides, and a later cancellation neither removes the file nor
 * reports it as unsaved.
 */
export async function saveDelivery(
  read: () => Promise<{ data: string }>,
  destination: string,
  signal?: AbortSignal,
  files: SaveFiles = SAVE_FILES,
): Promise<SavedDelivery> {
  signal?.throwIfAborted();
  const bytes = decodeDelivery((await read()).data);
  signal?.throwIfAborted();
  // The staging directory is a sibling by the destination's own spelling of
  // its parent, so the OS resolves both through the same components.
  const stage = await files.mkdtemp(childPath(dirname(destination), ".rustx-save-"));
  const staged = childPath(stage, "file");
  let created = false;
  let publication: Publication;
  try {
    const file = await files.open(staged, "wx", 0o666);
    created = true;
    const identity = await writeStaged(file, bytes, signal);
    signal?.throwIfAborted();
    publication = await publish(files, staged, destination, identity);
  } catch (error) {
    publication = { kind: "refused", error };
  }
  const cleanup = await removeStaging(files, stage, created ? staged : undefined);
  const residue = cleanup.ok ? undefined : stage;
  switch (publication.kind) {
    case "published":
      return cleanup.ok ? { path: destination } : { path: destination, residue: { path: stage, cause: cleanup.error } };
    case "uncertain":
      throw new DeliveryUncertainError(destination, publication.error, publication.observed, residue);
    case "refused":
      if (!cleanup.ok) throw new DeliveryResidueError(stage, publication.error, cleanup.error);
      throw publication.error;
  }
}

/** Writes every byte to the staged file, syncs and closes it; its identity. */
async function writeStaged(file: FileHandle, bytes: Uint8Array, signal?: AbortSignal): Promise<BigIntStats> {
  let unclosed = true;
  try {
    const identity = await file.stat({ bigint: true });
    for (let offset = 0; offset < bytes.length;) {
      signal?.throwIfAborted();
      const length = Math.min(SAVE_CHUNK_BYTES, bytes.length - offset);
      const { bytesWritten } = await file.write(bytes, offset, length);
      if (bytesWritten === 0) throw new DeliveryActionError("Save staging stopped accepting bytes");
      offset += bytesWritten;
    }
    await file.sync();
    unclosed = false;
    await file.close();
    return identity;
  } finally {
    // The operation's own failure is the one reported; the descriptor is
    // released either way.
    if (unclosed) await file.close().catch(() => {});
  }
}

/**
 * The publication commit and what a failure of it proves.
 *
 * The destination naming the staged file proves publication, whatever the
 * error (a network filesystem can fail a retransmitted link it performed).
 * Otherwise only the error can prove refusal: an absent or foreign
 * destination does not show that this link created nothing, because someone
 * may have removed or replaced the entry in between. A definite rejection is
 * a refusal; anything else is uncertain.
 */
async function publish(files: SaveFiles, staged: string, destination: string, identity: BigIntStats): Promise<Publication> {
  try {
    await files.link(staged, destination);
    return { kind: "published" };
  } catch (error) {
    let observed: DestinationObservation;
    try {
      const current = await files.lstat(destination, { bigint: true });
      if (current.dev === identity.dev && current.ino === identity.ino) return { kind: "published" };
      observed = { kind: "foreign" };
    } catch (inspection) {
      observed = errorCode(inspection) === "ENOENT" ? { kind: "absent" } : { kind: "uninspectable", error: inspection };
    }
    const code = errorCode(error);
    if (code !== undefined && DEFINITE_REFUSAL.has(code)) return { kind: "refused", error: refusal(error, destination) };
    return { kind: "uncertain", error, observed };
  }
}

function refusal(error: unknown, destination: string): unknown {
  const code = errorCode(error);
  if (code === "EEXIST") return withCause(new DeliveryActionError(`Not saved: ${destination} already exists`), error);
  if (code !== undefined && UNSUPPORTED_LINK.has(code)) {
    return withCause(
      new DeliveryActionError(`Not saved: this filesystem cannot publish ${destination} atomically without overwriting (${code})`),
      error,
    );
  }
  return error;
}

/**
 * Removes this save's staged name and its staging directory. A name that is
 * already absent counts as removed; anything else, including a directory
 * someone else added entries to, is retained and reported.
 */
async function removeStaging(
  files: SaveFiles,
  stage: string,
  staged: string | undefined,
): Promise<{ ok: true } | { ok: false; error: unknown }> {
  try {
    if (staged !== undefined) await absentIsDone(files.unlink(staged));
    await absentIsDone(files.rmdir(stage));
    return { ok: true };
  } catch (error) {
    return { ok: false, error };
  }
}

async function absentIsDone(removal: Promise<void>): Promise<void> {
  try {
    await removal;
  } catch (error) {
    if (errorCode(error) !== "ENOENT") throw error;
  }
}

function errorCode(error: unknown): string | undefined {
  const code = (error as { code?: unknown } | null)?.code;
  return typeof code === "string" ? code : undefined;
}

function withCause<E extends Error>(error: E, cause: unknown): E {
  error.cause = cause;
  return error;
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
  let local: BigIntStats;
  try {
    local = await lstat(location.path, { bigint: true });
  } catch (error) {
    // Only a known absence means "not the delivered file"; any other failure
    // is reported as what it is.
    if (errorCode(error) === "ENOENT") throw new DeliveryActionError("Open is unavailable: the local path is not the delivered file");
    throw withCause(new DeliveryActionError(`Open is unavailable: the local path cannot be inspected (${errorCode(error) ?? "error"})`), error);
  }
  if (
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
