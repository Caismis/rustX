/**
 * The typed Agent Status presentation.
 *
 * Agent Status is runtime-owned model-visible context, not a conversation
 * speaker. Its canonical message is a hidden `Context(AgentStatus)` User
 * message and stays out of the ordinary transcript; what a reader sees is an
 * *annotation*, drawn subordinate to the turn it belongs to.
 *
 * Everything here reads {@link AgentStatusView.sections} — the runtime's
 * closed typed section vocabulary:
 *
 * ```text
 * temporal               the sampled runtime clock and its timezone
 * background_executions  active executions, with the module's own omission count
 * todo                   the bounded committed task view and its counts
 * ```
 *
 * The runtime also publishes `rendered`, the exact text the model saw. It is
 * diagnostics: this file never parses it, because recovering structure from a
 * model-facing string would make the client a second interpreter of a
 * composition it already receives structurally.
 *
 * One facet model serves both surfaces, so the transcript annotation and
 * `/status` can never drift into two vocabularies:
 *
 * ```text
 * agentStatusFacets     the typed sections, as label/value/detail
 *   -> compact form     ◇ status · 10:42 CST · todo 3 · background 2
 *   -> detail form      /status, one labelled block per section
 * ```
 *
 * Every string a facet produces is bounded. Todo subjects, background tool
 * names, and an unparseable published instant are all externally derived and
 * may be arbitrarily long; a status annotation must stay one short line
 * however narrow the terminal is, and must never reflow the transcript rows
 * around it.
 */

import type {
  AgentStatusView,
  RuntimeClientStatusSection,
  RuntimeClientTodoStatusTask,
} from "../../protocol/types.ts";
import { role } from "../theme.ts";

/** The glyph that marks contextual runtime metadata, not a speaker. */
const STATUS_GLYPH = "◇";

/** How much of one externally derived value a facet shows. */
const VALUE_LIMIT = 64;

/** How much of one externally derived value the detail form shows. */
const DETAIL_LIMIT = 160;

/** How many background entries the detail form lists before summarizing. */
const DETAIL_ENTRY_LIMIT = 6;

/**
 * One typed section, reduced to presentation values.
 *
 * `kind` mirrors the runtime's section identity so a caller can style by
 * section without re-deciding which section it is looking at.
 */
export interface AgentStatusFacet {
  kind: "temporal" | "background" | "todo";
  /** The one-line compact form, already bounded: `todo 3`, `10:42 CST`. */
  compact: string;
  /** The detail form's label column. */
  label: string;
  /** The detail form's value lines, already bounded. Never empty. */
  values: string[];
}

/**
 * The typed facets of one composition, in the runtime's own section order.
 *
 * A section the runtime published with nothing to say — no active tasks, no
 * active executions — contributes no facet: an annotation that says
 * `background 0` states a fact nobody asked about and costs a line of the
 * conversation.
 */
export function agentStatusFacets(status: AgentStatusView): AgentStatusFacet[] {
  const facets: AgentStatusFacet[] = [];
  for (const section of status.sections) {
    const facet = facetOf(section);
    if (facet !== undefined) {
      facets.push(facet);
    }
  }
  return facets;
}

function facetOf(
  section: RuntimeClientStatusSection,
): AgentStatusFacet | undefined {
  switch (section.type) {
    case "temporal": {
      const time = formatStatusTime(section.current_time, section.timezone);
      return {
        kind: "temporal",
        compact: time,
        label: "time",
        values: [time],
      };
    }
    case "background_executions": {
      const executions = section.executions ?? [];
      const total = executions.length + section.omitted_count;
      if (total === 0) {
        return undefined;
      }
      const values = executions
        .slice(0, DETAIL_ENTRY_LIMIT)
        .map((execution) =>
          clip(
            `${execution.tool_name} · ${execution.state}`,
            DETAIL_LIMIT,
          ),
        );
      const hidden =
        section.omitted_count +
        Math.max(0, executions.length - DETAIL_ENTRY_LIMIT);
      if (hidden > 0) {
        values.push(`… and ${hidden} more`);
      }
      return {
        kind: "background",
        compact: `background ${total}`,
        label: "background",
        values,
      };
    }
    case "todo": {
      if (section.active_count === 0) {
        return undefined;
      }
      const values: string[] = [];
      if (section.current !== undefined) {
        values.push(clip(todoSubject(section.current), DETAIL_LIMIT));
      }
      // Counts restate the runtime's own committed totals; they are never
      // recomputed from the bounded task list, which may omit entries.
      const counts = [`${section.active_count} active`];
      if (section.blocked_count > 0) {
        counts.push(`${section.blocked_count} blocked`);
      }
      if (section.completed_count > 0) {
        counts.push(`${section.completed_count} completed`);
      }
      values.push(counts.join(" · "));
      return {
        kind: "todo",
        compact: `todo ${section.active_count}`,
        label: "todo",
        values,
      };
    }
  }
}

