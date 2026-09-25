/** Browser grammar only. Identity is independent of translated labels/aliases. */
export type CommandId = 'model' | 'compact' | 'new' | 'fork' | 'branch' | 'goal' | 'tools';
export interface CommandDefinition {
  id: CommandId;
  label: string;
  aliases: readonly string[];
  availability: 'attached' | 'idle' | 'no-attempt' | 'goal';
}
export const commands: readonly CommandDefinition[] = [
  { id: 'model', label: 'Choose model', aliases: ['模型'], availability: 'attached' },
  { id: 'compact', label: 'Compact context', aliases: ['压缩'], availability: 'no-attempt' },
  { id: 'new', label: 'New Conversation', aliases: ['新建'], availability: 'attached' },
  { id: 'fork', label: 'Fork independent Session', aliases: ['分叉'], availability: 'attached' },
  { id: 'branch', label: 'Branch within Session', aliases: ['分支'], availability: 'idle' },
  { id: 'goal', label: 'Goal controls', aliases: ['目标'], availability: 'goal' },
  { id: 'tools', label: 'Inspect capabilities', aliases: ['工具'], availability: 'attached' },
];
export function available(command: CommandDefinition, running: boolean, goal: boolean, lineageSwitchSafe: boolean) {
  switch (command.availability) {
    case 'goal': return goal;
    case 'idle': return lineageSwitchSafe;
    case 'no-attempt': return !running;
    case 'attached': return true;
  }
}
export type ParsedCommand = { type: 'text' } | { type: 'command'; id: CommandId } | { type: 'unsupported' };
/** Deliberately only a leading slash token; no arguments, shell, or interpolation.
 * URLs and inline slashes are ordinary text. All leading slash input is reserved. */
export function parseCommand(text: string): ParsedCommand {
  const input = text.trim();
  if (!input.startsWith('/')) return { type: 'text' };
  const token = input.slice(1).toLowerCase();
  const command = commands.find(item => item.id === token) ?? commands.find(item => item.aliases.includes(token));
  return command ? { type: 'command', id: command.id } : { type: 'unsupported' };
}
export function discoveryQuery(text: string): string | undefined {
  return /^\s*\/[^\s]*$/.test(text) ? text.trim().slice(1) : undefined;
}
