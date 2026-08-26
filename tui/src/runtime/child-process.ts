/**
 * The OS process lifecycle of the rustX local conversation runtime.
 *
 * This owner is deliberately thin. It spawns the binary, exposes its three
 * standard streams, keeps a bounded tail of stderr, and observes exit. It
 * knows nothing about the Runtime Client Protocol, allocates no request ids,
 * and never inspects a byte of stdout.
 *
 * The startup paths pass straight through to the binary. This client never
 * reads or interprets `models.jsonc`, the current runtime config, the
 * workspace, or the runtime root: those are Rust-owned configuration, and
 * reading them here would create a second model/Session authority.
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

/** The explicit startup paths the `rustx` binary requires. */
export interface RuntimePaths {
  models: string;
  config: string;
  workspace: string;
  runtimeRoot: string;
}

/** Startup controls forwarded verbatim to the Rust owner. */
export interface RuntimeStartupOptions {
  /**
   * Start on the Session the catalog has published as active instead of an
   * empty one. A launch is not a resume: the runtime begins on an empty
   * Session unless this asks otherwise, and a client replacing the process to
   * complete a Session switch it already published sets it.
   */
  continueActiveSession: boolean;
  /**
   * Start on this persisted Session instead. The identity is forwarded
   * verbatim: the catalog is the only thing that can say whether it names
   * anything, and a launch that names nothing fails in Rust.
   */
  session?: string | undefined;
  /** The lineage node to start on, meaningful only beside `session`. */
  node?: string | undefined;
  /**
   * Name the Session this launch binds. A name is display metadata Rust
   * publishes, never a way to choose a Session, so it qualifies whichever
   * Session the flags above bound.
   */
  sessionName?: string | undefined;
  /** Repeatable explicit Skill package/root paths, in caller order. */
  skillPaths: string[];
  /** Disable automatic/default Skill roots. */
  noSkills: boolean;
  /** Disable native/built-in Tool activation. */
  noBuiltinTools: boolean;
  /** Disable every active Tool. */
  noTools: boolean;
  /** Exact comma-separated strict Tool allowlist, when supplied. */
  tools?: string;
  /** Exact comma-separated final Tool exclusions, when supplied. */
  excludeTools?: string;
}

export interface ChildRuntimeProcessOptions {
  /** Path to the `rustx` binary. */
  binary: string;
  paths: RuntimePaths;
  /** Capability startup controls owned semantically by the Rust runtime. */
  startup?: RuntimeStartupOptions;
  /** The environment handed to the child. Defaults to this process's own. */
  env?: NodeJS.ProcessEnv;
  /** Working directory of the child. */
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

/**
 * One spawned `rustx` process.
 *
 * Nothing here is a semantic authority: a process that exits has not
 * cancelled anything, settled anything, or completed any background work. It
 * has only stopped running.
 */
export class ChildRuntimeProcess {
  readonly #child: ChildProcessWithoutNullStreams;
  readonly #stderrLimit: number;
  #stderrTail: Buffer = Buffer.alloc(0);
  #stderrTruncatedBytes = 0;
  #exit: ChildExit | undefined;
  readonly #exited: Promise<ChildExit>;

  private constructor(
    child: ChildProcessWithoutNullStreams,
    stderrLimit: number,
  ) {
    this.#child = child;
    this.#stderrLimit = stderrLimit;

    this.#child.stderr.on("data", (chunk: Buffer) => {
      this.#appendStderr(chunk);
    });
    // A diagnostics stream must never crash the client.
    this.#child.stderr.on("error", () => {});
    this.#child.stdin.on("error", () => {});

    this.#exited = this.#awaitExit();
  }

  /** Spawns the binary with the explicit startup argument contract. */
  static spawn(options: ChildRuntimeProcessOptions): ChildRuntimeProcess {
    const startup = options.startup ?? emptyRuntimeStartupOptions();
    const startupArguments: string[] = [];
    if (startup.continueActiveSession) {
      startupArguments.push("--continue");
    }
    if (startup.session !== undefined) {
      startupArguments.push("--session", startup.session);
    }
    if (startup.node !== undefined) {
      startupArguments.push("--node", startup.node);
    }
    if (startup.sessionName !== undefined) {
      startupArguments.push("--name", startup.sessionName);
    }
    for (const skillPath of startup.skillPaths) {
      startupArguments.push("--skill", skillPath);
    }
    if (startup.noSkills) {
      startupArguments.push("--no-skills");
    }
    if (startup.noBuiltinTools) {
      startupArguments.push("--no-builtin-tools");
    }
    if (startup.noTools) {
      startupArguments.push("--no-tools");
    }
    if (startup.tools !== undefined) {
      startupArguments.push("--tools", startup.tools);
    }
    if (startup.excludeTools !== undefined) {
      startupArguments.push("--exclude-tools", startup.excludeTools);
    }
    const child = spawn(
      options.binary,
      [
        "--models",
        options.paths.models,
        "--config",
        options.paths.config,
        "--workspace",
        options.paths.workspace,
        "--runtime-root",
        options.paths.runtimeRoot,
        ...startupArguments,
      ],
      {
        cwd: options.cwd,
        // The child performs its own credential resolution from this
        // environment. TypeScript never reads a credential out of it.
        env: options.env ?? process.env,
        stdio: ["pipe", "pipe", "pipe"],
      },
    ) as ChildProcessWithoutNullStreams;

    return new ChildRuntimeProcess(
      child,
      options.stderrTailBytes ?? STDERR_TAIL_BYTES,
    );
  }

  get pid(): number | undefined {
    return this.#child.pid;
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
   * reach it only after the canonical `shutdown` + stdin-EOF sequence.
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

function emptyRuntimeStartupOptions(): RuntimeStartupOptions {
  return {
    continueActiveSession: false,
    session: undefined,
    node: undefined,
    sessionName: undefined,
    skillPaths: [],
    noSkills: false,
    noBuiltinTools: false,
    noTools: false,
  };
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    // A pending grace timer must never hold the process open.
    timer.unref?.();
  });
}
