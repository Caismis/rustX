/**
 * The bounded startup arguments of `rustx-tui`.
 *
 * ```text
 * rustx-tui --binary <path> --models <path> --session <path>
 *           --workspace <dir> --runtime-root <dir>
 * ```
 *
 * The four runtime paths are passed straight through to the Rust binary. This
 * client never opens, parses, validates, or defaults any of them: `models.json`
 * and the bootstrap conversation config are Rust-owned authorities, and
 * reading them here would create a second one. Explicit arguments only — no
 * search path, no precedence, no profile discovery.
 */

import type { RuntimePaths } from "./runtime/child-process.ts";

export const USAGE = `usage: rustx-tui --binary <rustx> --models <models.json> \\
                 --session <bootstrap-config.json> --workspace <dir> --runtime-root <dir>`;

export interface TuiArguments {
  binary: string;
  paths: RuntimePaths;
}

export class ArgumentError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ArgumentError";
  }
}

const FLAGS = [
  "--binary",
  "--models",
  "--session",
  "--workspace",
  "--runtime-root",
] as const;

type Flag = (typeof FLAGS)[number];

/**
 * Parses the argument vector.
 *
 * @throws {ArgumentError} on an unknown flag, a missing value, a repeated
 * flag, or a missing required flag.
 */
export function parseArguments(argv: readonly string[]): TuiArguments {
  const values = new Map<Flag, string>();

  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index] as Flag;
    if (!FLAGS.includes(flag)) {
      throw new ArgumentError(`unknown argument ${JSON.stringify(argv[index])}`);
    }
    const value = argv[index + 1];
    if (value === undefined) {
      throw new ArgumentError(`argument ${flag} requires a value`);
    }
    if (values.has(flag)) {
      throw new ArgumentError(`argument ${flag} was supplied more than once`);
    }
    values.set(flag, value);
    index += 1;
  }

  const required = (flag: Flag): string => {
    const value = values.get(flag);
    if (value === undefined) {
      throw new ArgumentError(`missing required argument ${flag}`);
    }
    return value;
  };

  return {
    binary: required("--binary"),
    paths: {
      models: required("--models"),
      session: required("--session"),
      workspace: required("--workspace"),
      runtimeRoot: required("--runtime-root"),
    },
  };
}
