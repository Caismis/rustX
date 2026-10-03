/** Product Host desktop contract. Paths are never browser-authored launch inputs. */
export interface DesktopTarget { session_id: string; active_node: string }
export type DesktopAppId = 'files' | 'terminal' | 'code';
export interface DesktopApplication { id: DesktopAppId; label: string }
export type DesktopUnavailable = 'mapping' | 'headless' | 'platform' | 'applications';
export type DesktopCatalog = { available: true; applications: DesktopApplication[] } | { available: false; reason: DesktopUnavailable };
export interface DesktopLaunch { status: 'spawned' }
