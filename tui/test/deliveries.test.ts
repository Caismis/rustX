/**
 * `present` in the TUI consumes only committed typed deliveries.
 *
 * The contracts: a delivery is drawn or listed only from the `deliveries`
 * field of a successfully committed canonical Tool-result message; arguments,
 * tool output text, generic JSON and foreground settlements forge nothing;
 * client-local actions are bounded, explicit and never interpret a remote
 * server path locally.
 */

import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { open } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";

import {
  DELIVERY_MAX_BASE64,
  DELIVERY_MAX_BYTES,
  DeliveryResidueError,
  SAVE_CHUNK_BYTES,
  decodeDelivery,
  deliveryDestination,
  openDelivery,
  saveDelivery,
  type SaveFiles,
} from "../src/app-server/delivery-files.ts";
import { deliveryType, pageDeliveries } from "../src/presentation/deliveries.ts";
import { correlateTools } from "../src/presentation/tools.ts";
import type { RuntimeClientTranscriptPage, SessionFileReference } from "../src/protocol/app-server.ts";
import { DeliverySelector } from "../src/ui/components/delivery-selector.ts";
import { rendererFor } from "../src/ui/components/tool-renderers.ts";
import { renderTranscript } from "../src/ui/components/transcript.ts";
import {
  assistantBlocks,
  attemptView,
  foreground,
  toolCallBlock,
  toolMessage,
  toolResult,
} from "./support/fixtures.ts";
import { blockText, prefs, stateOf } from "./support/render.ts";

function file(name: string, description?: string): SessionFileReference {
  return {
    scope: { conversation_id: "conv_original", device: "1", inode: "2" },
    path: `out/${name}`,
    name,
    description: description ?? null,
    mime_type: "text/markdown",
  };
}
const REPORT = file("报告 final.md", "Final report");
/** Names that must never reach the terminal as typed. */
const HOSTILE_NAMES = [
  "\u001b[2J\u001b[31mcleared.md", // ESC + CSI
  "\u001b]0;retitled\u0007x.md", // OSC
  "\u009b31mc1.md", // C1 CSI
  "line\nbreak.md",
  "carriage\rreturn.md",
  "invoice\u202etxt.exe", // bidi override
  "isolate\u2066x\u2069.md", // bidi isolate
  "mark\u061c.md", // Arabic letter mark
];
/** Every rendered character outside this client's own SGR and cursor marks. */
function foreignControls(lines: string[]): string[] {
  const own = lines.join("\n")
    .replace(/\u001b\[[0-9;]*m/g, "")
    .replace(/\u001b_pi:c\u0007/g, "");
  return [...own.matchAll(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/g)]
    .map((match) => JSON.stringify(match[0]));
}
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((settle) => { resolve = settle; });
  return { promise, resolve };
}
/** Real files whose second chunk write parks until released. */
function parkedSecondWrite(): { files: SaveFiles; parked: Promise<void>; release: () => void } {
  const parked = deferred<void>();
  const proceed = deferred<void>();
  let writes = 0;
  return {
    parked: parked.promise,
    release: () => proceed.resolve(),
    files: {
      open: async (path, flags, mode) => {
        const handle = await open(path, flags, mode);
        const write = handle.write.bind(handle) as (buffer: Uint8Array, offset: number, length: number) => Promise<{ bytesWritten: number }>;
        Object.assign(handle, {
          write: async (buffer: Uint8Array, offset: number, length: number) => {
            writes += 1;
            if (writes === 2) { parked.resolve(); await proceed.promise; }
            return write(buffer, offset, length);
          },
        });
        return handle;
      },
    },
  };
}
const DATA = file("data set.csv");
const ARGS = { files: [{ path: "out/报告 final.md" }, { path: "out/data set.csv" }] };

function page(entries: RuntimeClientTranscriptPage["entries"], next?: string): RuntimeClientTranscriptPage {
  return { entries, next_cursor: next ?? null };
}
function entry(cursor: string, message: ReturnType<typeof toolMessage>) {
  return { cursor, item: { type: "message" as const, message } };
}

