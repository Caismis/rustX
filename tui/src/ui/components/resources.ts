/**
 * The loaded-resources banner shown once, before the first turn.
 *
 * ```text
 * [Context]
 *   AGENTS.md, /project/AGENTS.md
 *
 * [Skills]
 *   commit, review, release
 *
 * [Tools]
 *   bash, edit, glob, grep, read, write
 * ```
 *
 * It answers one question — *what is this runtime actually running with?* —
 * and it answers it entirely from what the runtime published. Every name
 * below is a projection field:
 *
 * ```text
 * Context   RuntimeClientResourcesView.context_files
 * Skills    CapabilityView.skills
 * Tools     CapabilityView.tools        (the active, model-visible set)
 * ```
 *
 * Nothing here reads a file, walks a directory, or infers a resource the
 * runtime did not name. A section the runtime published nothing for is
 * absent, not empty: `[Skills]` with no skills under it would be this client
 * asserting that a Skill catalog exists.
 *
 * `Tools` is the *active* catalog rather than the available one, because
 * availability is not activation: a tool the runtime can construct but did
 * not put in front of the model is not something this session is running
 * with. `/tools` shows both, with the distinction spelled out.
 *
 * Paths are shortened for display only. A shortened path is still the same
 * path — workspace-relative when the file is inside the workspace, `~`-based
 * when it is under the home directory, absolute otherwise — and the full
 * value stays one `/status` away.
 */

import { homedir } from "node:os";
import { isAbsolute, relative, resolve, sep } from "node:path";

import type { PresentationState } from "../../presentation/state.ts";
import { skills } from "../../presentation/selectors.ts";
import { role, style } from "../theme.ts";

/** What the banner needs beyond the projection: where "here" is. */
export interface ResourceBannerContext {
  /** The workspace the runtime was launched against, when the client knows it. */
  workspace?: string;
}

/**
 * The banner, or `""` when the runtime published no resources at all.
 *
 * An empty string is the honest rendering of a runtime with no project
 * instructions, no Skills, and no active Tools; the caller draws nothing
 * rather than an empty frame.
 */
export function renderResourceBanner(
  state: PresentationState,
  context: ResourceBannerContext = {},
): string {
  const sections: string[] = [];

  // Context files keep the runtime's own order — root-most to workspace, the
  // order it concatenated them in — because that order is what decides which
  // instruction wins. Sorting them alphabetically would misreport precedence.
  const contextFiles = (state.resources.context_files ?? []).map((file) =>
    displayPath(file.path, context.workspace),
  );
  if (state.resources.agent_profile) {
    contextFiles.push("agent profile");
  }
  if (contextFiles.length > 0) {
    sections.push(section("Context", contextFiles));
  }

  const skillNames = skills(state).map((skill) => skill.name);
  if (skillNames.length > 0) {
    sections.push(section("Skills", sorted(skillNames)));
  }

  const toolNames = (state.capabilities.tools ?? []).map((tool) => tool.name);
  if (toolNames.length > 0) {
    sections.push(section("Tools", sorted(toolNames)));
  }

  return sections.join("\n\n");
}

/** One `[Name]` heading over its own compact, comma-joined list. */
function section(name: string, items: readonly string[]): string {
  return `${style.heading(`[${name}]`)}\n${role.meta(`  ${items.join(", ")}`)}`;
}

/** Locale-stable ordering, so the same catalog always reads the same way. */
function sorted(items: readonly string[]): string[] {
  return [...items].sort((left, right) => left.localeCompare(right));
}

/**
 * The shortest honest spelling of one absolute path.
 *
 * Workspace-relative wins when the file is inside the workspace, because that
 * is how a reader refers to it. `~` wins next. A path outside both stays
 * absolute: shortening it would need a reference point the reader does not
 * have.
 */
export function displayPath(path: string, workspace?: string): string {
  if (workspace !== undefined && workspace.length > 0) {
    const base = resolve(workspace);
    const relativePath = relative(base, resolve(path));
    if (
      relativePath.length > 0 &&
      !relativePath.startsWith(`..${sep}`) &&
      relativePath !== ".." &&
      !isAbsolute(relativePath)
    ) {
      return relativePath;
    }
  }
  const home = homedir();
  if (home.length > 0 && path.startsWith(`${home}${sep}`)) {
    return `~${path.slice(home.length)}`;
  }
  return path;
}
