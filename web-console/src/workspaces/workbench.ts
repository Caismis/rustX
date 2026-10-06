import type { DesktopTarget } from './desktop';
export type WorkbenchRequest =
  | { kind: 'files'; path: string }
  | { kind: 'read'; path: string }
  | { kind: 'terminals' }
  | { kind: 'create'; id: string; shell: string }
  | { kind: 'poll'; id: string; cursor: number }
  | { kind: 'input'; id: string; data: string }
  | { kind: 'resize'; id: string; cols: number; rows: number }
  | { kind: 'close'; id: string };
export interface WorkbenchTerminal { id: string; shell: string; exited: boolean }
export interface WorkbenchResult {
  entries?: { name: string; directory: boolean; link: boolean }[];
  text?: string;
  terminals?: WorkbenchTerminal[];
  shells?: string[];
  output?: string;
  cursor?: number;
  reset?: boolean;
  exited?: boolean;
}
export interface WorkbenchCall { target: DesktopTarget; request: WorkbenchRequest }
