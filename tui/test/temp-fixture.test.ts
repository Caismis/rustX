/**
 * The `TempFixture` ownership regression (Issue #86 repair).
 *
 * Proves rustX's fixture — not Node's `mkdtemp` — owns the temporary root:
 * the directory exists while the fixture is alive, backs representative TUI
 * integration usage, and is removed by teardown; and teardown still removes
 * it when the usage step fails (a child `node --test` run whose test fails
 * by design).
 */

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";

import { TempFixture } from "./support/temp-fixture.ts";

describe("TempFixture owns its temporary root", () => {
  let fixture: TempFixture | undefined;
  let root = "";

  before(() => {
    fixture = TempFixture.create("rustx-tui-");
    root = fixture.root;
    assert.ok(existsSync(root), "the root exists while the fixture is alive");
  });

  it("backs representative TUI integration usage under the root", () => {
    const current = fixture;
    assert.ok(current);
    // Mirrors the real integration setup: models.json, rustx.json, and a
    // workspace directory all live under the single owned root.
    writeFileSync(current.path("models.json"), "{}");
    writeFileSync(current.path("rustx.json"), "{}");
    mkdirSync(current.path("workspace"), { recursive: true });
    assert.ok(existsSync(current.path("models.json")));
    assert.ok(existsSync(current.path("rustx.json")));
    assert.ok(existsSync(current.path("workspace")));
  });

  after(() => {
    fixture?.cleanup();
    assert.ok(
      !existsSync(root),
      "teardown removed the owned root after the tests completed",
    );
  });
});

describe("TempFixture cleanup on failure paths", () => {
  it("removes the owned root when the usage step fails", (t) => {
    // The scenario runs in a child `node --test` process: its test fails by
    // design, and its after hook must still remove the root it owns. The
    // outer fixture owns the scenario file's own temporary area.
    const area = TempFixture.create("rustx-tui-scenario-");
    t.after(() => area.cleanup());
    const scenario = join(area.root, "failing-suite.mjs");
    const fixtureModule = fileURLToPath(
      new URL("./support/temp-fixture.ts", import.meta.url),
    );
    writeFileSync(
      scenario,
      [
        `import { after, before, describe, it } from "node:test";`,
        `import { existsSync } from "node:fs";`,
        `import { TempFixture } from ${JSON.stringify(fixtureModule)};`,
        `describe("failing scenario", () => {`,
        `  const fixture = TempFixture.create("rustx-tui-fail-");`,
        `  before(() => {`,
        `    if (!existsSync(fixture.root)) throw new Error("root missing");`,
        `  });`,
        `  it("fails by design", () => { throw new Error("scenario failure"); });`,
        `  after(() => {`,
        `    fixture.cleanup();`,
        `    console.log("CLEANUP-RAN " + fixture.root);`,
        `  });`,
        `});`,
        ``,
      ].join("\n"),
    );
    // The outer `node --test` run marks its children with NODE_TEST_CONTEXT;
    // the scenario must run as a REAL standalone suite, so the marker is
    // stripped from the child environment.
    const env = { ...process.env };
    delete env.NODE_TEST_CONTEXT;
    const run = spawnSync(process.execPath, ["--test", scenario], {
      encoding: "utf8",
      env,
    });
    assert.equal(run.status, 1, "the scenario test fails by design");
    const marker = /CLEANUP-RAN (\S+)/.exec(run.stdout);
    assert.ok(marker, "the scenario's after hook ran despite the failure");
    const removedRoot = marker[1];
    assert.ok(removedRoot, "the marker carries the removed root path");
    assert.ok(
      !existsSync(removedRoot),
      "the failing scenario removed the root it owned",
    );
  });
});
