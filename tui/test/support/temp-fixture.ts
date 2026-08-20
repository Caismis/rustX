/**
 * One owned temporary filesystem root for a TUI integration fixture.
 *
 * RAII ownership (Issue #86 repair): `create` allocates a fresh
 * `rustx-tui-*` root and returns the only object that knows its path. The
 * owning suite removes it by calling `cleanup()` from its `after` hook,
 * which `node:test` runs on pass AND failure — so no test body ever creates
 * a raw path or remembers to remove a directory, and a failed or aborted
 * suite still releases its root. The fixture is deliberately the single
 * owner: the raw `mkdtemp` result never escapes this module.
 */
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export class TempFixture {
  readonly root: string;

  private constructor(root: string) {
    this.root = root;
  }

  /** Allocates a fresh owned temporary root. */
  static create(prefix: string): TempFixture {
    return new TempFixture(mkdtempSync(join(tmpdir(), prefix)));
  }

  /** Joins one path under the owned root. */
  path(...segments: string[]): string {
    return join(this.root, ...segments);
  }

  /**
   * Recursively removes the owned root. Idempotent and best-effort: the
   * caller invokes it exactly once from the owning suite's teardown, and a
   * removal failure surfaces through the test runner rather than being
   * silently retried.
   */
  cleanup(): void {
    rmSync(this.root, { recursive: true, force: true });
  }
}
