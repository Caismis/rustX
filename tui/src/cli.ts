/**
 * The bounded startup arguments of `rustx-tui`.
 *
 * ```text
 * rustx-tui --binary <path> --models <path> --config <path>
 *           --workspace <dir> --runtime-root <dir>
 *           [--skill <path>] [--no-skills]
 *           [--no-builtin-tools] [--no-tools]
 *           [--tools <a,b,c>] [--exclude-tools <a,b,c>]
 * ```
 *
 * The four runtime paths are passed straight through to the Rust binary. This
 * client never opens, parses, validates, or defaults any of them: `models.json`
 * and the current runtime config are Rust-owned authorities, and
 * reading them here would create a second one. Explicit arguments only — no
 * search path, no precedence, no profile discovery.
 */

import type {
  RuntimePaths,
  RuntimeStartupOptions,
} from "./runtime/child-process.ts";

export const USAGE = `usage: rustx-tui --binary <rustx> --models <models.json> \\
                 --config <rustx.json> --workspace <dir> --runtime-root <dir> \\
                 [--skill <path>] [--no-skills] [--no-builtin-tools] [--no-tools] \\
                 [--tools <a,b,c>] [--exclude-tools <a,b,c>]`;

export interface TuiArguments {
  binary: string;
  paths: RuntimePaths;
  startup: RuntimeStartupOptions;
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
  "--skill",
  "--tools",
  "--exclude-tools",
] as const;

const BOOLEAN_FLAGS = [
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
 * flag, or a missing required flag.
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

  return {
    binary: required("--binary"),
    paths: {
      models: required("--models"),
      config: required("--config"),
      workspace: required("--workspace"),
      runtimeRoot: required("--runtime-root"),
    },
    startup: {
      skillPaths,
      noSkills: booleans.has("--no-skills"),
      noBuiltinTools: booleans.has("--no-builtin-tools"),
      noTools: booleans.has("--no-tools"),
      tools: values.get("--tools"),
      excludeTools: values.get("--exclude-tools"),
    },
  };
}
