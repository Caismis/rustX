import type { AppServerSession } from "../app-server/session.ts";

/**
 * A lease for one asynchronous client-side presentation continuation.
 *
 * The epoch is advanced whenever authoritative runtime ownership changes. The
 * attachment identity is checked as well, so a late continuation cannot
 * repaint a new attachment that happens to have the same projection shape.
 */
export interface PresentationLease {
  epoch: number;
  sessionListGeneration: number;
  session: AppServerSession | undefined;
}
