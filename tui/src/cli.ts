/**
 * The bounded startup arguments of `rustx-tui`.
 *
 * There are exactly two modes, and they differ in one thing: who runs the App
 * Server.
 *
 * ```text
 * local self-hosted                       existing / remote
 *   rustx-tui --binary <rustx> ...          rustx-tui --connect ws://host:port
 *   spawns and owns                         attaches to a server someone
 *   rustx app-server --listen stdio         else already runs, over WebSocket
 * ```
 *
 * Both speak the same App Server protocol and the same generated DTOs. After
 * the transport is bound nothing downstream knows which mode it is in — except
 * the one place that must: who owns the process, and therefore what exiting
 * means.
 *
 * # Where each option belongs
 *
 * Options fall into three groups that are deliberately not mixed:
 *
 * - **Transport/mode**: `--binary` and `--connect` select the mode.
 * - **App Server process bindings** (`--user-settings`, `--models`,
 *   `--runtime-root`): these configure the *process*, so they are local-only.
 *   A remote App Server was launched by someone else and already has its own;
 *   accepting them against `--connect` would be a flag that pretends to
 *   configure a server it cannot reach.
 * - **Session settings** (`--cwd`, `--config`, `--model`, `--skill`, the tool
 *   and Skill switches): these are `session/create` inputs. A Session's cwd is
 *   a Session selection, never the App Server's launch directory, and the two
 *   are never substituted for one another. In remote mode these paths are
 *   resolved by the server, on the server's filesystem.
 *
 * Routing (`--session`, `--node`, `--resume`) selects which Session the
 * terminal opens on. It is client focus and nothing more: the App Server has no
 * global active Session, and opening one never stops another.
 */

import { posix, win32 } from "node:path";

import type { AppServerLaunchOptions } from "./app-server/child-process.ts";
import type { SessionSettings } from "./protocol/app-server.ts";

export const USAGE = `usage:
  local self-hosted (spawns and owns an App Server child over stdio):
    rustx-tui --binary <rustx> [--user-settings <settings.toml>] [--models <models.toml>] \\
              [--runtime-root <dir>] [session options] [routing options]

  existing / remote App Server (WebSocket):
    rustx-tui --connect <ws://host:port> --token-file <path> --cwd <server-absolute-dir> [session options] [routing options]

  session options (applied to Sessions this launch creates; paths resolve on the App Server host):
    [--cwd <dir>] [--config <rustx.toml>] [--model <provider/model>] [--name <text>]
    [--skill <path>] [--no-automatic-skills] [--no-builtin-tools] [--no-direct-tools]
    [--tools <a,b,c>] [--exclude-tools <a,b,c>]

  routing options (client focus only; never stops another Session):
    [--session <id> [--node <id>] | --resume]`;

/** How this launch reaches an App Server. */
export type ConnectionMode =
  | {
      kind: "local";
      /** Path to the `rustx` binary this TUI spawns and owns. */
      binary: string;
      launch: AppServerLaunchOptions;
    }
  | {
      kind: "remote";
      /** A `ws://` or `wss://` endpoint of an externally managed App Server. */
      endpoint: string;
      /** File holding the dedicated transport token. Read at connect time. */
      tokenFile: string;
    };

/** Which Session the terminal opens on. Client focus, never server state. */
export interface SessionRouting {
  session?: string;
  node?: string;
  /** Open the `/resume` picker as soon as the client is connected. */
  openSessionSelector: boolean;
}

export interface TuiArguments {
  mode: ConnectionMode;
  /** `session/create` inputs for Sessions this launch creates. */
  sessionSettings: SessionSettings;
  /** Name applied to the Session this launch binds, when supplied. */
  sessionName?: string;
  routing: SessionRouting;
}

export class ArgumentError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ArgumentError";
  }
}

const VALUE_FLAGS = [
  "--binary",
  "--connect",
  "--token-file",
  "--user-settings",
  "--models",
  "--runtime-root",
  "--cwd",
  "--config",
  "--model",
  "--name",
  "--session",
  "--node",
  "--skill",
  "--tools",
  "--exclude-tools",
] as const;

const BOOLEAN_FLAGS = [
  "--resume",
  "--no-automatic-skills",
  "--no-builtin-tools",
  "--no-direct-tools",
] as const;

/** Local-only because they bind sources of the App Server *process*. */
const PROCESS_FLAGS = ["--user-settings", "--models", "--runtime-root"] as const;

type ValueFlag = (typeof VALUE_FLAGS)[number];
type BooleanFlag = (typeof BOOLEAN_FLAGS)[number];

/**
 * Parses the argument vector.
 *
 * @throws {ArgumentError} on an unknown flag, a missing value, a repeated flag,
 * a missing required flag, or a combination that names two modes or two Session
 * routes at once.
 */
