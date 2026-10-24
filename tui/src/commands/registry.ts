/**
 * The bounded rustX TUI command surface.
 *
 * Commands are *TUI interactions*, not an alternate runtime API. Each one
 * either renders state the projection already holds or invokes one canonical
 * App Server operation. None defines parallel agent semantics, and none
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
 * no Skill invocation: Tool and Skill behaviour travel through
 * native capabilities. Explicit /attach uploads selected bytes to the Session.
 */
export const COMMANDS: readonly CommandSpec[] = [
  { name: '/workflow-recovery', description: 'Inspect native scheduled Workflow ownership and committed results.', argumentHint: '<start-id> [node-id] [after-effect-ordinal]' },
  { name: '/schedules', description: 'Inspect durable schedules for an exact Session node.', argumentHint: '[node-id] [after-ordinal]' },
  { name: '/context-model', description: 'Inspect the original native context model in bounded byte ranges.', argumentHint: '[byte-offset]' },
  { name: "/capabilities", description: "Inspect all native Agent capabilities in a scrollable view." },
  { name: "/attach", description: "Upload a selected local file to this Session.", argumentHint: "<local-path>" },
  { name: "/queue", description: "Edit or remove exact observed queued Human input." },
  { name: "/export", description: "Export this Session archive to a client-local file.", argumentHint: "[output-path]" },
  {
    name: "/settings",
    description: "Read, rescan, and edit User/Workspace configuration, or inspect its automatic repairs.",
    argumentHint: "[repairs | user … | workspace …]",
  },
  { name: "/content", description: "List deferred content and read one bounded byte range.", argumentHint: "[range-number] [byte-offset]" },
  { name: "/help", description: "List the available commands." },
  { name: "/goal", description: "Show, create, pause, resume, edit, or budget a persistent Goal." },
  {
    name: "/model",
    description:
      "Select a model while work runs; changing models resets primary overrides and preserves summary policy. Existing attempts stay frozen.",
    argumentHint: "[show|provider/model|profile set <id>|profile clear]",
  },
  { name: "/model-position", description: "Select native catalog defaults from an exact model binding revision and finite model/profile ordinals.", argumentHint: "<binding-revision> <model-position> [profile-position]" },
  { name: "/new", description: "Create a new independent local session." },
  {
    name: "/resume",
    description: "Search persisted sessions; Enter resumes, Ctrl+D previews permanent deletion.",
    argumentHint: "[session-id]",
  },
  {
    name: "/session",
    argumentHint: "[settings | adopt]",
    description: "Show Session metadata, settings, or explicitly adopt the inspected candidate.",
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
    name: "/context",
    description: "Inspect historical model projections: history [activation] or projection <checkpoint> [offset].",
  },
  {
    name: "/compact",
    description: "Manually compact canonical conversation context while the runtime is idle.",
  },
  {
    name: "/debug",
    description:
      "Show bounded presentation, runtime, and protocol diagnostics.",
  },
  {
    name: "/show-reasoning",
    description:
      "Show or hide model reasoning. A client display preference; it never changes what rustX requests.",
    argumentHint: "[on|off]",
  },
  {
    name: "/expand",
    description:
      "Expand or collapse foreground tool, background execution, and pending interaction detail. Purely visual: nothing is re-executed or re-fetched.",
    argumentHint:
      "[latest|all|none|<tool-call-id>|job <job-id>|interaction <conversation-id>::<interaction-id>]",
  },
  {
    name: "/cancel",
    description:
      "Request cancellation of the current attempt.",
  },
  { name: "/observation", description: "Inspect a bounded native domain or finite Trace head.", argumentHint: "[domain [page-offset] | trace <native-record-id>]" },
  { name: "/observation-value", description: "Read one byte range from a selected native domain value." },
  { name: "/agents", description: "Read one bounded Child Agent page; follow its displayed continuation." },
  { name: "/agent-read", description: "Observe one Child Agent by its exact id, including older Agent pages." },
  { name: "/send-message", description: "Send to an Agent; the runtime delivers or resumes atomically.", argumentHint: "<agent-id> <message>" },
  { name: "/wait-agent", description: "Wait for the Agent activation observed by the runtime.", argumentHint: "<agent-id>" },
  { name: "/interrupt-agent", description: "Interrupt the current activation; keep the Agent resumable.", argumentHint: "<agent-id>" },
  { name: "/jobs", description: "List finite background Jobs." },
  { name: "/job-status", description: "Read one Job without waiting.", argumentHint: "<job-id>" },
  { name: "/job-wait", description: "Wait for one exact Job to physically settle.", argumentHint: "<job-id>" },
  { name: "/job-cancel", description: "Cancel one finite Job through runtime settlement.", argumentHint: "<job-id>" },
  {
    name: "/files",
    description:
      "Browse files delivered by `present` in this Session's committed history; save or open them when this connection holds delivery access. Starts no model request.",
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