/**
 * The compact one-line annotation body, without styling.
 *
 * `heading` distinguishes the two placements a composition can have — the
 * annotation of an inbound turn, and a standalone update after a settled tool
 * batch — and is the only thing that differs between them.
 */
export function agentStatusSummary(
  status: AgentStatusView,
  heading: string,
): string {
  const parts = agentStatusFacets(status).map((facet) =>
    clip(facet.compact, VALUE_LIMIT),
  );
  return [`${STATUS_GLYPH} ${heading}`, ...parts].join(" · ");
}

/** How a composed status is placed, which is the only visual difference. */
export type AgentStatusAnnotationForm = "attached" | "standalone";

/**
 * The styled transcript annotation.
 *
 * One line, always. It is drawn in the metadata role and carries no
 * background band: the user band belongs to what the human said, and a
 * runtime annotation that borrowed it would read as part of the turn.
 */
export function renderAgentStatusAnnotation(
  status: AgentStatusView,
  form: AgentStatusAnnotationForm,
): string {
  const heading = form === "attached" ? "status" : "status update";
  return role.meta(agentStatusSummary(status, heading));
}

/**
 * The expanded typed presentation, as Markdown lines.
 *
 * Used by `/status`, from the same facets the annotation uses, so the two
 * surfaces cannot describe one composition differently.
 */
export function renderAgentStatusDetail(status: AgentStatusView): string[] {
  const facets = agentStatusFacets(status);
  if (facets.length === 0) {
    return ["The latest composition contributed no sections."];
  }
  const lines: string[] = [];
  for (const facet of facets) {
    lines.push(`- **${facet.label}** — ${facet.values[0] ?? ""}`);
    for (const value of facet.values.slice(1)) {
      lines.push(`  - ${value}`);
    }
  }
  return lines;
}

/** The in-progress label when the runtime published one, else the subject. */
function todoSubject(task: RuntimeClientTodoStatusTask): string {
  return task.status === "in_progress" && task.active_form !== undefined
    ? task.active_form
    : task.subject;
}

/**
 * The sampled runtime clock, in the timezone the runtime configured.
 *
 * The instant and the timezone are both runtime-published: nothing here reads
 * the local clock or the local zone, so the same composition renders the same
 * way on every machine. An instant the runtime published in a shape this
 * client cannot parse is shown verbatim rather than replaced by a guess.
 */
export function formatStatusTime(
  currentTime: string,
  timezone?: string,
): string {
  const instant = new Date(currentTime);
  if (Number.isNaN(instant.getTime())) {
    return clip(currentTime, VALUE_LIMIT);
  }
  const zone = timezone ?? "UTC";
  const parts = zonedParts(instant, zone) ?? zonedParts(instant, "UTC");
  if (parts === undefined) {
    return instant.toISOString();
  }
  return parts;
}

function zonedParts(instant: Date, zone: string): string | undefined {
  try {
    const parts = new Intl.DateTimeFormat("en-US", {
      timeZone: zone,
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
      timeZoneName: "short",
    }).formatToParts(instant);
    const hour = parts.find((part) => part.type === "hour")?.value;
    const minute = parts.find((part) => part.type === "minute")?.value;
    const name = parts.find((part) => part.type === "timeZoneName")?.value;
    if (hour === undefined || minute === undefined) {
      return undefined;
    }
    return name === undefined ? `${hour}:${minute}` : `${hour}:${minute} ${name}`;
  } catch {
    // An unknown IANA zone is a runtime fact this client cannot render, not a
    // reason to fail a transcript render.
    return undefined;
  }
}

/** Bounds one externally derived value without rewriting it into a lie. */
function clip(text: string, limit: number): string {
  const flat = text.replace(/\s+/gu, " ").trim();
  const characters = [...flat];
  if (characters.length <= limit) {
    return flat;
  }
  return `${characters.slice(0, Math.max(0, limit - 1)).join("")}…`;
}
