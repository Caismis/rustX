import type { TranslationKey } from '../../locale/translation';
/** Browser grammar only. Identity is independent of translated labels/aliases. */
export type CommandId = 'model' | 'compact' | 'new' | 'fork' | 'branch' | 'goal' | 'tools';
export interface CommandDefinition {
  id: CommandId;
  labelKey: TranslationKey;
  aliases: readonly string[];
  availability: 'attached' | 'idle' | 'no-attempt' | 'goal';
}
export const commands: readonly CommandDefinition[] = [
  { id: 'model', labelKey: 'commands:command.model', aliases: [/* i18n-raw: command search vocabulary, identical in both locales */ 'Choose model', '模型'], availability: 'attached' },
  { id: 'compact', labelKey: 'commands:command.compact', aliases: [/* i18n-raw: command search vocabulary, identical in both locales */ 'Compact context', '压缩'], availability: 'no-attempt' },
  { id: 'new', labelKey: 'commands:command.new', aliases: [/* i18n-raw: command search vocabulary, identical in both locales */ 'New Conversation', '新建'], availability: 'attached' },
  { id: 'fork', labelKey: 'commands:command.fork', aliases: [/* i18n-raw: command search vocabulary, identical in both locales */ 'Fork independent Session', '分叉'], availability: 'attached' },
  { id: 'branch', labelKey: 'commands:command.branch', aliases: [/* i18n-raw: command search vocabulary, identical in both locales */ 'Branch within Session', '分支'], availability: 'idle' },
  { id: 'goal', labelKey: 'commands:command.goal', aliases: [/* i18n-raw: command search vocabulary, identical in both locales */ 'Goal controls', '目标'], availability: 'goal' },
  { id: 'tools', labelKey: 'commands:command.tools', aliases: [/* i18n-raw: command search vocabulary, identical in both locales */ 'Inspect capabilities', '工具'], availability: 'attached' },
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
