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
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";

import {
  DELIVERY_MAX_BASE64,
  DELIVERY_MAX_BYTES,
  decodeDelivery,
  deliveryDestination,
  openDelivery,
  saveDelivery,
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

  it("resolves explicit client-local destinations", () => {
    assert.equal(deliveryDestination("报告 final.md", "", "/client/cwd", "/home/u"), "/client/cwd/报告 final.md");
    assert.equal(deliveryDestination("x", "~/out/y", "/client/cwd", "/home/u"), "/home/u/out/y");
    assert.throws(() => deliveryDestination("x", "a\nb"));
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
