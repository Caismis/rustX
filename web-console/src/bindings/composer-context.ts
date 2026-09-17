import type { GoalSnapshot, RuntimeClientSnapshot, TodoTask, UserContentBlock, UserInputBlock } from '../../../protocol/app-server/v6';

/** Current composer-context docks read only the replaceable authoritative
 * snapshot. Transcript, Trace and historical Tool results never enter here. */

/** `todos` is present exactly when the attached runtime composes Todo. An
 * empty list under a composed runtime is a current fact, not an absence. */
export type TodoDockState = { kind: 'absent' } | { kind: 'current'; tasks: readonly TodoTask[] };
export function todoDock(snapshot?: RuntimeClientSnapshot): TodoDockState {
  if (!snapshot?.todos) return { kind: 'absent' };
  // Tombstones stay native dependency targets; they are not current work.
  return { kind: 'current', tasks: (snapshot.todos.tasks ?? []).filter(task => task.status !== 'deleted') };
}

export interface GoalDockState {
  goal: GoalSnapshot;
  /** Process-local activation; it never implies a durable revision change. */
  armed: boolean;
}
/** Extension absent, no Goal and terminal Complete occupy no composer space. */
export function goalDock(snapshot?: RuntimeClientSnapshot): GoalDockState | undefined {
  const view = snapshot?.goal;
  return view?.current && view.current.phase !== 'complete' ? { goal: view.current, armed: view.armed } : undefined;
}

export type InboundRow = NonNullable<RuntimeClientSnapshot['inbound']['pending']>[number];
/** Accepted, not yet adopted native inbound in runtime sequence order. */
export const queueRows = (snapshot?: RuntimeClientSnapshot): readonly InboundRow[] => snapshot?.inbound.pending ?? [];

/** Presentation-only one-line summary of submitted content. Native inbound rows
 * carry canonical `UserContentBlock`s; an accepted submission carries the
 * `UserInputBlock`s this client authored. Both name text the same way. */
export function contentPreview(content: readonly (UserContentBlock | UserInputBlock)[]): string {
  const text = content.flatMap(block => block.type === 'text' ? [block.text] : []).join(' ').replace(/\s+/g, ' ').trim();
  const other = content.length - content.filter(block => block.type === 'text').length;
  return [text, other ? `${other} attachment${other === 1 ? '' : 's'}` : ''].filter(Boolean).join(' · ') || 'Empty content';
}

/** Non-Human inbound provenance label; Human input carries none. */
export function inboundOrigin(message: InboundRow['message']): string | undefined {
  if (message.kind && typeof message.kind === 'object' && 'goal_continuation' in message.kind) return 'Goal continuation';
  const source = message.source;
  if (source === 'human') return undefined;
  if (typeof source === 'string') return source.replace('_', ' ');
  if ('agent' in source) return `Agent ${source.agent.agent_id}`;
  if ('extension' in source) return source.extension.contributor;
  return undefined;
}
