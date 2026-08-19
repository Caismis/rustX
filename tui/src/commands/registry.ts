/**
 * The bounded rustX TUI command surface.
 *
 * Commands are *TUI interactions*, not an alternate runtime API. Each one
 * either renders state the projection already holds or invokes one canonical
 * Runtime Client operation. None defines parallel agent semantics, and none
 * reads a file, spawns a shell, or talks to a provider.
 *
 * The command spelling is presentation. Changing `/tools` to `/capabilities`
 * would change nothing about rustX.
 */

export interface CommandSpec {
  name: string;
  description: string;
  argumentHint?: string;
}

/**
 * The command table.
 *
 * Deliberately small. There is no `!bash` escape, no `@file` attachment, and
 * no Skill invocation: shell, file, and Skill behaviour must travel through
 * the real rustX tool and capability path, and rustX has not yet defined a
 * client-facing attachment contract.
 */
export const COMMANDS: readonly CommandSpec[] = [
  { name: "/help", description: "List the available commands." },
  {
    name: "/model",
    description:
      "Inspect the session model, or select one; selection resets primary overrides and preserves summary policy.",
    argumentHint: "[provider/model]",
  },
  {
    name: "/tools",
    description: "Show the active tool catalog from the capability projection.",
  },
  {
    name: "/skills",
    description: "Show the active Skill catalog from the capability projection.",
  },
  {
    name: "/status",
    description: "Show the runtime-composed Agent Status and runtime diagnostics.",
  },
  {
    name: "/debug",
    description: "Show bounded presentation and protocol diagnostics.",
  },
  {
    name: "/cancel",
    description:
      "Request cancellation of the current attempt, or of a background execution.",
    argumentHint: "[execution-id]",
  },
  {
    name: "/approve",
    description: "Answer one runtime-owned approval interaction.",
    argumentHint: "<interaction-id> <allow|deny> [reason]",
  },
  { name: "/quit", description: "Shut down the runtime and exit cleanly." },
];

/** Splits an input line into a command name and its raw argument text. */
export function parseCommandLine(
  line: string,
): { name: string; argument: string } | undefined {
  const trimmed = line.trim();
  if (!trimmed.startsWith("/")) {
    return undefined;
  }
  const space = trimmed.indexOf(" ");
  if (space === -1) {
    return { name: trimmed, argument: "" };
  }
  return {
    name: trimmed.slice(0, space),
    argument: trimmed.slice(space + 1).trim(),
  };
}
