/**
 * The bounded startup arguments of `rustx-tui`.
 *
 * ```text
 * rustx-tui --binary <path> [--models <path>] [--config <path>]
 *           [--workspace <dir>] [--runtime-root <dir>] [--model <provider/model>]
 *           [--trust grant|revoke]
 *           [--inspect-conversation <id> | --resume | --session <id> [--node <id>]]
 *           [--name <text>] [--skill <path>] [--no-automatic-skills]
 *           [--no-builtin-tools] [--no-direct-tools]
 *           [--tools <a,b,c>] [--exclude-tools <a,b,c>]
 * ```
 *
 * Without a Session request the runtime starts on an empty Session and every
 * persisted Session stays reachable through `/resume`. Conversation inspection
 * is a separate read-only attachment by known conversation identity. The
 * following requests determine the startup attachment, and they are mutually
 * exclusive with inspection:
 *
 * - `--session <id>` (optionally `--node <id>`) starts on a named persisted
 *   Session;
 * - `--resume` opens the `/resume` selector over a fresh Session as
 *   soon as the client attaches, so a Session can be chosen instead of named.
 * - `--inspect-conversation <id>` opens the ordinary Runtime Client projection
 *   for that conversation, attaching to a running child's live projection or
 *   falling back to durable authorities without composing a Session or
 *   execution owner.
 *
 * `--name` is not one of those requests. It names the Session the launch
 * bound, whichever one that is, and it is forwarded to Rust like every other
 * startup control — a Session is never opened by its name.
 *
 * The client owns focus. A picker result supplies the explicit Session/node
 * identity for its next subprocess attachment; no focus is published durably.
 *
 * Optional runtime path overrides are passed straight through to the Rust binary. This
 * client never opens, parses, validates, or defaults any of them: `models.toml`
 * and the current runtime config are Rust-owned authorities, and
 * reading them here would create a second one. Discovery, trust and defaults
 * belong exclusively to Rust.
 */

import type {
  RuntimePaths,
  RuntimeStartupOptions,
} from "./runtime/child-process.ts";

export const USAGE = `usage: rustx-tui --binary <rustx> [--models <models.toml>] \\
                 [--config <rustx.toml>] [--workspace <dir>] [--runtime-root <dir>] \\
                 [--model <provider/model>] [--trust grant|revoke] \\
                 [--inspect-conversation <conversation-id> | --resume | --session <id> [--node <id>]] \\
                 [--name <text>] [--skill <path>] [--no-automatic-skills] [--no-builtin-tools] [--no-direct-tools] \\
                 [--tools <a,b,c>] [--exclude-tools <a,b,c>]`;

export interface TuiArguments {
  binary: string;
  paths: RuntimePaths;
  startup: RuntimeStartupOptions;
  /**
   * Open the `/resume` selector over the initial fresh attachment. The chosen
   * identity is client routing state for the next subprocess.
   */
  openSessionSelector: boolean;
}

export class ArgumentError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ArgumentError";
  }
}

const VALUE_FLAGS = [
  "--model",
  "--trust",
  "--binary",
  "--models",
  "--config",
  "--workspace",
  "--runtime-root",
  "--inspect-conversation",
  "--session",
  "--node",
  "--name",
  "--skill",
  "--tools",
  "--exclude-tools",
] as const;

const BOOLEAN_FLAGS = [
  "--continue",
  "--resume",
  "--no-automatic-skills",
  "--no-builtin-tools",
  "--no-direct-tools",
] as const;

type ValueFlag = (typeof VALUE_FLAGS)[number];
type BooleanFlag = (typeof BOOLEAN_FLAGS)[number];

/**
 * Parses the argument vector.
 *
 * @throws {ArgumentError} on an unknown flag, a missing value, a repeated
 * flag, a missing required flag, or a combination of Session requests.
 */
