/**
 * Client-local file actions on committed deliveries.
 *
 * This is the one TUI owner of local filesystem effects for `/files`. Neither
 * renderers nor command definitions touch the filesystem; they dispatch to
 * these functions with an explicit destination or an explicit open request.
 *
 * ```text
 * Save   native delivery/read (authorized, ≤ 512 KiB, cancellable) -> bounded decode
 *        -> exclusive staged file beside the destination, written through its handle
 *        -> sync -> link(staged, destination): atomic, never replaces an entry
 *        -> published only if the destination then names that very file
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
import { randomBytes } from "node:crypto";
import { constants, type BigIntStats } from "node:fs";
import { link, lstat, open, unlink, type FileHandle } from "node:fs/promises";
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
  /** Save left its staged file linked somewhere besides the destination. */
  residue: boolean;
}
export function localEffect(): LocalEffect {
  return { committed: false, residue: false };
}

/** What a committed Save established. */
export interface SavedDelivery {
  /**
   * The exact destination at which this Save's link created a new entry,
   * observed right after the commit to name the very file this Save created
   * and wrote. A claim about the commit, not about later: anyone who may write
   * the parent can rename or replace that entry since.
   */
  path: string;
  /** This Save's staged file, when it is still linked somewhere besides the
   * destination: where it was created, and why it was kept. */
  residue?: { path: string; cause: unknown };
}

/** A Save that published nothing but whose staged file is still on disk. */
export class DeliveryResidueError extends DeliveryActionError {
  /** Where the staged file was created; it may have been moved since. */
  readonly residue: string;
  readonly cleanup: unknown;
  constructor(residue: string, cause: unknown, cleanup: unknown) {
    super(`Nothing was saved, but this save's staged file (created at ${residue}) was not removed`);
    this.name = "DeliveryResidueError";
    this.residue = residue;
    this.cleanup = cleanup;
    this.cause = cause;
  }
}

/**
 * What the destination showed when it did not name this Save's file, none of
 * which proves this save did not publish: its entry may have been created and
 * then removed or replaced by someone else.
 */
export type DestinationObservation =
  | { kind: "absent" }
  | { kind: "foreign" }
  | { kind: "uninspectable"; error: unknown };

/**
 * A Save whose dispatched link has no proven outcome: link(2) failed
 * ambiguously, or it succeeded and the destination then did not name the file
 * this Save wrote. Nothing retries the link or touches the destination.
 */
export class DeliveryUncertainError extends DeliveryActionError {
  readonly path: string;
  /** link(2) reported success; otherwise its error is the `cause`. */
  readonly linked: boolean;
  readonly observed: DestinationObservation;
  readonly residue: string | undefined;
  constructor(path: string, link: { error: unknown } | undefined, observed: DestinationObservation, residue: string | undefined) {
    const now = observed.kind === "absent" ? "it is now absent"
      : observed.kind === "foreign" ? "it now names another file"
        : `it cannot be inspected (${errorCode(observed.error) ?? "error"})`;
    super(
      (link === undefined
        ? `Save outcome unknown: linking ${path} succeeded, but ${now}, so this save cannot show it holds the saved bytes`
        : `Save outcome unknown: linking ${path} failed (${errorCode(link.error) ?? "error"}) and ${now}; this save may have created it`) +
        (residue === undefined ? "" : `; its staged file (created at ${residue}) was not removed`),
    );
    this.name = "DeliveryUncertainError";
    this.path = path;
    this.linked = link === undefined;
    this.observed = observed;
    this.residue = residue;
    if (link !== undefined) this.cause = link.error;
  }
}

/** Bytes per write; cancellation is observed between chunks. */
export const SAVE_CHUNK_BYTES = 64 * 1024;

/** The filesystem operations of {@link saveDelivery}; tests interpose on them. */
export interface SaveFiles {
  open: (path: string, flags: number, mode?: number) => Promise<FileHandle>;
  link: (existing: string, path: string) => Promise<void>;
  lstat: (path: string, options: { bigint: true }) => Promise<BigIntStats>;
  unlink: (path: string) => Promise<void>;
}
export const SAVE_FILES: SaveFiles = { open, link, lstat, unlink };

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
  | { kind: "uncertain"; link: { error: unknown } | undefined; observed: DestinationObservation };

const { O_WRONLY, O_CREAT, O_EXCL } = constants;

