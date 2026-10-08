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
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";

import {
  DELIVERY_MAX_BASE64,
  DELIVERY_MAX_BYTES,
  DeliveryResidueError,
  DeliveryUncertainError,
  SAVE_CHUNK_BYTES,
  SAVE_FILES,
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
type Written = { bytesWritten: number };
/** Interposition points on the staged file's handle. */
interface HandleHooks {
  write?: (call: number, proceed: () => Promise<Written>) => Promise<Written>;
  sync?: (proceed: () => Promise<void>) => Promise<void>;
  close?: (proceed: () => Promise<void>) => Promise<void>;
}
/** The real Save filesystem operations, with explicit interposition points. */
function saveFiles(over: Partial<SaveFiles> = {}, hooks: HandleHooks = {}): SaveFiles {
  return {
    ...SAVE_FILES,
    open: async (path, flags, mode) => {
      const handle = await SAVE_FILES.open(path, flags, mode);
      const write = handle.write.bind(handle) as (buffer: Uint8Array, offset: number, length: number) => Promise<Written>;
      const sync = handle.sync.bind(handle);
      const close = handle.close.bind(handle);
      let writes = 0;
      Object.assign(handle, {
        write: (buffer: Uint8Array, offset: number, length: number) => {
          writes += 1;
          const proceed = () => write(buffer, offset, length);
          return hooks.write ? hooks.write(writes, proceed) : proceed();
        },
        sync: () => hooks.sync ? hooks.sync(sync) : sync(),
        close: () => hooks.close ? hooks.close(close) : close(),
      });
      return handle;
    },
    ...over,
  };
}
/** Real files whose `call`th chunk write parks, before writing, until released. */
function parkedWrite(call = 2): { files: SaveFiles; parked: Promise<void>; release: () => void } {
  const parked = deferred<void>();
  const proceed = deferred<void>();
  return {
    parked: parked.promise,
    release: () => proceed.resolve(),
    files: saveFiles({}, {
      write: async (writes, write) => {
        if (writes === call) { parked.resolve(); await proceed.promise; }
        return write();
      },
    }),
  };
}
function failure(code: string): NodeJS.ErrnoException {
  return Object.assign(new Error(`${code}: injected`), { code });
}
/** The directory's entries; a Save leaves only what it published. */
function listing(dir: string): string[] {
  return readdirSync(dir).sort();
}
function staging(dir: string): string[] {
  return listing(dir).filter((name) => name.startsWith(".rustx-save-"));
}
async function inDir(prefix: string, body: (dir: string) => Promise<void>): Promise<void> {
  const dir = mkdtempSync(join(tmpdir(), prefix));
  try {
    await body(dir);
  } finally {
    chmodSync(dir, 0o700);
    rmSync(dir, { recursive: true, force: true });
  }
}
const BODY_BYTES = Buffer.concat([Buffer.from("line\r\n\u0000报告 ", "utf8"), Buffer.alloc(SAVE_CHUNK_BYTES * 2, 0xff)]);
const BODY = { data: BODY_BYTES.toString("base64") };
const answer = async () => BODY;

/** Distinct valid names that trimming or normalization would conflate. */
const FIDELITY_NAMES = ["report.md", " report.md", "report.md ", "  report final.md  ", "报告 final.md", "naïve résumé — v2.txt"];
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

  it("keeps the typed destination spelling exactly, relative to the cwd", () => {
    for (const name of FIDELITY_NAMES) {
      assert.equal(deliveryDestination(name, "/client/cwd", "/home/u"), `/client/cwd/${name}`, JSON.stringify(name));
    }
    assert.equal(deliveryDestination("~/out/ y ", "/client/cwd", "/home/u"), "/home/u/out/ y ");
    assert.equal(deliveryDestination("~", "/client/cwd", "/home/u"), "/home/u");
    assert.equal(deliveryDestination("/abs/ x/../y ", "/client/cwd"), "/abs/ x/../y ", "absolute, `..` left to the OS");
    assert.equal(deliveryDestination("link/../x", "/"), "/link/../x");
    for (const unsafe of ["", "  ", "\t", ...HOSTILE_NAMES]) {
      assert.throws(() => deliveryDestination(unsafe, "/client/cwd"), /Invalid save destination/, JSON.stringify(unsafe));
    }
  });
});