describe("committed delivery records", () => {
  it("come only from successful committed Tool results, newest result first, in canonical order", () => {
    const forged = toolResult({
      status: { type: "success" },
      content: [{ type: "json", value: { deliveries: [REPORT] } }, { type: "text", text: "Declared report.md" }],
    });
    const records = pageDeliveries(page([
      entry("1", toolMessage("m-old", "c-old", "tool-present", toolResult({ deliveries: [DATA] }))),
      entry("2", toolMessage("m-failed", "c-failed", "tool-present", toolResult({ status: { type: "failed", error: "x" }, deliveries: [REPORT] }))),
      entry("3", toolMessage("m-cancelled", "c-cancelled", "tool-present", toolResult({ status: { type: "cancelled", reason: "stop" } as never, deliveries: [REPORT] }))),
      entry("4", toolMessage("m-forged", "c-forged", "tool-write", forged)),
      { cursor: "5", item: { type: "message" as const, message: assistantBlocks("m-call", [toolCallBlock("c-new", "tool-present", "present", ARGS)]) } },
      entry("6", toolMessage("m-new", "c-new", "tool-present", toolResult({ deliveries: [REPORT, DATA] }))),
    ], "older"));
    assert.deepEqual(records.records.map((record) => [record.messageId, record.index, record.count, record.file.name]), [
      ["m-new", 0, 2, REPORT.name],
      ["m-new", 1, 2, DATA.name],
      ["m-old", 0, 1, DATA.name],
    ]);
    assert.equal(records.next, "older");
    assert.equal(deliveryType(REPORT), "MD");
  });
});

describe("tool-present presentation", () => {
  const renderer = rendererFor("tool-present");

  it("draws declared paths as the call, and deliveries only from committed facts", () => {
    assert.equal(renderer.renderCall(ARGS)?.title, "Present");
    assert.match(renderer.renderCall(ARGS)?.subject ?? "", /data set\.csv/);
    for (const malformed of [undefined, {}, { files: "x" }, { files: [{}] }, { files: [] }]) {
      assert.equal(renderer.renderCall(malformed), undefined, "malformed shapes fall back to the generic card");
    }
    assert.equal(renderer.renderResult?.({ content: [{ type: "text", text: "Declared 2 deliverable(s)" }], deliveries: [] }, ARGS), undefined);
    const committed = renderer.renderResult?.({ content: [], deliveries: [REPORT, DATA] }, ARGS);
    assert.match(committed?.summary?.[0] ?? "", /Delivered 2 files/);
    assert.ok((committed?.detail ?? []).join("\n").indexOf(REPORT.name) < (committed?.detail ?? []).join("\n").indexOf(DATA.name));
    assert.notEqual(rendererFor("tool-unknown-present"), renderer, "unknown tools stay generic");
    assert.notEqual(rendererFor("tool-unknown-present").renderCall(ARGS)?.title, "Present");
  });

  it("never draws a pre-commit foreground settlement as a delivery", () => {
    const settled = toolResult({ deliveries: [REPORT] });
    const running = stateOf({
      messages: [assistantBlocks("m1", [toolCallBlock("call-1", "tool-present", "present", ARGS)])],
      attempt: attemptView({ foreground: [foreground("call-1", "tool-present", "present", { type: "settled", arguments: JSON.stringify(ARGS), result: settled })] }),
    });
    const tool = correlateTools(running).byCallId.get("call-1");
    assert.equal(tool?.lifecycle.type, "settled");
    assert.equal(tool?.resultCommitted, false);
    const before = renderTranscript(running, prefs()).map(blockText).join("\n");
    assert.doesNotMatch(before, /Delivered/);

    const committed = stateOf({
      messages: [
        assistantBlocks("m1", [toolCallBlock("call-1", "tool-present", "present", ARGS)]),
        toolMessage("m2", "call-1", "tool-present", settled),
      ],
      attempt: attemptView({ foreground: [foreground("call-1", "tool-present", "present", { type: "settled", arguments: JSON.stringify(ARGS), result: settled })] }),
    });
    assert.equal(correlateTools(committed).byCallId.get("call-1")?.resultCommitted, true);
    assert.match(renderTranscript(committed, prefs()).map(blockText).join("\n"), /Delivered 1 file/);

    const failed = stateOf({
      messages: [
        assistantBlocks("m1", [toolCallBlock("call-1", "tool-present", "present", ARGS)]),
        toolMessage("m2", "call-1", "tool-present", toolResult({ status: { type: "failed", error: "missing" }, deliveries: [REPORT] })),
      ],
    });
    assert.doesNotMatch(renderTranscript(failed, prefs()).map(blockText).join("\n"), /Delivered/);
  });
});

