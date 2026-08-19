/**
 * Presentation state -> Markdown text.
 *
 * These are pure functions over the projection, which is what makes the
 * rendering layer replaceable: they produce strings, and Pi (or any other
 * terminal library) turns strings into cells. Nothing here holds state, and
 * nothing here reaches the runtime.
 *
 * Tool rendering is generic. Origin and name pick a label and nothing else —
 * there is no `if (tool.name === "bash")` branch anywhere, because execution
 * semantics belong to Rust.
 */

import type { DefaultTextStyle } from "@earendil-works/pi-tui";

import {
  activeBackground,
  isBackgroundTerminal,
  originLabel,
  outcomeLabel,
} from "../presentation/selectors.ts";
import { style } from "./theme.ts";
import type {
  PresentationState,
  StreamingMessage,
  TranscriptCommitted,
  TranscriptEntry,
} from "../presentation/state.ts";
import type {
  ForegroundToolExecution,
  InteractionRequest,
  RuntimeClientBackgroundExecution,
  ToolExecutionResult,
  ToolProgress,
} from "../protocol/types.ts";

/** The Markdown body of one transcript entry. */
export function renderEntry(entry: TranscriptEntry): string {
  return renderEntryBlocks(entry)
    .map((block) => block.markdown)
    .join("\n\n");
}

/**
 * One semantic transcript block rendered by its own Markdown component.
 *
 * Applying reasoning style through Markdown's default text style makes the
 * renderer reapply it after nested Markdown spans reset their ANSI styling.
 */
export interface RenderedEntryBlock {
  markdown: string;
  defaultTextStyle?: DefaultTextStyle;
}

/** The independently styled Markdown blocks of one transcript entry. */
export function renderEntryBlocks(entry: TranscriptEntry): RenderedEntryBlock[] {
  return entry.kind === "streaming"
    ? renderStreaming(entry)
    : renderCommitted(entry);
}

function renderCommitted(entry: TranscriptCommitted): RenderedEntryBlock[] {
  const message = entry.message;
  switch (message.role) {
    case "user": {
      // Provenance is metadata, never a different role. A runtime-originated
      // inbound message is labelled as such rather than shown as a human turn.
      const label =
        message.source === "human"
          ? "you"
          : typeof message.source === "object" && "agent" in message.source
            ? `agent ${message.source.agent.agent_id}`
            : String(message.source);
      const body = message.content
        .map((block) =>
          block.type === "text" ? block.text : `_(${block.type})_`,
        )
        .join("\n");
      const kind =
        message.kind === "compaction_summary" ? " · compaction summary" : "";
      return [{ markdown: `${style.cyan(`▌ ${label}${kind}`)}\n${body}` }];
    }

    case "assistant":
      return message.content
        .map((block) => {
          switch (block.type) {
            case "text":
              return {
                markdown: `${style.grey("▌ answer")}\n${block.text}`,
              };
            case "reasoning":
              // Reasoning stays reasoning; it is never presented as an answer.
              return block.text === undefined
                ? {
                    markdown: "▌ reasoning (not exposed by the provider)",
                    defaultTextStyle: { color: style.grey },
                  }
                : {
                    markdown: `▌ reasoning\n${block.text}`,
                    defaultTextStyle: { color: style.grey },
                  };
            case "refusal":
              // Refusal stays refusal.
              return {
                markdown: `${style.yellow("▌ refusal")}\n${block.text}`,
              };
            case "tool_call":
              return {
                markdown: `${style.magenta(`▌ tool call ${block.name}`)}\n${fence(
                  stringifyArguments(block.arguments),
                )}`,
              };
            case "image":
              return { markdown: style.grey("▌ image") };
            default:
              return undefined;
          }
        })
        .filter((block) => block !== undefined);

    case "tool":
      return [
        {
          markdown: `${style.magenta(`▌ tool result ${message.tool_id}`)}\n${renderResult(message.result)}`,
        },
      ];

    case "system":
      return [
        {
          markdown: `${style.grey(`▌ system (${message.authority})`)}\n${style.dim(
            message.content.map((block) => block.text).join("\n"),
          )}`,
        },
      ];

    default:
      return [];
  }
}

