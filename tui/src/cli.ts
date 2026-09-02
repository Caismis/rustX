/**
 * The bounded startup arguments of `rustx-tui`.
 *
 * ```text
 * rustx-tui --binary <path> --models <path> --config <path>
 *           --workspace <dir> --runtime-root <dir>
 *           [--inspect-conversation <id> | --continue | --resume | --session <id> [--node <id>]]
 *           [--name <text>] [--skill <path>] [--no-skills]
 *           [--no-builtin-tools] [--no-tools]
 *           [--tools <a,b,c>] [--exclude-tools <a,b,c>]
 * ```
 *
 * Without a Session request the runtime starts on an empty Session and every
 * persisted Session stays reachable through `/resume`. Conversation inspection
 * is a separate read-only attachment by known conversation identity. The
 * remaining three requests change the startup Session, and they are mutually
 * exclusive with inspection:
 *
 * - `--continue` starts on the Session the previous launch left active;
 * - `--session <id>` (optionally `--node <id>`) starts on a named persisted
 *   Session;
 * - `--resume` opens the `/resume` selector over the continued Session as
 *   soon as the client attaches, so a Session can be chosen instead of named.
 * - `--inspect-conversation <id>` opens the ordinary Runtime Client projection
 *   for that durable conversation without composing a Session or execution
 *   owner.
 *
 * `--name` is not one of those requests. It names the Session the launch
 * bound, whichever one that is, and it is forwarded to Rust like every other
 * startup control — a Session is never opened by its name.
 *
 * Only `--resume` is a client behaviour, and only because drawing a picker is
 * what a terminal client is for: it forwards `--continue`, then issues the
 * ordinary `/resume` selection for whatever the user picks. Every other part
 * of the decision is Rust's — this client neither reads the catalog nor
 * resolves an identity it was given.
 *
 * The four runtime paths are passed straight through to the Rust binary. This
 * client never opens, parses, validates, or defaults any of them: `models.jsonc`
 * and the current runtime config are Rust-owned authorities, and
 * reading them here would create a second one. Explicit arguments only — no
 * search path, no precedence, no profile discovery.
 */

import type {
  RuntimePaths,
  RuntimeStartupOptions,
} from "./runtime/child-process.ts";

export const USAGE = `usage: rustx-tui --binary <rustx> --models <models.jsonc> \\
                 --config <rustx.jsonc> --workspace <dir> --runtime-root <dir> \\
                 [--inspect-conversation <conversation-id> | --continue | --resume | --session <id> [--node <id>]] \\
                 [--name <text>] [--skill <path>] [--no-skills] [--no-builtin-tools] [--no-tools] \\
                 [--tools <a,b,c>] [--exclude-tools <a,b,c>]`;

export interface TuiArguments {
  binary: string;
  paths: RuntimePaths;
  startup: RuntimeStartupOptions;
  /**
   * Open the `/resume` selector as soon as the client attaches. This is the
   * only startup Session decision the client makes, and it makes none of it
   * alone: the picker is drawn over the continued Session and the choice
   * becomes an ordinary `/resume` selection Rust publishes.
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
  "--no-skills",
  "--no-builtin-tools",
  "--no-tools",
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
      models: required("--models"),
      config: required("--config"),
      workspace: required("--workspace"),
      runtimeRoot: required("--runtime-root"),
    },
    openSessionSelector: resume,
    startup: {
      // `--resume` draws its picker over the Session the last launch left
      // active, so it publishes nothing of its own: cancelling the selector
      // leaves that Session bound rather than stranding an empty one in the
      // catalog beside it.
      continueActiveSession: booleans.has("--continue") || resume,
      inspectConversation,
      session,
      node,
      sessionName: values.get("--name"),
      skillPaths,
      noSkills: booleans.has("--no-skills"),
      noBuiltinTools: booleans.has("--no-builtin-tools"),
      noTools: booleans.has("--no-tools"),
      tools: values.get("--tools"),
      excludeTools: values.get("--exclude-tools"),
    },
  };
}

/**
 * The arguments of a **replacement** spawn.
 *
 * A replacement is never a new launch: it completes a Session transition the
 * runtime has already published durably, so it continues the catalog's active
 * selection however this client was started. The destination is not named
 * here — the catalog stays the authority for it — which is exactly why a
 * launch-time `--session`/`--node` is dropped rather than repeated: replaying
 * it would re-select the Session the user has just switched away from. A
 * launch-time `--resume` is dropped for the same reason: the selector is the
 * choice this replacement is already carrying out. `--name` is dropped
 * because it named the Session the user launched into: repeating it would
 * put that label on whichever Session they switched to next.
 */
export function replacementArguments(parsed: TuiArguments): TuiArguments {
  return {
    ...parsed,
    openSessionSelector: false,
    startup: {
      ...parsed.startup,
      continueActiveSession: true,
      inspectConversation: undefined,
      session: undefined,
      node: undefined,
      sessionName: undefined,
    },
  };
}
