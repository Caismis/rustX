/**
 * Client-local file actions on committed deliveries.
 *
 * This is the one TUI owner of local filesystem effects for `/files`. Neither
 * renderers nor command definitions touch the filesystem; they dispatch to
 * these functions with an explicit destination or an explicit open request.
 *
 * ```text
 * Save   native delivery/read (authorized, ≤ 512 KiB, cancellable) -> bounded decode
 *        -> private staging held by descriptor in the destination's parent
 *        -> sync + close -> link(staged, destination): atomic, never replaces an entry
 * Open   native delivery/locate (authorized, verified leaf identity)
 *        -> this TUI spawned the server  AND  local lstat(dev, ino) matches
 *        -> argv-only OS opener; success means the opener accepted the request
 * ```
 *
 * A remote server's path is never interpreted as a client path: Open is
 * offered only for the stdio child this TUI spawned, and even then the leaf
 * device/inode must match before anything is launched. Path spelling and
 * loopback addresses are not evidence of a shared filesystem.
 *
 * Open is best effort by contract: the opener receives a pathname and
 * resolves it again itself, so the identity check is an availability check
 * at one instant, not a guarantee about the file an application eventually
 * shows. Save is the identity-preserving action.
 */

import { spawn } from "node:child_process";
import { constants, type BigIntStats } from "node:fs";
import { link, lstat, mkdtemp, open, rmdir, stat, unlink, type FileHandle } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, sep } from "node:path";

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
 * when the preceding component is a symlink; every spelled Save path is
 * built here.
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

/**
 * What one local action has done outside this process, recorded by the
 * action itself at the moment it happens and never reset. A surface that
 * retires before the action settles reads it to decide whether the outcome is
 * still owed to the user: retirement may stop an action that has not
 * committed, but cannot unsay one that has.
 */
export interface LocalEffect {
  /** Save dispatched its link; Open spawned its opener. */
  committed: boolean;
  /** Save left its private staging directory behind. */
  residue: boolean;
}
export function localEffect(): LocalEffect {
  return { committed: false, residue: false };
}

/** What a committed Save established. */
export interface SavedDelivery {
  /**
   * The exact destination at which this Save's link created an entry naming
   * the complete staged file. A claim about the commit, not about later:
   * anyone who may write the parent can rename or replace that entry since.
   */
  path: string;
  /** This Save's private staging directory, when it could not be removed:
   * where it was created, and why it was kept. */
  residue?: { path: string; cause: unknown };
}