export function parseArguments(argv: readonly string[]): TuiArguments {
  const values = new Map<ValueFlag, string>();
  const skillPaths: string[] = [];
  const booleans = new Set<BooleanFlag>();

  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === undefined) {
      throw new ArgumentError("unknown argument");
    }
    if ((VALUE_FLAGS as readonly string[]).includes(flag)) {
      const valueFlag = flag as ValueFlag;
      const value = argv[index + 1];
      if (value === undefined) {
        throw new ArgumentError(`argument ${valueFlag} requires a value`);
      }
      if (valueFlag === "--skill") {
        skillPaths.push(value);
      } else {
        if (values.has(valueFlag)) {
          throw new ArgumentError(
            `argument ${valueFlag} was supplied more than once`,
          );
        }
        values.set(valueFlag, value);
      }
      index += 1;
      continue;
    }
    if ((BOOLEAN_FLAGS as readonly string[]).includes(flag)) {
      const booleanFlag = flag as BooleanFlag;
      if (booleans.has(booleanFlag)) {
        throw new ArgumentError(
          `argument ${booleanFlag} was supplied more than once`,
        );
      }
      booleans.add(booleanFlag);
      continue;
    }
    throw new ArgumentError(`unknown argument ${JSON.stringify(flag)}`);
  }

  const binary = values.get("--binary");
  const connect = values.get("--connect");
  if (binary !== undefined && connect !== undefined) {
    throw new ArgumentError(
      "arguments --binary and --connect cannot be combined; a launch either owns an App Server or connects to one",
    );
  }
  if (binary === undefined && connect === undefined) {
    throw new ArgumentError(
      "missing required argument --binary (local self-hosted) or --connect (existing App Server)",
    );
  }

  const mode = connect === undefined
    ? localMode(binary as string, values)
    : remoteMode(connect, values);

  const session = values.get("--session");
  const node = values.get("--node");
  const resume = booleans.has("--resume");
  if (session !== undefined && resume) {
    throw new ArgumentError("arguments --session and --resume cannot be combined");
  }
  if (node !== undefined && session === undefined) {
    throw new ArgumentError("argument --node requires --session");
  }

  // Only a self-hosted child shares the TUI's filesystem namespace. Never
  // resolve, normalize, or default a remote path against the client machine.
  let cwd: string;
  if (mode.kind === "local") {
    cwd = values.get("--cwd") ?? process.cwd();
  } else {
    const explicit = values.get("--cwd");
    if (explicit === undefined || !(
      posix.isAbsolute(explicit) ||
      (win32.isAbsolute(explicit) && win32.parse(explicit).root.length > 1)
    )) {
      throw new ArgumentError("remote Session cwd requires an explicit --cwd absolute path on the App Server host");
    }
    cwd = explicit;
  }

  const model = values.get("--model");
  const tools = values.get("--tools");
  const excludeTools = values.get("--exclude-tools");

  return {
    mode,
    sessionSettings: {
      cwd,
      config: values.get("--config") ?? null,
      model: model === undefined ? null : { model },
      skill_paths: skillPaths,
      no_automatic_skills: booleans.has("--no-automatic-skills"),
      no_builtin_tools: booleans.has("--no-builtin-tools"),
      no_direct_tools: booleans.has("--no-direct-tools"),
      tools: tools === undefined ? null : splitList(tools),
      exclude_tools:
        excludeTools === undefined ? null : splitList(excludeTools),
    },
    sessionName: values.get("--name"),
    routing: { session, node, openSessionSelector: resume },
  };
}

function localMode(
  binary: string,
  values: Map<ValueFlag, string>,
): ConnectionMode {
  if (values.has("--token-file")) {
    throw new ArgumentError(
      "argument --token-file applies only to --connect; a stdio App Server child needs no transport credential",
    );
  }
  return {
    kind: "local",
    binary,
    launch: {
      userSettings: values.get("--user-settings"),
      models: values.get("--models"),
      runtimeRoot: values.get("--runtime-root"),
    },
  };
}

function remoteMode(
  endpoint: string,
  values: Map<ValueFlag, string>,
): ConnectionMode {
  if (!/^wss?:\/\//.test(endpoint)) {
    throw new ArgumentError(
      "argument --connect requires a ws:// or wss:// endpoint",
    );
  }
  for (const flag of PROCESS_FLAGS) {
    if (values.has(flag)) {
      throw new ArgumentError(
        `argument ${flag} configures an App Server process and cannot be combined with --connect; the external server owns its own configuration`,
      );
    }
  }
  const tokenFile = values.get("--token-file");
  if (tokenFile === undefined) {
    throw new ArgumentError(
      "argument --connect requires --token-file; the App Server WebSocket transport admits only credentialed clients",
    );
  }
  return { kind: "remote", endpoint, tokenFile };
}

function splitList(value: string): string[] {
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0);
}
