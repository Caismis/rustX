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
  /** Save left its staged file behind, or could not show it removed. */
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
  /** This Save's staged file, when cleanup could not show it removed: where
   * it was created, what cleanup established, and why. */
  residue?: { path: string; staged: StagedResidue; cause: unknown };
}

/**
 * What cleanup established about a staged file it could not show removed.
 * `remains`: the file is still linked by a name the evidence cannot
 * attribute to the destination. `unknown`: the evidence cannot tell, because
 * the one remaining link may be an uninspectable destination, or the file's
 * link count could not be read.
 */
export type StagedResidue = "remains" | "unknown";

/** The clause a residue adds to a Save's outcome. */
export function residueText(path: string, staged: StagedResidue, owner = "its"): string {
  return staged === "remains"
    ? `${owner} staged file (created at ${path}) was not removed`
    : `whether ${owner} staged file (created at ${path}) was removed could not be established`;
}

/** A Save that published nothing but could not show its staged file removed. */
export class DeliveryResidueError extends DeliveryActionError {
  /** Where the staged file was created; it may have been moved since. */
  readonly residue: string;
  readonly staged: StagedResidue;
  readonly cleanup: unknown;
  constructor(residue: string, staged: StagedResidue, cause: unknown, cleanup: unknown) {
    super(`Nothing was saved, but ${residueText(residue, staged, "this save's")}`);
    this.name = "DeliveryResidueError";
    this.residue = residue;
    this.staged = staged;
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
 * A Save whose dispatched link has no proven outcome: link(2) failed, which
 * never shows that it created nothing, or it succeeded, and either way the
 * destination then did not name the file this Save wrote. Nothing retries
 * the link or touches the destination.
 */
export class DeliveryUncertainError extends DeliveryActionError {
  readonly path: string;
  /** link(2) reported success; otherwise its error is the `cause`. */
  readonly linked: boolean;
  readonly observed: DestinationObservation;
  /** Where the staged file was created, when cleanup could not show it removed. */
  readonly residue: string | undefined;
  readonly staged: StagedResidue | undefined;
  constructor(
    path: string,
    link: { error: unknown } | undefined,
    observed: DestinationObservation,
    residue: { path: string; staged: StagedResidue } | undefined,
  ) {
    const now = observed.kind === "absent" ? "it is now absent"
      : observed.kind === "foreign" ? "it now names another file"
        : `it cannot be inspected (${errorCode(observed.error) ?? "error"})`;
    super(
      (link === undefined
        ? `Save outcome unknown: linking ${path} succeeded, but ${now}, so this save cannot show it holds the saved bytes`
        : `Save outcome unknown: linking ${path} failed (${linkFailure(link.error)}) and ${now}; this save may have created it`) +
        (residue === undefined ? "" : `; ${residueText(residue.path, residue.staged)}`),
    );
    this.name = "DeliveryUncertainError";
    this.path = path;
    this.linked = link === undefined;
    this.observed = observed;
    this.residue = residue?.path;
    this.staged = residue?.staged;
    if (link !== undefined) this.cause = link.error;
  }
}

function linkFailure(error: unknown): string {
  const code = errorCode(error);
  if (code === undefined) return "error";
  return UNSUPPORTED_LINK.has(code) ? `${code}; this filesystem may not support the hard link Save publishes with` : code;
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

/**
 * Link errors that usually mean this filesystem cannot create a name by a hard
 * link. Once a link is dispatched they still prove nothing (see
 * {@link publish}); they only explain an uncertain outcome.
 */
const UNSUPPORTED_LINK = new Set(["EPERM", "ENOTSUP", "EOPNOTSUPP", "ENOSYS"]);

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
 *   -> lstat(destination) not absent?  refused (exists / uninspectable), no link
 *   -> abort?   <- publication admission: the last cancellation point
 *   -> link(staged name, destination)   <- publication commit (atomic, no clobber)
 *   -> lstat(destination) names F's (dev, ino)?  published : uncertain
 *   -> unlink(staged name), once; fresh observations decide whether F is residue
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
 * The parent's access policy, ACLs included, is the user's choice and is
 * trusted with the bytes: Save requests mode 0600 and grants nothing else.
 * A substitution cannot make Save claim more than it did: publication is
 * claimed only when the destination, observed after the commit, names F, and
 * cleanup only on observations taken after its unlink, so a staged file moved
 * elsewhere is never reported removed even though its name is gone.
 *
 * The destination is a pathname resolved at the commit: `link` creates a new
 * name there and fails with EEXIST for any existing entry, including a
 * symlink or a directory, so nothing is ever overwritten. A filesystem that
 * cannot link fails the link, an uncertain outcome like any dispatched link
 * that publication cannot be shown for; there is no copying or rename
 * fallback, and no retry. Nothing ever removes the destination.
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
  // 0600 at creation: no umask can set a group or other bit, and the
  // published file, the same inode, keeps that mode. Effective access is the
  // filesystem's: an ACL the parent's new files inherit (macOS) is the chosen
  // directory's trusted policy, which Save neither strips nor rewrites.
  const staged = childPath(dirname(destination), `.rustx-save-${randomBytes(16).toString("hex")}`);
  const file = await files.open(staged, O_WRONLY | O_CREAT | O_EXCL, 0o600);
  let publication: Publication;
  let identity: BigIntStats | undefined;
  try {
    identity = await file.stat({ bigint: true });
    await writeStaged(file, bytes, signal);
    publication = await publish(files, staged, destination, identity, effect, signal);
  } catch (error) {
    publication = { kind: "refused", error };
  }
  const cleanup = await removeStaged(files, file, staged, destination, identity);
  // The outcome and the cleanup are decided, and published bytes were synced
  // before the link, so closing cannot change what this Save did.
  await file.close().catch(() => {});
  if (cleanup.staged !== "removed") effect.residue = true;
  switch (publication.kind) {
    case "published":
      return cleanup.staged === "removed" ? { path: destination }
        : { path: destination, residue: { path: staged, staged: cleanup.staged, cause: cleanup.error } };
    case "uncertain":
      throw new DeliveryUncertainError(
        destination,
        publication.link,
        publication.observed,
        cleanup.staged === "removed" ? undefined : { path: staged, staged: cleanup.staged },
      );
    case "refused":
      if (cleanup.staged !== "removed") throw new DeliveryResidueError(staged, cleanup.staged, publication.error, cleanup.error);
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
 * Before anything is linked, the destination must be absent: an existing
 * entry is refused as "already exists", and a destination that cannot be
 * inspected is refused too, since publication could never be shown there.
 * Both are definite: this save dispatched no link.
 *
 * Once the link is dispatched, only the destination naming the staged file,
 * observed after the commit, proves publication, whatever link(2) answered.
 * Nothing proves refusal. A network filesystem may perform a link and then
 * answer a retransmission of it, and that answer describes the repeat, not
 * the original: EEXIST because the original created the name, ENOENT
 * because someone then moved the staged name, EACCES because the parent's
 * permissions then changed. An absent or foreign destination does not show
 * that the link created nothing either, since whoever may change the parent
 * can remove or replace the entry in between. So every other outcome is
 * uncertain, with the error and the observation kept.
 */
async function publish(
  files: SaveFiles,
  staged: string,
  destination: string,
  identity: BigIntStats,
  effect: LocalEffect,
  signal: AbortSignal | undefined,
): Promise<Publication> {
  try {
    await files.lstat(destination, { bigint: true });
    return { kind: "refused", error: new DeliveryActionError(`Not saved: ${destination} already exists`) };
  } catch (error) {
    if (errorCode(error) !== "ENOENT") {
      return {
        kind: "refused",
        error: withCause(new DeliveryActionError(`Not saved: ${destination} cannot be inspected (${errorCode(error) ?? "error"})`), error),
      };
    }
  }
  let failure: { error: unknown } | undefined;
  try {
    // Admission and dispatch in one synchronous step: no abort can be
    // observed between them.
    signal?.throwIfAborted();
    effect.committed = true;
    await files.link(staged, destination);
  } catch (error) {
    if (!effect.committed) throw error;
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
  return { kind: "uncertain", link: failure, observed };
}

/** What one lstat of `path` shows about the staged file F, at its instant. */
type LinkObservation = { kind: "names"; nlink: bigint } | { kind: "other" } | { kind: "uninspectable" };
async function linkAt(files: SaveFiles, path: string, identity: BigIntStats): Promise<LinkObservation> {
  try {
    const entry = await files.lstat(path, { bigint: true });
    return entry.dev === identity.dev && entry.ino === identity.ino ? { kind: "names", nlink: entry.nlink } : { kind: "other" };
  } catch (error) {
    return errorCode(error) === "ENOENT" ? { kind: "other" } : { kind: "uninspectable" };
  }
}

/**
 * The one cleanup: a single unlink of the staged name, whatever the outcome,
 * then fresh evidence of where the staged file F is still linked.
 *
 * Every conclusion rests on one observation taken after the unlink, and
 * atomic on its own. Earlier observations, including the one that proved
 * publication, are never reused, since entries may have changed since, and
 * observations made at different instants are never combined into one:
 *
 * - removed: F's own link count is 0; or the destination names F and its
 *   count, read in that same lstat, is 1, so the destination is F's only link.
 * - remains: F's count is 2 or more (one name holds one link, so another
 *   link besides any destination exists); or the staged name still names F;
 *   or the destination names F with a count of 2 or more.
 * - unknown: none of these. For example, F has one link but neither the
 *   staged name nor the destination names it, the destination cannot be
 *   inspected, or nothing can be read.
 *
 * No call removes a name only while it names a given file, so this unlink
 * removes whatever the staged name holds; it never removes a directory,
 * which unlink refuses, and never the destination.
 */
async function removeStaged(
  files: SaveFiles,
  file: FileHandle,
  staged: string,
  destination: string,
  identity: BigIntStats | undefined,
): Promise<{ staged: "removed" } | { staged: StagedResidue; error: unknown }> {
  let failure: unknown;
  try {
    await files.unlink(staged);
  } catch (error) {
    if (errorCode(error) !== "ENOENT") failure = error;
  }
  const remains = (reason: string) => ({ staged: "remains" as const, error: failure ?? new DeliveryActionError(reason) });
  let unread: unknown;
  try {
    const { nlink } = await file.stat({ bigint: true });
    if (nlink === 0n) return { staged: "removed" };
    if (nlink >= 2n) return remains("it is linked elsewhere, so it was not removed");
  } catch (error) {
    unread = error;
  }
  let reason = "it is still linked, but neither at its staged name nor at the destination";
  if (identity !== undefined) {
    if ((await linkAt(files, staged, identity)).kind === "names") return remains("its staged name still holds it");
    const at = await linkAt(files, destination, identity);
    if (at.kind === "names") {
      return at.nlink === 1n ? { staged: "removed" } : remains("it is linked elsewhere, so it was not removed");
    }
    if (at.kind === "uninspectable") reason = "its remaining link may be the destination, which could not be inspected";
  }
  return { staged: "unknown", error: failure ?? unread ?? new DeliveryActionError(reason) };
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
