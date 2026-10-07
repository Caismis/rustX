import type { DesktopTarget } from './desktop';
export type WorkbenchRequest =
  | { kind: 'files'; path: string }
  | { kind: 'resolve'; path: string }
  | { kind: 'read'; path: string }
  | { kind: 'bytes'; path: string }
  | { kind: 'office'; path: string }
  | { kind: 'applications' }
  | { kind: 'open'; path: string; directory: boolean; application: 'files' | 'code' }
  | { kind: 'terminals' }
  | { kind: 'create'; id: string; shell: string }
  | { kind: 'poll'; id: string; cursor: number }
  | { kind: 'input'; id: string; data: string }
  | { kind: 'resize'; id: string; cols: number; rows: number }
  | { kind: 'close'; id: string };
export interface WorkbenchTerminal { id: string; shell: string; exited: boolean }
export interface WorkbenchResult {
  /** Host-admitted, normalized workspace-relative file reference. */
  path?: string;
  applications?: import('./desktop').DesktopCatalog;
  entries?: { name: string; directory: boolean; link: boolean }[];
  text?: string;
  base64?: string;
  /** Canonical directory resolved by the Host from the admitted native Session. */
  cwd?: string;
  terminals?: WorkbenchTerminal[];
  shells?: string[];
  output?: string;
  cursor?: number;
  reset?: boolean;
  exited?: boolean;
}
export interface WorkbenchCall { target: DesktopTarget; request: WorkbenchRequest }
