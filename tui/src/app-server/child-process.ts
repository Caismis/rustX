/**
 * The OS process lifecycle of a TUI-owned App Server child.
 *
 * This owner is deliberately thin. It spawns `rustx app-server --listen stdio`,
 * exposes its three standard streams, keeps a bounded tail of stderr, and
 * observes exit. It knows nothing about the App Server protocol, allocates no
 * request ids, and never inspects a byte of stdout.
 *
 * ```text
 * stdout  protocol only  -> JSONL App Server messages
 * stderr  diagnostics only -> bounded tail, never parsed as protocol
 * ```
 *
 * That separation is the contract, not a convention: stderr is read into a
 * bounded buffer by this class and is never handed to the protocol decoder, so
 * no amount of child logging can corrupt a protocol record.
 *
 * The launch flags bind **process-level user configuration** — the canonical
 * user TOML, the model catalog, the runtime root. They are not Session
 * settings: a Session's cwd and project configuration come from
 * `session/create`, and launch cwd is never substituted for Session cwd. This
 * client never reads or interprets any of those files; they are Rust-owned
 * authorities, and reading them here would create a second one.
 *
 * Environment: the child inherits the intended parent environment so rustX
 * performs its own credential resolution. No credential is ever read,
 * resolved, defaulted, or logged on this side.
 */

import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import type { Readable, Writable } from "node:stream";

/** How many stderr bytes to retain for diagnostics. Never unbounded. */
export const STDERR_TAIL_BYTES = 16 * 1024;

/** How long a terminated child gets to exit before escalation. */
export const DEFAULT_TERMINATION_GRACE_MS = 5_000;

/**
 * Process-level source bindings forwarded verbatim to the App Server.
 *
 * Every one of these selects a source for the whole **process**, which is why
 * they exist only in local self-hosted mode: a remote App Server was launched
 * by someone else and already has its own.
 */
export interface AppServerLaunchOptions {
  /** Fixes `UserConfigSources.settings` for this process and its Sessions. */
  userSettings?: string | undefined;
  /** Overrides the model catalog source binding. */
  models?: string | undefined;
  /** Overrides the durable runtime root binding. */
  runtimeRoot?: string | undefined;
}

export interface AppServerChildOptions {
  /** Path to the `rustx` binary. */
  binary: string;
  launch: AppServerLaunchOptions;
  /** The environment handed to the child. Defaults to this process's own. */
  env?: NodeJS.ProcessEnv;
  /** Working directory of the child process. Never a Session cwd. */
  cwd?: string;
  stderrTailBytes?: number;
}

/** How a child process ended. */
export interface ChildExit {
  code: number | null;
  signal: NodeJS.Signals | null;
  /**
   * Why the process never started, when it never started.
   *
   * A spawn failure (a missing or non-executable binary) produces no exit
   * code and no signal, so without this the failure would be indistinguishable
   * from an unexplained disappearance.
   */
  spawnError?: string;
}

/** Builds the exact argument vector for a stdio App Server child. */
export function appServerArguments(launch: AppServerLaunchOptions): string[] {
  const argv = ["app-server"];
  if (launch.userSettings !== undefined) {
    argv.push("--user-settings", launch.userSettings);
  }
  if (launch.models !== undefined) {
    argv.push("--models", launch.models);
  }
  if (launch.runtimeRoot !== undefined) {
    argv.push("--runtime-root", launch.runtimeRoot);
  }
  argv.push("--listen", "stdio");
  return argv;
}

/**
 * One spawned `rustx app-server` process.
 *
 * Nothing here is a semantic authority: a process that exits has not cancelled
 * anything, settled anything, or completed any background work. It has only
 * stopped running.
 */
export class AppServerChild {
  readonly #child: ChildProcessWithoutNullStreams;
  readonly #stderrLimit: number;
  readonly #command: string;
  #stderrTail: Buffer = Buffer.alloc(0);
  #stderrTruncatedBytes = 0;
  #exit: ChildExit | undefined;
  readonly #exited: Promise<ChildExit>;