function renderStreaming(entry: StreamingMessage): RenderedEntryBlock[] {
  return entry.blocks
    .map((block) => {
      switch (block.kind) {
        case "text":
          return {
            markdown: `${style.grey("▌ answer")}\n${block.text}`,
          };
        case "reasoning":
          return {
            markdown: `▌ reasoning\n${block.text}`,
            defaultTextStyle: { color: style.grey },
          };
        case "refusal":
          return {
            markdown: `${style.yellow("▌ refusal")}\n${block.text}`,
          };
        case "tool_call":
          return {
            markdown: `${style.magenta(`▌ tool call ${block.name}`)}\n${fence(
              block.argumentsText,
            )}`,
          };
        default:
          return undefined;
      }
    })
    .filter((block) => block !== undefined);
}

/**
 * One foreground tool card.
 *
 * Purely lifecycle-driven: assembled -> running -> settled, with the runtime's
 * own progress and normalized result. Nothing branches on which tool this is.
 */
export function renderForegroundTool(
  execution: ForegroundToolExecution,
): string {
  const header = `${style.magenta("▌ tool")} ${style.bold(execution.name || execution.tool_id)}`;
  switch (execution.state.type) {
    case "assembled":
      return `${header} ${style.dim("assembled")}\n${fence(execution.state.arguments)}`;
    case "running":
      return `${header} ${style.yellow("running")}${renderProgress(execution.state.progress)}\n${fence(
        execution.state.arguments,
      )}`;
    case "settled":
      return `${header} ${statusLabel(execution.state.result)}\n${renderResult(
        execution.state.result,
      )}`;
    default:
      return header;
  }
}

/**
 * One background execution card.
 *
 * A background unit is alive because the runtime says so. Removing this card
 * cancels nothing, and a hidden card is never evidence of settlement.
 */
export function renderBackground(
  execution: RuntimeClientBackgroundExecution,
): string {
  const marker = isBackgroundTerminal(execution.state)
    ? style.grey("●")
    : style.yellow("◐");
  const head = `${marker} ${style.bold(execution.tool_name)} ${style.dim(
    `${execution.execution_id} · ${execution.state}`,
  )}`;
  const progress = renderProgress(execution.progress);
  const result =
    execution.result === undefined ? "" : `\n${renderResult(execution.result)}`;
  return `${head}${progress}${result}`;
}

/** The background section, or an empty string when nothing is known. */
export function renderBackgroundSection(state: PresentationState): string {
  if (state.background.length === 0) {
    return "";
  }
  const active = activeBackground(state).length;
  return [
    style.bold(
      `Background — ${active} active of ${state.background.length} known`,
    ),
    ...state.background.map((execution) => renderBackground(execution)),
  ].join("\n");
}

/** The live runtime-owned approval cards, rendered without local outcome state. */
export function renderInteractionSection(state: PresentationState): string {
  if (state.pendingInteractions.length === 0) {
    return "";
  }
  return [
    style.bold(`Approval required — ${state.pendingInteractions.length} pending`),
    ...state.pendingInteractions.map(renderInteraction),
    style.dim("Answer with /approve <interaction-id> <allow|deny> [reason]."),
  ].join("\n");
}

function renderInteraction(interaction: InteractionRequest): string {
  if (interaction.kind.type !== "approval") {
    return `${style.yellow("? interaction")} ${interaction.id}`;
  }
  return [
    `${style.yellow("? approval")} ${style.bold(interaction.kind.tool_name)} ${style.dim(interaction.id)}`,
    `${style.dim("call")} ${interaction.kind.call_id} · ${interaction.kind.mode} · ${originLabel(interaction.kind.origin)}`,
    `${style.dim(interaction.kind.reason)}`,
    fence(JSON.stringify(interaction.kind.arguments, null, 2), "json"),
  ].join("\n");
}

