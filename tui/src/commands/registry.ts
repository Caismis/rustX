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
      "Open the searchable model selector, or select one directly; selection resets primary overrides and preserves summary policy.",
    argumentHint: "[show|provider/model]",
  },
  { name: "/new", description: "Create a new independent local session." },
  {
    name: "/resume",
    description: "Search persisted sessions and activate one.",
    argumentHint: "[session-id]",
  },
  {
    name: "/session",
    description: "Show the active session, node, and conversation metadata.",
  },
  {
    name: "/name",
    description:
      "Show the active session's name, or give it one; a name is display metadata only.",
    argumentHint: "[text]",
  },
  { name: "/clone", description: "Clone the committed conversation head into a new session." },
  {
    name: "/fork",
    description: "Select an earlier user message and open an editable fork.",
  },
  {
    name: "/tree",
    description: "Inspect session lineages or branch from a historical user message.",
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
    name: "/todos",
    description:
      "Show the complete task list the agent is tracking, grouped by status. The panel above the editor shows the same list, bounded.",
  },
  {
    name: "/status",
    description:
      "Show the latest runtime-composed Agent Status in full. Runtime and client diagnostics are in /debug.",
  },
  {
    name: "/compact",
    description: "Manually compact canonical conversation context while the runtime is idle.",
  },
  {
    name: "/reload",
    description: "Atomically reload project, Skill, extension, and Tool resources for future attempts.",
  },
  {
    name: "/debug",
    description:
      "Show bounded presentation, runtime, and protocol diagnostics.",
  },
  {
    name: "/reasoning",
    description:
      "Show or hide model reasoning. A client display preference; it never changes what rustX requests.",
    argumentHint: "[on|off]",
  },
  {
    name: "/expand",
    description:
      "Expand or collapse foreground tool, background execution, and pending interaction detail. Purely visual: nothing is re-executed or re-fetched.",
    argumentHint:
      "[latest|all|none|<tool-call-id>|background <execution-id>|interaction <conversation-id>::<interaction-id>]",
  },
  {
    name: "/cancel",
    description:
      "Request cancellation of the current attempt, or of a background execution.",
    argumentHint: "[execution-id]",
  },
  {
    name: "/approval",
    description: "Request the runtime ApprovalMode.",
    argumentHint: "<policy|full_access>",
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