export function parseArguments(argv: readonly string[]): TuiArguments {
  const values = new Map<ValueFlag, string>();
  const skillPaths: string[] = [];
  const booleans = new Set<BooleanFlag>();

  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag !== undefined && (VALUE_FLAGS as readonly string[]).includes(flag)) {
      const valueFlag = flag as ValueFlag;
      const value = argv[index + 1];
      if (value === undefined) {
        throw new ArgumentError(`argument ${valueFlag} requires a value`);
      }
      if (valueFlag === "--skill") {
        skillPaths.push(value);
      } else {
        if (values.has(valueFlag)) {
          throw new ArgumentError(`argument ${valueFlag} was supplied more than once`);
        }
        values.set(valueFlag, value);
      }
      index += 1;
      continue;
    }
    if (flag !== undefined && (BOOLEAN_FLAGS as readonly string[]).includes(flag)) {
      const booleanFlag = flag as BooleanFlag;
      if (booleans.has(booleanFlag)) {
        throw new ArgumentError(`argument ${booleanFlag} was supplied more than once`);
      }
      booleans.add(booleanFlag);
      continue;
    }
    if (flag === undefined) {
      throw new ArgumentError("unknown argument");
    }
    if (!(VALUE_FLAGS as readonly string[]).includes(flag)) {
      throw new ArgumentError(`unknown argument ${JSON.stringify(argv[index])}`);
    }
  }

  const required = (
    flag: Exclude<ValueFlag, "--skill" | "--tools" | "--exclude-tools">,
  ): string => {
    const value = values.get(flag);
    if (value === undefined) {
      throw new ArgumentError(`missing required argument ${flag}`);
    }
    return value;
  };

  const session = values.get("--session");
  const node = values.get("--node");
  const inspectConversation = values.get("--inspect-conversation");
  if (booleans.has("--continue")) throw new ArgumentError("use --session <id> or --resume; there is no global Session focus");
  const resume = booleans.has("--resume");
  const requests = [
    booleans.has("--continue") ? "--continue" : undefined,
    resume ? "--resume" : undefined,
    session !== undefined ? "--session" : undefined,
    inspectConversation !== undefined ? "--inspect-conversation" : undefined,
  ].filter((flag): flag is string => flag !== undefined);
  if (requests.length > 1) {
    throw new ArgumentError(
      `arguments ${requests.join(" and ")} cannot be combined`,
    );
  }
  if (node !== undefined && session === undefined) {
    throw new ArgumentError("argument --node requires --session");
  }
  if (inspectConversation !== undefined && node !== undefined) {
    throw new ArgumentError("argument --inspect-conversation cannot be combined with --node");
  }
  if (inspectConversation !== undefined && values.has("--name")) {
    throw new ArgumentError("argument --inspect-conversation cannot be combined with --name");
  }

  return {
    binary: required("--binary"),
    paths: {
      models: values.get("--models"),
      config: values.get("--config"),
      workspace: values.get("--workspace"),
      runtimeRoot: values.get("--runtime-root"),
    },
    openSessionSelector: resume,
    startup: {
      model: values.get("--model"),
      trust: values.get("--trust"),
      // `--resume` draws its picker over the Session the last launch left
      // active, so it publishes nothing of its own: cancelling the selector
      // leaves that Session bound rather than stranding an empty one in the
      // catalog beside it.
      continueActiveSession: false,
      inspectConversation,
      session,
      node,
      sessionName: values.get("--name"),
      skillPaths,
      noAutomaticSkills: booleans.has("--no-automatic-skills"),
      noBuiltinTools: booleans.has("--no-builtin-tools"),
      noDirectTools: booleans.has("--no-direct-tools"),
      tools: values.get("--tools"),
      excludeTools: values.get("--exclude-tools"),
    },
  };
}

/**
 * The arguments of a **replacement** spawn.
 *
 * Clear launch-only routing and naming. The caller supplies the chosen
 * Session/node explicitly for the next subprocess; the catalog has no focus.
 */
export function replacementArguments(parsed: TuiArguments): TuiArguments {
  return {
    ...parsed,
    openSessionSelector: false,
    startup: {
      ...parsed.startup,
      continueActiveSession: false,
      inspectConversation: undefined,
      session: undefined,
      node: undefined,
      sessionName: undefined,
    },
  };
}