/** The one-line footer, written entirely from rustX presentation data. */
export function renderFooter(
  state: PresentationState,
  connectionState: string,
): string {
  const parts: string[] = [];
  parts.push(style.cyan(state.sessionModel.configured.model));

  const attempt = state.attempt;
  if (attempt !== undefined) {
    // When the attempt is on a different model than the session's desired
    // one, both are shown. The footer never collapses them.
    if (attempt.model.primary.model !== state.sessionModel.effective.model) {
      parts.push(style.yellow(`attempt: ${attempt.model.primary.model}`));
    }
    parts.push(
      attempt.phase.type === "settled"
        ? style.dim(outcomeLabel(attempt.phase.outcome))
        : style.yellow(`${attempt.phase.type} · turn ${attempt.turn}`),
    );
    if (attempt.lastUsage !== undefined) {
      parts.push(
        style.dim(
          `${attempt.lastUsage.input_tokens}in/${attempt.lastUsage.output_tokens}out`,
        ),
      );
    }
  }

  parts.push(style.dim(`cap r${state.capabilities.revision}`));

  const pending = (state.inbound.pending ?? []).length;
  if (pending > 0) {
    parts.push(style.yellow(`inbox ${pending}`));
  }
  const background = activeBackground(state).length;
  if (background > 0) {
    parts.push(style.magenta(`bg ${background}`));
  }
  if (state.runtimeShutdown) {
    parts.push(style.red("shutdown"));
  }
  parts.push(style.grey(connectionState));

  return parts.join(style.grey(" · "));
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function renderProgress(progress: ToolProgress | undefined): string {
  if (progress === undefined) {
    return "";
  }
  const pieces: string[] = [];
  if (progress.message !== undefined) {
    pieces.push(progress.message);
  }
  if (progress.completed !== undefined) {
    pieces.push(
      progress.total === undefined
        ? `${progress.completed}`
        : `${progress.completed}/${progress.total}`,
    );
  }
  return pieces.length === 0 ? "" : ` ${style.dim(pieces.join(" · "))}`;
}

function statusLabel(result: ToolExecutionResult): string {
  switch (result.status.type) {
    case "success":
      return style.green("ok");
    case "failed":
      return style.red("failed");
    case "denied":
      return style.yellow(`denied (${result.status.reason})`);
    case "cancelled":
      return style.yellow(`cancelled (${result.status.reason})`);
    case "timed_out":
      return style.yellow("timed out");
    case "interrupted":
      // Interrupted is genuinely "outcome unknown", never a quiet success.
      return style.yellow("interrupted (outcome unknown)");
    default:
      return style.dim("settled");
  }
}

function renderResult(result: ToolExecutionResult): string {
  const lines: string[] = [];
  if (result.status.type === "failed") {
    lines.push(style.red(result.status.error));
  }
  if (result.status.type === "denied") {
    lines.push(style.yellow(result.status.reason));
  }
  for (const content of result.content ?? []) {
    switch (content.type) {
      case "text":
        lines.push(content.text);
        break;
      case "json":
        lines.push(fence(JSON.stringify(content.value, null, 2), "json"));
        break;
      case "file":
        lines.push(style.dim("(file)"));
        break;
      case "image":
        lines.push(style.dim("(image)"));
        break;
      default:
        break;
    }
  }
  if (result.truncation?.truncated === true) {
    lines.push(
      style.dim(
        `… truncated${
          result.truncation.original_bytes === undefined
            ? ""
            : ` from ${result.truncation.original_bytes} bytes`
        }`,
      ),
    );
  }
  if (result.exit_code !== undefined) {
    lines.push(style.dim(`exit ${result.exit_code}`));
  }
  return lines.join("\n");
}

/** Arguments are carried as opaque JSON and only pretty-printed. */
function stringifyArguments(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2) ?? String(value);
  } catch {
    return String(value);
  }
}

function fence(body: string, language = ""): string {
  return `\`\`\`${language}\n${body}\n\`\`\``;
}

export { originLabel };