/** A Save that published nothing but could not remove its private staging. */
export class DeliveryResidueError extends DeliveryActionError {
  /** Where the staging directory was created; it may have been moved since. */
  readonly residue: string;
  readonly cleanup: unknown;
  constructor(residue: string, cause: unknown, cleanup: unknown) {
    super(`Nothing was saved, but this save's staging directory (created at ${residue}) was not removed`);
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
        (residue === undefined ? "" : `; its staging directory (created at ${residue}) was not removed`),
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
  open: (path: string, flags: number, mode?: number) => Promise<FileHandle>;
  stat: (path: string, options: { bigint: true }) => Promise<BigIntStats>;
  mkdtemp: (prefix: string) => Promise<string>;
  link: (existing: string, path: string) => Promise<void>;
  lstat: (path: string, options: { bigint: true }) => Promise<BigIntStats>;
  unlink: (path: string) => Promise<void>;
  rmdir: (path: string) => Promise<void>;
}
export const SAVE_FILES: SaveFiles = { open, stat, mkdtemp, link, lstat, unlink, rmdir };

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

const { O_WRONLY, O_CREAT, O_EXCL, O_DIRECTORY, O_NOFOLLOW } = constants;
/** Linux `O_PATH`, which Node does not export: a descriptor that names a
 * directory without reading it, so a write-only parent still works. */
const O_PATH = 0o10000000;
const PINNED_PATHS = "Not saved: Save stages through descriptor paths (/proc/self/fd), which this system does not provide";

/**
 * `name` inside the directory `held` refers to, wherever that directory is
 * now. Linux resolves `/proc/self/fd/N` to the open file description itself,
 * so these paths keep naming the objects this Save created even after
 * someone renames or replaces their original names.
 */
function pinned(held: FileHandle, name?: string): string {
  return name === undefined ? `/proc/self/fd/${held.fd}` : `/proc/self/fd/${held.fd}/${name}`;
}

/**
 * Reads authorized original bytes and publishes them as a new client-local
 * file at exactly `destination`, or leaves `destination` untouched.
 *
 * ```text
 * read (cancellable on the server) -> abort?
 *   -> open the destination's parent as spelled          (held: P)
 *   -> mkdtemp P/.rustx-save-*  (0700)                    (held: S, checked ours)
 *   -> S/file  O_CREAT|O_EXCL -> chunked writes, abort? between chunks -> sync -> close
 *   -> abort?   <- publication admission: the last cancellation point
 *   -> link(S/file, destination)   <- publication commit (atomic, no clobber)
 *   -> unlink S/file; rmdir P/<name> only while that name is still S
 * ```
 *
 * Ownership is bound to objects, not names. The parent is opened through
 * the destination's own spelling, so the OS resolves it as it resolves the
 * destination, and the staging directory and staged file are then reached
 * only through those held descriptors: renaming, replacing or symlinking
 * the staging name, or renaming the parent, cannot redirect a write, the
 * link or the file's cleanup to another object. Only this user can add
 * entries to the 0700 staging directory, so `S/file` is this Save's file.
 * The one name-based step is removing the empty staging directory itself
 * (no API removes a directory by descriptor): it happens only when that name
 * still refers to S, checked immediately before, and removes nothing but an
 * empty directory; otherwise the directory is reported as residue.
 *
 * The destination is a pathname resolved at the commit: `link` creates a
 * new name there and fails with EEXIST for any existing entry, including a
 * symlink or a directory, so nothing is ever overwritten. A filesystem or
 * system that cannot link, or cannot hold staging by descriptor
 * (`/proc/self/fd`), refuses explicitly before anything is created; there is
 * no copying or rename fallback. Nothing ever removes the destination.
 *
 * Cancellation observed at or before admission prevents publication. The
 * admission check and the call that dispatches `link` run in one synchronous
 * step, so no abort can be observed in between; once dispatched, the link's
 * own result decides, and a later cancellation neither removes the file nor
 * reports it as unsaved. `effect` records the dispatch and any residue.
 */
export async function saveDelivery(
  read: () => Promise<{ data: string }>,
  destination: string,
  signal?: AbortSignal,
  files: SaveFiles = SAVE_FILES,
  effect: LocalEffect = localEffect(),
): Promise<SavedDelivery> {
  signal?.throwIfAborted();
  const bytes = decodeDelivery((await read()).data);
  signal?.throwIfAborted();
  if (process.platform !== "linux") throw new DeliveryActionError(PINNED_PATHS);
  const parent = await files.open(dirname(destination), O_PATH | O_DIRECTORY);
  let stage: FileHandle | undefined;
  let name: string | undefined;
  let created = false;
  let publication: Publication;
  try {
    await requirePinnedPaths(files, parent);
    name = basename(await files.mkdtemp(pinned(parent, ".rustx-save-")));
    stage = await openStage(files, parent, name);
    const file = await files.open(pinned(stage, "file"), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0o666);
    created = true;
    const identity = await writeStaged(file, bytes, signal);
    signal?.throwIfAborted();
    publication = await publish(files, pinned(stage, "file"), destination, identity, effect);
  } catch (error) {
    publication = { kind: "refused", error };
  }
  const cleanup = name === undefined ? { ok: true as const } : await removeStaging(files, parent, name, stage, created);
  // Path-only directory descriptors: closing changes no filesystem state,
  // so it cannot alter what this Save did.
  await stage?.close().catch(() => {});
  await parent.close().catch(() => {});
  // Reported by the destination's spelling of its parent, where it was made.
  const residue = name === undefined ? "" : childPath(dirname(destination), name);
  if (!cleanup.ok) effect.residue = true;
  switch (publication.kind) {
    case "published":
      return cleanup.ok ? { path: destination } : { path: destination, residue: { path: residue, cause: cleanup.error } };
    case "uncertain":
      throw new DeliveryUncertainError(destination, publication.error, publication.observed, cleanup.ok ? undefined : residue);
    case "refused":
      if (!cleanup.ok) throw new DeliveryResidueError(residue, publication.error, cleanup.error);
      throw publication.error;
  }
}

/** Refuses, before creating anything, where staging cannot be held by descriptor. */
async function requirePinnedPaths(files: SaveFiles, parent: FileHandle): Promise<void> {
  const held = await parent.stat({ bigint: true });
  let named: BigIntStats | undefined;
  let cause: unknown;
  try {
    named = await files.stat(pinned(parent), { bigint: true });
  } catch (error) {
    cause = error;
  }
  if (named === undefined || named.dev !== held.dev || named.ino !== held.ino) {
    throw withCause(new DeliveryActionError(PINNED_PATHS), cause);
  }
}

/** Opens the staging directory just created, refusing anything not this user's directory. */
async function openStage(files: SaveFiles, parent: FileHandle, name: string): Promise<FileHandle> {
  let stage: FileHandle;
  try {
    stage = await files.open(pinned(parent, name), O_PATH | O_DIRECTORY | O_NOFOLLOW);
  } catch (error) {
    throw withCause(new DeliveryActionError("Not saved: the staging directory was replaced before use"), error);
  }
  const owner = await stage.stat({ bigint: true }).catch(async (error: unknown) => {
    await stage.close().catch(() => {});
    throw error;
  });
  if (!owner.isDirectory() || owner.uid !== BigInt(process.getuid?.() ?? -1)) {
    await stage.close().catch(() => {});
    throw new DeliveryActionError("Not saved: the staging directory was replaced before use");
  }
  return stage;
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
async function publish(
  files: SaveFiles,
  staged: string,
  destination: string,
  identity: BigIntStats,
  effect: LocalEffect,
): Promise<Publication> {
  try {
    effect.committed = true;
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
 * Removes this Save's staged file through its held directory, then the
 * staging directory by name, only while that name still refers to it. A
 * staged name already gone counts as removed, and so does a directory that
 * no longer exists anywhere; a directory moved or replaced, or one someone
 * else added entries to, is kept and reported.
 */
async function removeStaging(
  files: SaveFiles,
  parent: FileHandle,
  name: string,
  stage: FileHandle | undefined,
  created: boolean,
): Promise<{ ok: true } | { ok: false; error: unknown }> {
  try {
    if (stage === undefined) throw new DeliveryActionError("it was replaced before use, so it was not removed");
    if (created) await absentIsDone(files.unlink(pinned(stage, "file")));
    const own = await stage.stat({ bigint: true });
    if (own.nlink === 0n) return { ok: true };
    const named = await files.lstat(pinned(parent, name), { bigint: true }).catch((error: unknown) => {
      if (errorCode(error) === "ENOENT") return undefined;
      throw error;
    });
    if (named?.dev !== own.dev || named.ino !== own.ino) {
      throw new DeliveryActionError("it was moved or replaced, so it was not removed");
    }
    await files.rmdir(pinned(parent, name)).catch(async (error: unknown) => {
      // Gone between the check and the removal: done only if it is gone
      // everywhere, not merely from this name.
      if (errorCode(error) !== "ENOENT" || (await stage.stat({ bigint: true })).nlink !== 0n) throw error;
    });
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

/** What an open request established: the opener accepted a request to open
 * this pathname. Never "the application opened", and never which file it
 * eventually showed. */
export interface OpenRequested {
  path: string;
  opener: string;
}

/**
 * Asks the system opener to open a delivery's original file, best effort, in
 * the one environment where a server path is also a client path: the stdio
 * child this TUI spawned (`sharedHost`), on this machine, as this user.
 *
 * ```text
 * locate (native, verified leaf) -> abort? -> lstat: regular file, same dev/ino?
 *   -> abort?   <- launch admission: the last cancellation point
 *   -> spawn opener(path)   <- launch commit (effect.committed)
 *   -> exit 0: accepted | otherwise: rejected
 * ```
 *
 * The device/inode check proves only that, at that instant, the path names
 * the delivered file; it is an availability check, not a security
 * guarantee. The opener and the application it starts resolve the pathname
 * again, later, by themselves, so a rename or replacement in between can make
 * them open something else, and no portable opener accepts a descriptor
 * instead. The result therefore claims only that the opener accepted the
 * request. Save is the action that preserves identity.
 */
export async function openDelivery(
  locate: () => Promise<DeliveryLocation>,
  options: { sharedHost: boolean; opener?: string; launch?: Opener; signal?: AbortSignal; effect?: LocalEffect },
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
  // The launch commit, in the same step as the last cancellation check: once
  // the opener is spawned the request belongs to the OS, and a later
  // cancellation does not withdraw or misreport it.
  options.signal?.throwIfAborted();
  if (options.effect !== undefined) options.effect.committed = true;
  let status: number | null;
  try {
    status = await (options.launch ?? launchOpener)(opener, location.path);
  } catch (error) {
    throw withCause(new DeliveryActionError(`${opener} could not take the open request (${errorCode(error) ?? "error"})`), error);
  }
  if (status !== 0) throw new DeliveryActionError(`${opener} did not accept the open request (exit ${status ?? "signal"})`);
  return { path: location.path, opener };
}