describe("client-local delivery actions", () => {
  it("bounds base64 before decoding and rejects malformed bodies", () => {
    assert.equal(decodeDelivery(Buffer.alloc(DELIVERY_MAX_BYTES, 1).toString("base64")).length, DELIVERY_MAX_BYTES);
    assert.equal(Buffer.alloc(DELIVERY_MAX_BYTES).toString("base64").length, DELIVERY_MAX_BASE64);
    for (const body of [Buffer.alloc(DELIVERY_MAX_BYTES + 1).toString("base64"), "a", "@@@@", "QQ==QQ=="]) {
      assert.throws(() => decodeDelivery(body));
    }
  });

  it("resolves only explicit, renderable client-local destinations", () => {
    assert.equal(deliveryDestination("报告 final.md", "/client/cwd", "/home/u"), "/client/cwd/报告 final.md");
    assert.equal(deliveryDestination("~/out/y", "/client/cwd", "/home/u"), "/home/u/out/y");
    for (const unsafe of ["", "  ", ...HOSTILE_NAMES]) {
      assert.throws(() => deliveryDestination(unsafe, "/client/cwd"), /Invalid save destination/, JSON.stringify(unsafe));
    }
  });

  it("saves byte-exactly, never truncates, and leaves nothing on cancellation", async () => {
    const dir = mkdtempSync(join(tmpdir(), "rustx-delivery-save-"));
    try {
      const bytes = Buffer.from("line\r\n\u0000报告", "utf8");
      const read = async () => ({ data: bytes.toString("base64") });
      const target = join(dir, "报告 final.md");
      await saveDelivery(read, target);
      assert.deepEqual(readFileSync(target), bytes);
      writeFileSync(target, "kept");
      await assert.rejects(saveDelivery(read, target), /EEXIST/);
      assert.equal(readFileSync(target, "utf8"), "kept");
      const abort = new AbortController();
      abort.abort();
      await assert.rejects(saveDelivery(read, join(dir, "cancelled"), abort.signal));
      assert.equal(existsSync(join(dir, "cancelled")), false);
      await assert.rejects(saveDelivery(async () => ({ data: "!" }), join(dir, "malformed")));
      assert.equal(existsSync(join(dir, "malformed")), false);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("cancellation before the local commit leaves nothing; after it the save stands", async () => {
    const dir = mkdtempSync(join(tmpdir(), "rustx-delivery-commit-"));
    try {
      const bytes = Buffer.alloc(SAVE_CHUNK_BYTES * 3, 7);
      const body = { data: bytes.toString("base64") };

      // Before local file creation: the read answered, cancellation won.
      const beforeCreate = new AbortController();
      const answered = deferred<{ data: string }>();
      const early = saveDelivery(() => answered.promise, join(dir, "early"), beforeCreate.signal);
      beforeCreate.abort();
      answered.resolve(body);
      await assert.rejects(early, (error) => error === beforeCreate.signal.reason);
      assert.equal(existsSync(join(dir, "early")), false, "never created");

      // During an incomplete write: the owned partial file is removed.
      const during = new AbortController();
      const writes = parkedSecondWrite();
      const partial = saveDelivery(async () => body, join(dir, "partial"), during.signal, writes.files);
      await writes.parked;
      assert.equal(existsSync(join(dir, "partial")), true, "partial output exists mid-write");
      during.abort();
      writes.release();
      await assert.rejects(partial, (error) => error === during.signal.reason);
      assert.equal(existsSync(join(dir, "partial")), false, "the owned partial file is removed");

      // A file that replaced the destination mid-write is not this
      // operation's to remove.
      const replacedAbort = new AbortController();
      const replacedWrites = parkedSecondWrite();
      const replaced = saveDelivery(async () => body, join(dir, "replaced"), replacedAbort.signal, replacedWrites.files);
      await replacedWrites.parked;
      writeFileSync(join(dir, "someone else"), "theirs");
      renameSync(join(dir, "someone else"), join(dir, "replaced"));
      replacedAbort.abort();
      replacedWrites.release();
      await assert.rejects(replaced, (error) => error === replacedAbort.signal.reason);
      assert.equal(readFileSync(join(dir, "replaced"), "utf8"), "theirs");

      // After the commit: resolving means saved; a later cancel changes nothing.
      const after = new AbortController();
      assert.equal(await saveDelivery(async () => body, join(dir, "kept"), after.signal), join(dir, "kept"));
      after.abort();
      assert.deepEqual(readFileSync(join(dir, "kept")), bytes);

      // Residue that cannot be removed is reported, never hidden.
      if (process.getuid?.() !== 0) {
        const locked = join(dir, "locked");
        const residueAbort = new AbortController();
        const residueWrites = parkedSecondWrite();
        const { mkdirSync } = await import("node:fs");
        mkdirSync(locked);
        const residue = saveDelivery(async () => body, join(locked, "out"), residueAbort.signal, residueWrites.files);
        await residueWrites.parked;
        chmodSync(locked, 0o500);
        residueAbort.abort();
        residueWrites.release();
        await assert.rejects(residue, DeliveryResidueError);
        chmodSync(locked, 0o700);
        assert.equal(existsSync(join(locked, "out")), true);
      }
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("an Open cancelled before launch launches nothing; after launch it is not undone", async () => {
    const dir = mkdtempSync(join(tmpdir(), "rustx-delivery-open-"));
    try {
      const path = join(dir, "报告 final.md");
      writeFileSync(path, "x");
      const { lstatSync } = await import("node:fs");
      const leaf = lstatSync(path, { bigint: true });
      const location = { file: REPORT, path, device: leaf.dev.toString(), inode: leaf.ino.toString() };
      const launched: string[] = [];
      const before = new AbortController();
      const located = deferred<typeof location>();
      const early = openDelivery(() => located.promise, {
        sharedHost: true, opener: "xdg-open", signal: before.signal,
        launch: async (_command, target) => { launched.push(target); return 0; },
      });
      before.abort();
      located.resolve(location);
      await assert.rejects(early, (error) => error === before.signal.reason);
      assert.deepEqual(launched, [], "cancellation before launch acceptance launches nothing");

      const after = new AbortController();
      const accepting: string[] = [];
      const exited = deferred<number>();
      const spawned = deferred<void>();
      const accepted = openDelivery(async () => location, {
        sharedHost: true, opener: "xdg-open", signal: after.signal,
        launch: (_command, target) => { accepting.push(target); spawned.resolve(); return exited.promise; },
      });
      await spawned.promise;
      after.abort();
      exited.resolve(0);
      assert.deepEqual(await accepted, { path, opener: "xdg-open" }, "an accepted launch is reported as such");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("never interprets a server path without a shared host and a matching leaf", async () => {
    let located = false;
    await assert.rejects(openDelivery(async () => { located = true; throw new Error("unreachable"); }, {
      sharedHost: false, opener: "xdg-open", launch: async () => assert.fail("never launched"),
    }), /not on this machine/);
    assert.equal(located, false, "a remote client requests no location");
    await assert.rejects(openDelivery(async () => ({ file: REPORT, path: "/nonexistent/报告 final.md", device: "1", inode: "2" }), {
      sharedHost: true, opener: "xdg-open", launch: async () => assert.fail("never launched"),
    }), /not the delivered file/);
    await assert.rejects(openDelivery(async () => assert.fail("no opener, no request"), { sharedHost: true }), /no system opener/);
  });
});

describe("/files selector", () => {
  const records = pageDeliveries(page([
    entry("1", toolMessage("m1", "c1", "tool-present", toolResult({ deliveries: [REPORT, DATA] }))),
  ], "older"));

  it("refuses unavailable actions explicitly and starts none", () => {
    const selector = new DeliverySelector(records, { save: "no delivery access", open: "files are not on this machine" });
    const started: unknown[] = [];
    selector.onAction = (...args) => started.push(args);
    selector.handleInput("s");
    assert.match(selector.render(120).join("\n"), /Save unavailable: no delivery access/);
    selector.handleInput("o");
    assert.match(selector.render(120).join("\n"), /Open unavailable: files are not on this machine/);
    assert.deepEqual(started, []);
    assert.match(selector.render(120).join("\n"), /out\/data set\.csv/);
    assert.match(selector.render(120).join("\n"), /Load older history/);
  });

  it("dispatches explicit intents and drops outcomes it no longer owns", () => {
    const selector = new DeliverySelector(records, {});
    const started: Array<[number, string, string, string | undefined]> = [];
    const aborted: number[] = [];
    selector.onAction = (operation, action, record, destination) => started.push([operation, action, record.file.name, destination]);
    selector.onAbort = (operation) => aborted.push(operation);
    selector.handleInput("o");
    assert.deepEqual(started, [[1, "open", REPORT.name, undefined]]);
    selector.handleInput("\u001b");
    assert.deepEqual(aborted, [1], "Escape while busy cancels the owned operation");
    selector.settle(0, "info", "obsolete");
    assert.doesNotMatch(selector.render(120).join("\n"), /obsolete/);
    selector.settle(1, "error", "Cancelled; no file was written or opened");
    assert.match(selector.render(120).join("\n"), /Cancelled/);
    selector.settle(1, "info", "late duplicate");
    assert.doesNotMatch(selector.render(120).join("\n"), /late duplicate/);
    selector.handleInput("s");
    assert.match(selector.render(120).join("\n"), /client-local path/);
    selector.handleInput("\r");
    assert.deepEqual(started[1], [2, "save", REPORT.name, REPORT.name]);
  });

  it("never renders a hostile delivered name through the editable destination", () => {
    for (const name of HOSTILE_NAMES) {
      const hostile = file(name, `${name} description`);
      const selector = new DeliverySelector(
        pageDeliveries(page([entry("1", toolMessage("m1", "c1", "tool-present", toolResult({ deliveries: [hostile] })))])),
        {},
      );
      selector.focused = true;
      const started: Array<string | undefined> = [];
      selector.onAction = (_operation, _action, record, destination) => {
        assert.equal(record.file.name, name, "the authoritative name is never altered");
        started.push(destination);
      };
      assert.deepEqual(foreignControls(selector.render(120)), [], `list: ${JSON.stringify(name)}`);
      selector.handleInput("s");
      const rendered = selector.render(120);
      assert.deepEqual(foreignControls(rendered), [], `destination: ${JSON.stringify(name)}`);
      assert.match(rendered.join("\n"), /cannot be shown as typed/);
      selector.handleInput("\r");
      assert.deepEqual(started, [], "no prefilled destination to submit");
      // A paste carrying controls is refused whole; typed safe text is kept.
      selector.handleInput("\u001b[200~evil\u001b]0;x\u0007\u202e\u001b[201~");
      assert.deepEqual(foreignControls(selector.render(120)), []);
      for (const character of "copy.md") selector.handleInput(character);
      selector.handleInput("\r");
      assert.deepEqual(started, ["copy.md"], `safe explicit destination for ${JSON.stringify(name)}`);
      selector.settle(1, "error", `Failed: ${name}`);
      assert.deepEqual(foreignControls(selector.render(120)), [], "outcome text is sanitized");
    }
  });

  it("prefills ordinary Unicode names exactly and saves them byte-exactly", async () => {
    const dir = mkdtempSync(join(tmpdir(), "rustx-delivery-names-"));
    try {
      for (const name of ["报告 final.md", "data set.csv", "naïve résumé — v2.txt"]) {
        const selector = new DeliverySelector(
          pageDeliveries(page([entry("1", toolMessage("m1", "c1", "tool-present", toolResult({ deliveries: [file(name)] })))])),
          {},
        );
        let destination: string | undefined;
        selector.onAction = (_operation, _action, _record, chosen) => { destination = chosen; };
        selector.handleInput("s");
        assert.match(selector.render(160).join("\n"), new RegExp(name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
        selector.handleInput("\r");
        assert.equal(destination, name, "the original name, unchanged");
        const bytes = Buffer.from(`${name}\r\n\u0000`, "utf8");
        const target = deliveryDestination(destination!, dir);
        assert.equal(await saveDelivery(async () => ({ data: bytes.toString("base64") }), target), join(dir, name));
        assert.deepEqual(readFileSync(join(dir, name)), bytes);
      }
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("loads older history only on an explicit request", () => {
    const selector = new DeliverySelector({ records: [], next: "older" }, {});
    let loads = 0;
    selector.onLoadMore = () => { loads += 1; };
    assert.match(selector.render(120).join("\n"), /No committed deliveries in the loaded history/);
    selector.handleInput("\r");
    selector.handleInput("\r");
    assert.equal(loads, 1, "one bounded page per request");
    selector.appendPage({ records: records.records });
    assert.doesNotMatch(selector.render(120).join("\n"), /Load older history/);
  });
});
