import { Container, Loader, Spacer, Text, type TUI } from "@earendil-works/pi-tui";
import type { CollectionFrontier } from "../../../protocol/app-server/v57.ts";
import type { PresentationState } from "../presentation/state.ts";
import { correlateTools } from "../presentation/tools.ts";
import { selectTodos } from "../presentation/todos.ts";
import type { InteractionRef, SessionView } from "../protocol/app-server.ts";
import { sanitizeField } from "../sanitize.ts";
import {
  renderBackgroundSection,
  renderInteractionSection,
  renderOrphanExecutions,
  renderSubagentSection,
} from "./components/activity.ts";
import { renderResourceBanner } from "./components/resources.ts";
import { renderStartup, startupVisible, workingStatus } from "./components/status.ts";
import { renderTodoPanel, deferredTodoRecords } from "./components/todos.ts";
import { renderTranscript } from "./components/transcript.ts";
import { banded } from "./components/transcript-block.ts";
import type { PresentationPreferences } from "./preferences.ts";
import { style } from "./theme.ts";

/** Everything one total render reads besides the projection itself. */
export interface ConversationRenderContext {
  preferences: PresentationPreferences;
  sessionInfo: SessionView | undefined;
  workspace: string | undefined;
  /** The Agent row highlighted while the Agent list holds focus. */
  selectedAgentId: string | undefined;
  interactionFocus: InteractionRef | undefined;
  collections: readonly CollectionFrontier[] | undefined;
}

/**
 * The conversation area of the terminal: welcome, transcript, activity,
 * task panel and the working indicator.
 *
 * Every render is total: each component is discarded and reconstructed from
 * the projection, which is what makes the UI reconstructable from a fresh
 * snapshot — no Pi component carries state the projection does not have.
 */
export class ConversationView {
  readonly startup = new Container();
  readonly transcript = new Container();
  readonly activity = new Container();
  /**
   * The task panel, drawn between the conversation and the editor.
   *
   * It is the plan the reader needs *while typing the next message*, so it
   * sits at the bottom of the scrollback rather than inside the transcript,
   * and it is rebuilt from the projection like every other component here.
   */
  readonly todos = new Container();
  readonly #loader: Loader;
  readonly #tui: TUI;

  constructor(tui: TUI) {
    this.#tui = tui;
    this.#loader = new Loader(tui, style.cyan, style.dim, "");
  }

  /** Nothing is shown and nothing is working. */
  clear(): void {
    this.startup.clear(); this.transcript.clear(); this.activity.clear(); this.todos.clear();
    this.#loader.stop();
  }
  /** The working indicator stops and the activity area empties. */
  stopWorking(): void { this.#loader.stop(); this.activity.clear(); }
  /** The working indicator stops; terminal shutdown. */
  stop(): void { this.#loader.stop(); }
  /** One startup line, replacing or following what the welcome area holds. */
  notice(text: string, replace = true): void {
    if (replace) this.startup.clear();
    this.startup.addChild(new Text(text, 1, 0));
  }

  /** Durable history and retained input while the first attachment connects. */
  renderConnecting(history: PresentationState | undefined, queued: string | undefined, preferences: PresentationPreferences): void {
    this.clear();
    if (history) for (const block of renderTranscript(history, preferences, correlateTools(history))) {
      this.transcript.addChild(banded(block)); this.transcript.addChild(new Spacer(1));
    }
    if (queued !== undefined) {
      this.transcript.addChild(new Text(sanitizeField(queued, true), 1, 0));
      this.#loader.setMessage("Connecting… Esc to withdraw.");
      this.#loader.start(); this.activity.addChild(this.#loader);
    }
  }

  /** Rebuilds the visible conversation from the projection. */
  render(state: PresentationState, context: ConversationRenderContext): void {
    const { preferences } = context;
    // Correlated once per render and shared: the transcript and the activity
    // area must agree on which calls have a transcript anchor.
    const correlation = correlateTools(state);

    // The welcome block is useful only before the first real turn. Session
    // metadata is refreshed from the native Session projection and is never
    // reconstructed from client or attachment identifiers.
    this.startup.clear();
    if (startupVisible(state)) {
      this.startup.addChild(new Text(renderStartup(state, context.sessionInfo, this.#tui.terminal.columns), 1, 0));
      // What the runtime is actually running with — its project context
      // files, its Skill catalog, its active Tools — stated once, before the
      // first turn, entirely from the capability and resource projections.
      const resources = renderResourceBanner(state, { workspace: context.workspace });
      if (resources.length > 0) {
        this.startup.addChild(new Spacer(1));
        this.startup.addChild(new Text(resources, 1, 0));
      }
    }

    this.transcript.clear();
    for (const block of renderTranscript(state, preferences, correlation)) {
      this.transcript.addChild(banded(block));
      this.transcript.addChild(new Spacer(1));
    }

    // The activity area holds only what is *not* conversation content. A
    // foreground tool call renders inside the assistant message that asked
    // for it, which is what keeps one call to one card.
    this.activity.clear();
    for (const section of [
      renderOrphanExecutions(correlation, preferences),
      renderBackgroundSection(state, preferences),
      renderSubagentSection(state, preferences, new Date(), context.selectedAgentId),
      renderInteractionSection(state, preferences, context.interactionFocus),
    ]) {
      if (section.length > 0) this.activity.addChild(new Text(section, 1, 0));
    }

    // The native loaded task prefix and its independently advertised range.
    // Extension absence and a deferred nonempty list remain distinct.
    this.todos.clear();
    const todos = renderTodoPanel(selectTodos(state), {
      columns: this.#tui.terminal.columns,
      deferredTasks: deferredTodoRecords(context.collections),
    });
    if (todos.length > 0) this.todos.addChild(new Text(todos, 1, 0));

    const working = workingStatus(state);
    if (working === undefined) {
      this.#loader.stop();
    } else {
      this.#loader.setMessage(working);
      this.#loader.start();
      this.activity.addChild(this.#loader);
    }
  }
}
