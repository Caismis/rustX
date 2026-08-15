/**
 * A launcher for the shared external provider emulator.
 *
 * This file owns **process mechanics only**: command construction, readiness
 * parsing, the base URL, the control API, and child cleanup. It contains no
 * provider protocol and no scripted SSE body — those live once, in
 * `test-support/fake-provider/`, and are shared with the Rust conformance
 * suite. A second TypeScript provider-wire implementation is exactly what
 * issue #47 removed.
 *
 * ```text
 * integration test -> ProviderEmulator.start("tui_integration")
 *                  -> uv run fake-provider --port 0
 * rustx child      -> real adapter -> real HTTP/SSE -> that process
 * ```
 */

import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

/** The uv project root of the emulator. */
const PROJECT = fileURLToPath(
  new URL("../../../test-support/fake-provider", import.meta.url),
);

/** Deadlock protection only; ordering always comes from an observation. */
const AWAIT_TIMEOUT_MS = 20_000;

interface Ready {
  readonly ready: true;
  readonly host: string;
  readonly port: number;
  readonly scenario: string;
}

/** A growing capture of the child's stderr, shared with the launcher. */
interface Diagnostics {
  text: string;
}

export class ProviderEmulator {
  readonly #child: ChildProcessWithoutNullStreams;
  readonly #ready: Ready;
  readonly #diagnostics: Diagnostics;

  private constructor(
    child: ChildProcessWithoutNullStreams,
    ready: Ready,
    diagnostics: Diagnostics,
  ) {
    this.#child = child;
    this.#ready = ready;
    this.#diagnostics = diagnostics;
  }

  /** Starts one scenario on an ephemeral loopback port. */
  static async start(scenario: string): Promise<ProviderEmulator> {
    const child = spawn(
      "uv",
      [
        "run",
        "--project",
        PROJECT,
        "--frozen",
        "fake-provider",
        "--scenario",
        scenario,
        "--port",
        "0",
      ],
      { stdio: ["pipe", "pipe", "pipe"] },
    );

    const diagnostics: Diagnostics = { text: "" };
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => {
      diagnostics.text += chunk;
    });

    const lines = createInterface({ input: child.stdout });
    const first = await new Promise<string>((resolve, reject) => {
      const timer = setTimeout(
        () =>
          reject(
            new Error(
              `the provider emulator never reported readiness\n${diagnostics.text}`,
            ),
          ),
        AWAIT_TIMEOUT_MS,
      );
      lines.once("line", (line: string) => {
        clearTimeout(timer);
        resolve(line);
      });
      child.once("exit", (code) => {
        clearTimeout(timer);
        reject(
          new Error(
            `the provider emulator exited with ${code} before readiness\n${diagnostics.text}`,
          ),
        );
      });
    });
    // The remaining stdout (the final report) is drained so the child never
    // blocks on a full pipe.
    lines.on("line", () => {});

    const ready = JSON.parse(first) as Ready;
    if (ready.ready !== true || ready.scenario !== scenario) {
      child.kill("SIGKILL");
      throw new Error(`unexpected readiness record: ${first}`);
    }
    return new ProviderEmulator(child, ready, diagnostics);
  }

  /** Whether `uv` is available at all. */
  static async available(): Promise<boolean> {
    return await new Promise((resolve) => {
      const probe = spawn("uv", ["--version"], { stdio: "ignore" });
      probe.once("error", () => resolve(false));
      probe.once("exit", (code) => resolve(code === 0));
    });
  }

  /** The provider root. */
  get baseUrl(): string {
    return `http://${this.#ready.host}:${this.#ready.port}`;
  }

  /** The OpenAI-family `baseUrl` to configure in the model catalog. */
  url(path = "/v1"): string {
    return `${this.baseUrl}${path}`;
  }

  /** Every provider request the runtime actually sent, in arrival order. */
  async requests(): Promise<readonly Record<string, unknown>[]> {
    const body = (await this.#control("GET", "/requests")) as {
      requests: Record<string, unknown>[];
    };
    return body.requests;
  }

  /** The captured provider diagnostics. */
  get diagnostics(): string {
    return this.#diagnostics.text;
  }

  /**
   * Shuts the provider down and asserts the scenario was satisfied: every
   * declared step consumed, in order, with no unexpected request.
   */
  async finish(): Promise<void> {
    let report: { ok?: boolean } = {};
    try {
      report = (await this.#control("POST", "/shutdown")) as { ok?: boolean };
    } finally {
      await this.#exit();
    }
    if (report.ok !== true) {
      throw new Error(
        `the ${this.#ready.scenario} scenario was not satisfied: ` +
          `${JSON.stringify(report, null, 2)}\n${this.#diagnostics.text}`,
      );
    }
  }

  /** Terminates the child unconditionally, for a failing test's cleanup. */
  async stop(): Promise<void> {
    this.#child.stdin.end();
    await this.#exit();
  }

  async #exit(): Promise<void> {
    if (this.#child.exitCode !== null || this.#child.signalCode !== null) {
      return;
    }
    this.#child.stdin.end();
    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => {
        this.#child.kill("SIGKILL");
        resolve();
      }, 10_000);
      this.#child.once("exit", () => {
        clearTimeout(timer);
        resolve();
      });
    });
  }

  async #control(method: string, path: string): Promise<unknown> {
    const response = await fetch(`${this.baseUrl}/__control${path}`, {
      method,
    });
    const body = await response.json();
    if (!response.ok && !path.startsWith("/shutdown")) {
      throw new Error(
        `control ${method} ${path} failed: ${JSON.stringify(body)}`,
      );
    }
    return body;
  }
}