/**
 * Reads authorized original bytes and publishes them as a new client-local
 * file at exactly `destination`, or leaves `destination` untouched.
 *
 * ```text
 * read (cancellable on the server) -> abort?
 *   -> open <parent>/.rustx-save-<random>  O_CREAT|O_EXCL      held: F
 *   -> chunked writes through F, abort? between chunks -> fsync F
 *   -> abort?   <- publication admission: the last cancellation point
 *   -> link(staged name, destination)   <- publication commit (atomic, no clobber)
 *   -> lstat(destination) names F's (dev, ino)?  published : refused | uncertain
 *   -> unlink(staged name), once; F's link count decides whether F is residue
 * ```
 *
 * One owner: the handle the exclusive create returned. O_EXCL fails on any
 * existing entry, a symlink included, so F is a file this Save created, and
 * every byte is written through F, never by name. Neither the owner, the
 * type nor the name of an entry is taken as evidence that it is this Save's.
 *
 * Two steps take names, because neither Linux nor macOS offers a portable
 * call that links or removes a file by descriptor: the link reads the staged
 * name, and cleanup unlinks it. Whoever may modify the destination's parent
 * can therefore substitute the staged name between steps; that is the trust
 * boundary, and such an actor could create or remove those entries directly.
 * A substitution cannot make Save claim more than it did: publication is
 * claimed only when the destination, observed after the commit, names F, and
 * cleanup is judged by F's link count, so a staged file moved elsewhere is
 * residue even though its name is gone.
 *
 * The destination is a pathname resolved at the commit: `link` creates a new
 * name there and fails with EEXIST for any existing entry, including a
 * symlink or a directory, so nothing is ever overwritten. A filesystem that
 * cannot link refuses explicitly; there is no copying or rename fallback.
 * Nothing ever removes the destination.
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
  // Beside the destination, so the link stays on one filesystem. The random
  // name only avoids collisions; O_EXCL is what makes the file this Save's.
  const staged = childPath(dirname(destination), `.rustx-save-${randomBytes(16).toString("hex")}`);
  const file = await files.open(staged, O_WRONLY | O_CREAT | O_EXCL, 0o666);
  let publication: Publication;
  try {
    const identity = await file.stat({ bigint: true });
    await writeStaged(file, bytes, signal);
    signal?.throwIfAborted();
    publication = await publish(files, staged, destination, identity, effect);
  } catch (error) {
    publication = { kind: "refused", error };
  }
  const cleanup = await removeStaged(files, file, staged, publication);
  // The outcome and the cleanup are decided, and published bytes were synced
  // before the link, so closing cannot change what this Save did.
  await file.close().catch(() => {});
  if (!cleanup.ok) effect.residue = true;
  switch (publication.kind) {
    case "published":
      return cleanup.ok ? { path: destination } : { path: destination, residue: { path: staged, cause: cleanup.error } };
    case "uncertain":
      throw new DeliveryUncertainError(destination, publication.link, publication.observed, cleanup.ok ? undefined : staged);
    case "refused":
      if (!cleanup.ok) throw new DeliveryResidueError(staged, publication.error, cleanup.error);
      throw publication.error;
  }
}

/** Writes every byte through the staged file's own handle, then syncs it. */
async function writeStaged(file: FileHandle, bytes: Uint8Array, signal?: AbortSignal): Promise<void> {
  for (let offset = 0; offset < bytes.length;) {
    signal?.throwIfAborted();
    const length = Math.min(SAVE_CHUNK_BYTES, bytes.length - offset);
    const { bytesWritten } = await file.write(bytes, offset, length);
    if (bytesWritten === 0) throw new DeliveryActionError("Save staging stopped accepting bytes");
    offset += bytesWritten;
  }
  await file.sync();
}

/**
 * The publication commit and what it proves.
 *
 * Only the destination naming the staged file, observed after the commit,
 * proves publication, whatever link(2) answered: a network filesystem can
 * fail a retransmitted link it performed, and a successful link of the staged
 * name says nothing about which file that name held. Otherwise only a
 * definite rejection proves refusal: an absent or foreign destination does
 * not show that this link created nothing, because someone may have removed
 * or replaced the entry in between. Anything else is uncertain.
 */
async function publish(
  files: SaveFiles,
  staged: string,
  destination: string,
  identity: BigIntStats,
  effect: LocalEffect,
): Promise<Publication> {
  let failure: { error: unknown } | undefined;
  try {
    effect.committed = true;
    await files.link(staged, destination);
  } catch (error) {
    failure = { error };
  }
  let observed: DestinationObservation;
  try {
    const current = await files.lstat(destination, { bigint: true });
    if (current.dev === identity.dev && current.ino === identity.ino) return { kind: "published" };
    observed = { kind: "foreign" };
  } catch (inspection) {
    observed = errorCode(inspection) === "ENOENT" ? { kind: "absent" } : { kind: "uninspectable", error: inspection };
  }
  const code = errorCode(failure?.error);
  if (failure !== undefined && code !== undefined && DEFINITE_REFUSAL.has(code)) {
    return { kind: "refused", error: refusal(failure.error, destination) };
  }
  return { kind: "uncertain", link: failure, observed };
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
 * The one cleanup: a single unlink of the staged name, whatever the outcome,
 * judged by the staged file's own link count rather than by the name. The
 * file is done when no name links it except, when it may, the destination;
 * one still linked anywhere else (the unlink failed, or the file was moved)
 * is residue. No call removes a name only while it names a given file, so
 * this unlink removes whatever the staged name holds; it never removes a
 * directory, which unlink refuses, and never the destination.
 */
async function removeStaged(
  files: SaveFiles,
  file: FileHandle,
  staged: string,
  publication: Publication,
): Promise<{ ok: true } | { ok: false; error: unknown }> {
  const accounted = publication.kind === "published" ||
      (publication.kind === "uncertain" && publication.observed.kind === "uninspectable")
    ? 1n
    : 0n;
  let failure: unknown;
  try {
    await files.unlink(staged);
  } catch (error) {
    if (errorCode(error) !== "ENOENT") failure = error;
  }
  try {
    const { nlink } = await file.stat({ bigint: true });
    if (nlink <= accounted) return { ok: true };
    return { ok: false, error: failure ?? new DeliveryActionError("it is linked elsewhere, so it was not removed") };
  } catch (error) {
    return { ok: false, error: failure ?? error };
  }
}

function errorCode(error: unknown): string | undefined {
  const code = (error as { code?: unknown } | null | undefined)?.code;
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