  private constructor(
    child: ChildProcessWithoutNullStreams,
    stderrLimit: number,
    command: string,
  ) {
    this.#child = child;
    this.#stderrLimit = stderrLimit;
    this.#command = command;

    this.#child.stderr.on("data", (chunk: Buffer) => {
      this.#appendStderr(chunk);
    });
    // A diagnostics stream must never crash the client.
    this.#child.stderr.on("error", () => {});
    this.#child.stdin.on("error", () => {});

    this.#exited = this.#awaitExit();
  }

  /** Spawns the App Server. Rust owns every launch resolution. */
  static spawn(options: AppServerChildOptions): AppServerChild {
    const argv = appServerArguments(options.launch);
    const child = spawn(options.binary, argv, {
      cwd: options.cwd,
      // The child performs its own credential resolution from this
      // environment. TypeScript never reads a credential out of it.
      env: options.env ?? process.env,
      stdio: ["pipe", "pipe", "pipe"],
    }) as ChildProcessWithoutNullStreams;

    return new AppServerChild(
      child,
      options.stderrTailBytes ?? STDERR_TAIL_BYTES,
      [options.binary, ...argv].join(" "),
    );
  }

  get pid(): number | undefined {
    return this.#child.pid;
  }

  /** The command line, for diagnostics only. */
  get command(): string {
    return this.#command;
  }

  /** The protocol input stream of the child. */
  get stdin(): Writable {
    return this.#child.stdin;
  }

  /** The protocol output stream of the child. */
  get stdout(): Readable {
    return this.#child.stdout;
  }

  /** Whether the process has been observed to exit. */
  get exited(): ChildExit | undefined {
    return this.#exit;
  }

  /**
   * The bounded stderr tail.
   *
   * Diagnostics are finite by construction: only the most recent
   * {@link STDERR_TAIL_BYTES} are retained, and the count of dropped bytes is
   * reported rather than silently hidden.
   */
  stderrTail(): { text: string; truncatedBytes: number } {
    return {
      text: this.#stderrTail.toString("utf8"),
      truncatedBytes: this.#stderrTruncatedBytes,
    };
  }

  /** Closes the transport input stream. This is EOF, never cancellation. */
  closeStdin(): void {
    if (!this.#child.stdin.destroyed && this.#child.stdin.writable) {
      this.#child.stdin.end();
    }
  }

  /** Resolves when the process exits. */
  wait(): Promise<ChildExit> {
    return this.#exited;
  }

  /**
   * Waits for exit, escalating only if the process overstays its grace.
   *
   * The escalation is a process-level fallback, not a semantic operation: it
   * says nothing about attempts, tool executions, or background work. Callers
   * reach it only after the owner's explicit stdin-EOF shutdown step.
   */
  async waitOrTerminate(
    graceMs: number = DEFAULT_TERMINATION_GRACE_MS,
  ): Promise<ChildExit> {
    const raced = await Promise.race([
      this.#exited,
      delay(graceMs).then(() => "timeout" as const),
    ]);
    if (raced !== "timeout") {
      return raced;
    }

    this.#child.kill("SIGTERM");
    const afterTerm = await Promise.race([
      this.#exited,
      delay(graceMs).then(() => "timeout" as const),
    ]);
    if (afterTerm !== "timeout") {
      return afterTerm;
    }

    this.#child.kill("SIGKILL");
    return this.#exited;
  }

  #awaitExit(): Promise<ChildExit> {
    return new Promise<ChildExit>((resolve) => {
      const settle = (exit: ChildExit) => {
        if (this.#exit !== undefined) {
          return;
        }
        this.#exit = exit;
        resolve(exit);
      };
      this.#child.on("exit", (code, signal) => {
        settle({ code, signal });
      });
      // A spawn failure (a missing or non-executable binary) emits `error`
      // and never `exit`. Listening explicitly keeps the failure a resolved
      // exit rather than an unhandled rejection, so callers waiting on the
      // process still settle — and the reason survives for the diagnostic.
      this.#child.on("error", (cause: Error) => {
        settle({ code: null, signal: null, spawnError: cause.message });
      });
    });
  }

  #appendStderr(chunk: Buffer): void {
    const combined =
      this.#stderrTail.byteLength === 0
        ? chunk
        : Buffer.concat([this.#stderrTail, chunk]);
    if (combined.byteLength <= this.#stderrLimit) {
      this.#stderrTail = combined;
      return;
    }
    const dropped = combined.byteLength - this.#stderrLimit;
    this.#stderrTruncatedBytes += dropped;
    this.#stderrTail = combined.subarray(dropped);
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    // A pending grace timer must never hold the process open.
    timer.unref?.();
  });
}