describe("atomic Save publication", () => {
  it("publishes only the complete staged file, at the exact destination, and removes its staging", async () => {
    await inDir("rustx-save-publish-", async (dir) => {
      const destination = join(dir, "报告 final.md");
      const writes = parkedWrite(2);
      const saving = saveDelivery(answer, destination, undefined, writes.files);
      await writes.parked;
      assert.equal(existsSync(destination), false, "nothing is visible at the destination mid-write");
      const [stage] = staging(dir);
      assert.ok(stage, "one private staging directory beside the destination");
      assert.equal(statSync(join(dir, stage)).mode & 0o777, 0o700);
      writes.release();
      assert.deepEqual(await saving, { path: destination });
      assert.deepEqual(readFileSync(destination), BODY_BYTES);
      assert.equal(lstatSync(destination).nlink, 1, "the staged name is gone");
      assert.deepEqual(listing(dir), ["报告 final.md"]);
    });
  });

  it("saves every name at exactly its spelled path, byte-exactly", async () => {
    await inDir("rustx-save-names-", async (dir) => {
      for (const name of FIDELITY_NAMES) {
        const bytes = Buffer.from(`${name}\r\n\u0000\xff`, "latin1");
        const destination = deliveryDestination(name, dir);
        assert.equal(destination, `${dir}/${name}`);
        assert.deepEqual(await saveDelivery(async () => ({ data: bytes.toString("base64") }), destination), { path: destination });
        assert.deepEqual(readFileSync(`${dir}/${name}`), bytes, JSON.stringify(name));
      }
      assert.deepEqual(listing(dir), [...FIDELITY_NAMES].sort(), "no trimmed or normalized sibling");
    });
  });

  it("never replaces an existing entry, whoever created it and whenever", async () => {
    await inDir("rustx-save-clobber-", async (dir) => {
      const existing = join(dir, "existing");
      writeFileSync(existing, "kept");
      await assert.rejects(saveDelivery(answer, existing), /already exists/);
      assert.equal(readFileSync(existing, "utf8"), "kept");

      // Created by another writer while this save was staging.
      const raced = join(dir, "raced");
      const writes = parkedWrite(2);
      const saving = saveDelivery(answer, raced, undefined, writes.files);
      await writes.parked;
      writeFileSync(raced, "theirs");
      writes.release();
      await assert.rejects(saving, /already exists/);
      assert.equal(readFileSync(raced, "utf8"), "theirs");

      // Two complete saves admitted to publication together: one publishes.
      const contested = join(dir, "contested");
      const admitted = [deferred<void>(), deferred<void>()];
      const go = deferred<void>();
      const contender = (index: number) => saveDelivery(
        async () => ({ data: Buffer.from(`save ${index}`).toString("base64") }),
        contested,
        undefined,
        saveFiles({ link: async (existing, path) => { admitted[index]!.resolve(); await go.promise; return SAVE_FILES.link(existing, path); } }),
      );
      const contenders = [contender(0), contender(1)];
      await Promise.all(admitted.map((gate) => gate.promise));
      go.resolve();
      const settled = await Promise.allSettled(contenders);
      const won = settled.findIndex((result) => result.status === "fulfilled");
      assert.equal(settled.filter((result) => result.status === "fulfilled").length, 1);
      assert.match(String((settled[1 - won] as PromiseRejectedResult).reason), /already exists/);
      assert.equal(readFileSync(contested, "utf8"), `save ${won}`);

      // A symlink, dangling or not, and a directory are entries too; nothing
      // is created through the link.
      const target = join(dir, "symlink target");
      writeFileSync(target, "target");
      symlinkSync(target, join(dir, "to file"));
      symlinkSync(join(dir, "nowhere"), join(dir, "dangling"));
      mkdirSync(join(dir, "a directory"));
      for (const name of ["to file", "dangling", "a directory"]) {
        await assert.rejects(saveDelivery(answer, join(dir, name)), /already exists/, name);
      }
      assert.equal(readFileSync(target, "utf8"), "target");
      assert.equal(readlinkSync(join(dir, "to file")), target);
      assert.equal(existsSync(join(dir, "nowhere")), false, "a dangling link is not followed");
      assert.deepEqual(readdirSync(join(dir, "a directory")), []);
      assert.deepEqual(staging(dir), []);
    });
  });

  it("fails explicitly, without a fallback, where the filesystem cannot link", async () => {
    await inDir("rustx-save-nolink-", async (dir) => {
      for (const code of ["EPERM", "ENOTSUP", "EOPNOTSUPP"]) {
        const destination = join(dir, `out ${code}`);
        await assert.rejects(
          saveDelivery(answer, destination, undefined, saveFiles({ link: async () => { throw failure(code); } })),
          new RegExp(`cannot publish .* atomically without overwriting \\(${code}\\)`),
        );
        assert.equal(existsSync(destination), false);
      }
      assert.deepEqual(listing(dir), []);
    });
  });

  it("cancellation before publication admission publishes nothing and leaves nothing", async () => {
    await inDir("rustx-save-cancel-", async (dir) => {
      let links = 0;
      const counted = (over: Partial<SaveFiles> = {}, hooks: HandleHooks = {}) =>
        saveFiles({ link: async (existing, path) => { links += 1; return SAVE_FILES.link(existing, path); }, ...over }, hooks);

      // Before staging begins: the read answered, cancellation won.
      const beforeStaging = new AbortController();
      const answered = deferred<{ data: string }>();
      let staged = 0;
      const early = saveDelivery(() => answered.promise, join(dir, "early"), beforeStaging.signal,
        counted({ mkdtemp: async (prefix) => { staged += 1; return SAVE_FILES.mkdtemp(prefix); } }));
      beforeStaging.abort();
      answered.resolve(BODY);
      await assert.rejects(early, (error) => error === beforeStaging.signal.reason);
      assert.equal(staged, 0, "no staging allocated");

      // During the first write, and between chunks.
      for (const [name, hook] of [
        ["first", (abort: AbortController) => async (_call: number, write: () => Promise<Written>) => {
          abort.abort();
          return write();
        }],
        ["between", (abort: AbortController) => async (call: number, write: () => Promise<Written>) => {
          const written = await write();
          if (call === 1) abort.abort();
          return written;
        }],
      ] as const) {
        const abort = new AbortController();
        await assert.rejects(
          saveDelivery(answer, join(dir, name), abort.signal, counted({}, { write: hook(abort) })),
          (error) => error === abort.signal.reason,
        );
      }

      // After sync and close, before admission.
      const synced = new AbortController();
      await assert.rejects(
        saveDelivery(answer, join(dir, "synced"), synced.signal, counted({}, {
          sync: async (sync) => { await sync(); synced.abort(); },
        })),
        (error) => error === synced.signal.reason,
      );

      // A destination another writer created meanwhile is theirs, untouched.
      const theirs = new AbortController();
      const writes = parkedWrite(2);
      const replaced = saveDelivery(answer, join(dir, "theirs"), theirs.signal, writes.files);
      await writes.parked;
      writeFileSync(join(dir, "theirs"), "theirs");
      theirs.abort();
      writes.release();
      await assert.rejects(replaced, (error) => error === theirs.signal.reason);
      assert.equal(readFileSync(join(dir, "theirs"), "utf8"), "theirs");

      assert.equal(links, 0, "no publication was attempted");
      assert.deepEqual(listing(dir), ["theirs"], "no partial file and no staging");
    });
  });

  it("once publication is dispatched its result decides, and a later cancel never undoes it", async () => {
    await inDir("rustx-save-dispatched-", async (dir) => {
      const dispatched = new AbortController();
      const linked = deferred<void>();
      const reply = deferred<void>();
      const destination = join(dir, "dispatched");
      const saving = saveDelivery(answer, destination, dispatched.signal, saveFiles({
        link: async (existing, path) => { await SAVE_FILES.link(existing, path); linked.resolve(); await reply.promise; },
      }));
      await linked.promise;
      dispatched.abort();
      reply.resolve();
      assert.deepEqual(await saving, { path: destination }, "reported as saved, not cancelled");
      assert.deepEqual(readFileSync(destination), BODY_BYTES);

      const after = new AbortController();
      const kept = join(dir, "kept");
      assert.deepEqual(await saveDelivery(answer, kept, after.signal), { path: kept });
      after.abort();
      assert.deepEqual(readFileSync(kept), BODY_BYTES);
      assert.deepEqual(listing(dir), ["dispatched", "kept"]);
    });
  });

  it("a failed write, sync, close or link leaves the destination absent and no staging", async () => {
    await inDir("rustx-save-fail-", async (dir) => {
      const cases: [string, SaveFiles, RegExp][] = [
        ["write", saveFiles({}, { write: async (call, write) => { if (call === 2) throw failure("EIO"); return write(); } }), /EIO/],
        ["short", saveFiles({}, { write: async () => ({ bytesWritten: 0 }) }), /stopped accepting bytes/],
        ["sync", saveFiles({}, { sync: async () => { throw failure("EIO"); } }), /EIO/],
        ["close", saveFiles({}, { close: async (close) => { await close(); throw failure("EIO"); } }), /EIO/],
        ["link", saveFiles({ link: async () => { throw failure("EIO"); } }), /EIO/],
        ["mkdtemp", saveFiles({ mkdtemp: async () => { throw failure("EACCES"); } }), /EACCES/],
      ];
      for (const [name, files, expected] of cases) {
        await assert.rejects(saveDelivery(answer, join(dir, name), undefined, files), expected, name);
      }
      assert.deepEqual(listing(dir), []);
    });
  });

  it("decides an ambiguous link failure by the destination's identity", async () => {
    await inDir("rustx-save-evidence-", async (dir) => {
      // The entry exists and names the staged file: that is a publication.
      const created = join(dir, "created");
      assert.deepEqual(await saveDelivery(answer, created, undefined, saveFiles({
        link: async (existing, path) => { await SAVE_FILES.link(existing, path); throw failure("EIO"); },
      })), { path: created });
      assert.deepEqual(readFileSync(created), BODY_BYTES);

      // The destination cannot be inspected: the outcome is uncertain, said so.
      const unknown = join(dir, "unknown");
      const uncertain = await saveDelivery(answer, unknown, undefined, saveFiles({
        link: async () => { throw failure("EIO"); },
        lstat: async () => { throw failure("EACCES"); },
      })).catch((error: unknown) => error);
      assert.ok(uncertain instanceof DeliveryUncertainError);
      assert.equal(uncertain.path, unknown);
      assert.equal(uncertain.residue, undefined);
      assert.match(uncertain.message, /outcome unknown/);
      assert.deepEqual(listing(dir), ["created"]);
    });
  });

  it("reports staging that cannot be removed, keeping the publication outcome", async () => {
    await inDir("rustx-save-residue-", async (dir) => {
      // Published, then staging cleanup failed: still saved, with a warning.
      const published = join(dir, "published");
      const saved = await saveDelivery(answer, published, undefined, saveFiles({ rmdir: async () => { throw failure("EACCES"); } }));
      assert.equal(saved.path, published);
      assert.match(String((saved.residue?.cause as Error).message), /EACCES/);
      assert.deepEqual(readFileSync(published), BODY_BYTES);
      assert.ok(existsSync(saved.residue!.path), "the staging directory is retained, not deleted recursively");

      // Not published (cancelled), staged name not removable: the original
      // cancellation is kept as the cause, and the destination is absent.
      const abort = new AbortController();
      const kept = await saveDelivery(answer, join(dir, "cancelled"), abort.signal, saveFiles(
        { unlink: async () => { throw failure("EIO"); } },
        { sync: async (sync) => { await sync(); abort.abort(); } },
      )).catch((error: unknown) => error);
      assert.ok(kept instanceof DeliveryResidueError);
      assert.equal(kept.cause, abort.signal.reason);
      assert.match(String((kept.cleanup as Error).message), /EIO/);
      assert.match(kept.message, /^Nothing was saved/);
      assert.equal(existsSync(join(dir, "cancelled")), false);
      assert.ok(existsSync(join(kept.residue, "file")));

      // Someone else's entry inside the staging directory is never removed.
      const foreign = join(dir, "foreign");
      const retained = staging(dir);
      const writes = parkedWrite(2);
      const saving = saveDelivery(answer, foreign, undefined, writes.files);
      await writes.parked;
      const [stage] = staging(dir).filter((name) => !retained.includes(name));
      const intruder = join(dir, stage!, "not ours");
      writeFileSync(intruder, "not ours");
      writes.release();
      const outcome = await saving;
      assert.equal(outcome.path, foreign);
      assert.equal(outcome.residue?.path, join(dir, stage!));
      assert.match(String((outcome.residue?.cause as Error).message), /ENOTEMPTY|EEXIST/);
      assert.equal(readFileSync(intruder, "utf8"), "not ours");
      assert.deepEqual(readFileSync(foreign), BODY_BYTES);
    });
  });

  it("rejects a malformed body before allocating anything", async () => {
    await inDir("rustx-save-malformed-", async (dir) => {
      await assert.rejects(saveDelivery(async () => ({ data: "!" }), join(dir, "malformed")), /bounded base64/);
      assert.deepEqual(listing(dir), []);
    });
  });
});

describe("client-local Open", () => {
  it("an Open cancelled before launch launches nothing; after launch it is not undone", async () => {
    const dir = mkdtempSync(join(tmpdir(), "rustx-delivery-open-"));
    try {
      const path = join(dir, "报告 final.md");
      writeFileSync(path, "x");
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

  it("reports an uninspectable local path as such, never as absent", async () => {
    if (process.getuid?.() === 0) return;
    await inDir("rustx-open-eacces-", async (dir) => {
      mkdirSync(join(dir, "sealed"));
      writeFileSync(join(dir, "sealed", "x"), "x");
      chmodSync(join(dir, "sealed"), 0o000);
      try {
        await assert.rejects(openDelivery(async () => ({ file: REPORT, path: join(dir, "sealed", "x"), device: "1", inode: "2" }), {
          sharedHost: true, opener: "xdg-open", launch: async () => assert.fail("never launched"),
        }), /cannot be inspected \(EACCES\)/);
      } finally {
        chmodSync(join(dir, "sealed"), 0o700);
      }
    });
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
    selector.settle(1, "error", "Cancelled; nothing was saved or opened");
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
        assert.deepEqual(await saveDelivery(async () => ({ data: bytes.toString("base64") }), target), { path: join(dir, name) });
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
