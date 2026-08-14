/**
 * Slash-command autocomplete for the rustX editor.
 *
 * This implements Pi TUI's `AutocompleteProvider` interface and completes
 * *only* rustX TUI commands.
 *
 * Pi's own `CombinedAutocompleteProvider` is deliberately not used: it walks
 * the local filesystem and may shell out to `fd`, which would put a Node-side
 * workspace reader inside the client. Until rustX defines a canonical
 * client-facing resource/attachment contract there is no `@file` completion,
 * no path completion, and no filesystem prompt injection here.
 */

import type {
  AutocompleteItem,
  AutocompleteProvider,
  AutocompleteSuggestions,
} from "@earendil-works/pi-tui";
import { fuzzyFilter } from "@earendil-works/pi-tui";

import { COMMANDS, type CommandSpec } from "./registry.ts";

export class SlashCommandAutocompleteProvider implements AutocompleteProvider {
  readonly triggerCharacters = ["/"];
  readonly #commands: readonly CommandSpec[];

  constructor(commands: readonly CommandSpec[] = COMMANDS) {
    this.#commands = commands;
  }

  getSuggestions(
    lines: string[],
    cursorLine: number,
    cursorCol: number,
    // Completion is a synchronous table lookup, so there is nothing to abort
    // and nothing to force; the parameter exists to satisfy the interface.
    _options: { signal: AbortSignal; force?: boolean },
  ): Promise<AutocompleteSuggestions | null> {
    const prefix = commandPrefix(lines, cursorLine, cursorCol);
    if (prefix === undefined) {
      return Promise.resolve(null);
    }

    const items: AutocompleteItem[] = this.#commands.map((command) => ({
      value: command.name,
      label:
        command.argumentHint === undefined
          ? command.name
          : `${command.name} ${command.argumentHint}`,
      description: command.description,
    }));

    const matched =
      prefix === "/"
        ? items
        : fuzzyFilter(items, prefix.slice(1), (item) => item.value);

    return Promise.resolve(
      matched.length === 0 ? null : { items: matched, prefix },
    );
  }

  applyCompletion(
    lines: string[],
    cursorLine: number,
    cursorCol: number,
    item: AutocompleteItem,
    prefix: string,
  ): { lines: string[]; cursorLine: number; cursorCol: number } {
    const line = lines[cursorLine] ?? "";
    const start = cursorCol - prefix.length;
    const replaced = `${line.slice(0, start)}${item.value} ${line.slice(cursorCol)}`;
    const updated = [...lines];
    updated[cursorLine] = replaced;
    return {
      lines: updated,
      cursorLine,
      cursorCol: start + item.value.length + 1,
    };
  }

  /**
   * File completion is never triggered.
   *
   * Returning false keeps Pi's filesystem path off entirely; the client has
   * no workspace reader by design.
   */
  shouldTriggerFileCompletion(): boolean {
    return false;
  }
}

/**
 * The command token under the cursor, when the line is a command line.
 *
 * A command must start at the beginning of the first line: `/model` is a
 * command, and a slash inside prose is not.
 */
export function commandPrefix(
  lines: string[],
  cursorLine: number,
  cursorCol: number,
): string | undefined {
  if (cursorLine !== 0) {
    return undefined;
  }
  const line = lines[0] ?? "";
  if (!line.startsWith("/")) {
    return undefined;
  }
  const upToCursor = line.slice(0, cursorCol);
  // Once an argument has been typed the command itself is settled.
  if (upToCursor.includes(" ")) {
    return undefined;
  }
  return upToCursor;
}
