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
  accessSync,
  chmodSync,
  constants,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { execFileSync, spawnSync } from "node:child_process";
import type { BigIntStats } from "node:fs";
import { tmpdir, userInfo } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
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
  localEffect,
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
/** Interposition points on the staged file: its exclusive create, and its handle. */
interface HandleHooks {
  /** Runs immediately before the exclusive create of `path`. */
  create?: (path: string) => void;
  write?: (call: number, proceed: () => Promise<Written>) => Promise<Written>;
  sync?: (proceed: () => Promise<void>) => Promise<void>;
  close?: (proceed: () => Promise<void>) => Promise<void>;
  /** The `call`th `stat` of the staged file's handle. */
  stat?: (call: number, proceed: () => Promise<BigIntStats>) => Promise<BigIntStats>;
}
/** Save's filesystem operations, plus the staged path it created and the
 * mode it asked that creation for. */
type Interposed = SaveFiles & { staged: () => string; requested: () => number | undefined };
/** The real Save filesystem operations, with explicit interposition points
 * on the staged file (the one file Save creates). */
function saveFiles(over: Partial<SaveFiles> = {}, hooks: HandleHooks = {}): Interposed {
  let staged: string | undefined;
  let requested: number | undefined;
  return {
    ...SAVE_FILES,
    open: async (path, flags, mode) => {
      staged = path;
      requested = mode;
      hooks.create?.(path);
      const handle = await SAVE_FILES.open(path, flags, mode);
      const write = handle.write.bind(handle) as (buffer: Uint8Array, offset: number, length: number) => Promise<Written>;
      const sync = handle.sync.bind(handle);
      const close = handle.close.bind(handle);
      const fstat = handle.stat.bind(handle) as (options: { bigint: true }) => Promise<BigIntStats>;
      const stat = () => fstat({ bigint: true });
      let writes = 0;
      let stats = 0;
      Object.assign(handle, {
        write: (buffer: Uint8Array, offset: number, length: number) => {
          writes += 1;
          const proceed = () => write(buffer, offset, length);
          return hooks.write ? hooks.write(writes, proceed) : proceed();
        },
        sync: () => hooks.sync ? hooks.sync(sync) : sync(),
        close: () => hooks.close ? hooks.close(close) : close(),
        stat: () => {
          stats += 1;
          return hooks.stat ? hooks.stat(stats, stat) : stat();
        },
      });
      return handle;
    },
    ...over,
    staged: () => {
      assert.ok(staged !== undefined, "this save created its staged file");
      return staged;
    },
    requested: () => requested,
  };
}
/** Real files whose `call`th chunk write parks, before writing, until released. */
function parkedWrite(call = 2): { files: Interposed; parked: Promise<void>; release: () => void } {
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
/** An entry's device and inode: what identifies a file, unlike its name. */
function identity(path: string): string {
  const { dev, ino } = lstatSync(path, { bigint: true });
  return `${dev}:${ino}`;
}
function failure(code: string): NodeJS.ErrnoException {
  return Object.assign(new Error(`${code}: injected`), { code });
}
/** The directory's entries; a Save leaves only what it published. */
function listing(dir: string): string[] {
  return readdirSync(dir).sort();
}
/** `path`'s ACL entries as macOS `ls -le` prints them, without indices. */
function aclEntries(path: string): string[] {
  const lines = execFileSync("/bin/ls", ["-led", path], { encoding: "utf8" }).split("\n").slice(1);
  return lines.map((line) => line.replace(/^\s*\d+:\s*/, "")).filter((line) => line !== "");
}
function staging(dir: string): string[] {
  return listing(dir).filter((name) => name.startsWith(".rustx-save-"));
}
/** Someone who may write the parent moves `path` to `to`. */
function moved(path: string, to: string): string {
  renameSync(path, to);
  return to;
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
      assert.ok(stage, "one staged file beside the destination");
      assert.equal(join(dir, stage), writes.files.staged());
      assert.match(stage, /^\.rustx-save-[0-9a-f]{32}$/);
      assert.ok(lstatSync(join(dir, stage)).isFile());
      const written = identity(join(dir, stage));
      writes.release();
      assert.deepEqual(await saving, { path: destination });
      assert.deepEqual(readFileSync(destination), BODY_BYTES);
      assert.equal(identity(destination), written, "the very file this save created and wrote");
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

      // 10. Created by another writer while this save was staging.
      const raced = join(dir, "raced");
      const writes = parkedWrite(2);
      const saving = saveDelivery(answer, raced, undefined, writes.files);
      await writes.parked;
      writeFileSync(raced, "theirs");
      const theirs = identity(raced);
      writes.release();
      await assert.rejects(saving, /already exists/);
      assert.equal(readFileSync(raced, "utf8"), "theirs");
      assert.equal(identity(raced), theirs);

      // 10. Two complete saves admitted to publication together: one publishes.
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

  it("11. cancellation before publication admission publishes nothing and leaves nothing", async () => {
    await inDir("rustx-save-cancel-", async (dir) => {
      let links = 0;
      const counted = (over: Partial<SaveFiles> = {}, hooks: HandleHooks = {}) =>
        saveFiles({ link: async (existing, path) => { links += 1; return SAVE_FILES.link(existing, path); }, ...over }, hooks);

      // Before staging begins: the read answered, cancellation won.
      const beforeStaging = new AbortController();
      const answered = deferred<{ data: string }>();
      let staged = 0;
      const early = saveDelivery(() => answered.promise, join(dir, "early"), beforeStaging.signal,
        counted({}, { create: () => { staged += 1; } }));
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

      // After sync, before admission.
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

  it("12. once publication is dispatched its result decides, and a later cancel never undoes it", async () => {
    await inDir("rustx-save-dispatched-", async (dir) => {
      const dispatched = new AbortController();
      const linked = deferred<void>();
      const reply = deferred<void>();
      const destination = join(dir, "dispatched");
      const effect = localEffect();
      const saving = saveDelivery(answer, destination, dispatched.signal, saveFiles({
        link: async (existing, path) => { await SAVE_FILES.link(existing, path); linked.resolve(); await reply.promise; },
      }), effect);
      await linked.promise;
      assert.equal(effect.committed, true, "recorded at dispatch");
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

  it("a failed create, write, sync or link leaves the destination absent and no staging", async () => {
    await inDir("rustx-save-fail-", async (dir) => {
      const cases: [string, SaveFiles, RegExp][] = [
        ["create", saveFiles({ open: async () => { throw failure("EACCES"); } }), /EACCES/],
        ["write", saveFiles({}, { write: async (call, write) => { if (call === 2) throw failure("EIO"); return write(); } }), /EIO/],
        ["short", saveFiles({}, { write: async () => ({ bytesWritten: 0 }) }), /stopped accepting bytes/],
        ["sync", saveFiles({}, { sync: async () => { throw failure("EIO"); } }), /EIO/],
        ["link", saveFiles({ link: async () => { throw failure("EACCES"); } }), /EACCES/],
      ];
      for (const [name, files, expected] of cases) {
        await assert.rejects(saveDelivery(answer, join(dir, name), undefined, files), expected, name);
      }
      assert.deepEqual(listing(dir), []);

      // Closing comes after the outcome and the cleanup: it cannot change them.
      const closing = join(dir, "closing");
      const closed = await saveDelivery(answer, closing, undefined, saveFiles({}, {
        close: async (close) => { await close(); throw failure("EIO"); },
      }));
      assert.deepEqual(closed, { path: closing });
      assert.deepEqual(readFileSync(closing), BODY_BYTES);
      assert.deepEqual(listing(dir), ["closing"]);
    });
  });

  it("13. claims publication or refusal only on evidence; otherwise the outcome is uncertain", async () => {
    await inDir("rustx-save-evidence-", async (dir) => {
      /** A real link whose acknowledgement is replaced by `code`, after `meanwhile`. */
      const linkedThen = (code: string | undefined, meanwhile: (path: string) => void = () => {}) => saveFiles({
        link: async (existing, path) => {
          await SAVE_FILES.link(existing, path);
          meanwhile(path);
          if (code !== undefined) throw failure(code);
        },
      });
      const outcome = (destination: string, files: SaveFiles) =>
        saveDelivery(answer, destination, undefined, files).catch((error: unknown) => error);
      /** Someone replaces `path` with a new file of their own. */
      const replace = (path: string) => {
        writeFileSync(join(dir, "replacement"), "theirs");
        renameSync(join(dir, "replacement"), path);
      };

      // C. The link committed and the destination still names the staged
      // file: published, whatever the error (EIO, or EEXIST from a
      // retransmitted request).
      for (const code of ["EIO", "EEXIST"]) {
        const created = join(dir, `created ${code}`);
        assert.deepEqual(await outcome(created, linkedThen(code)), { path: created });
        assert.deepEqual(readFileSync(created), BODY_BYTES);
      }

      // D. The link committed, someone removed the entry, then EIO: absence
      // does not prove nothing was published.
      const removed = join(dir, "removed");
      const gone = await outcome(removed, linkedThen("EIO", (path) => rmSync(path)));
      assert.ok(gone instanceof DeliveryUncertainError, String(gone));
      assert.equal(gone.linked, false);
      assert.deepEqual(gone.observed, { kind: "absent" });
      assert.equal((gone.cause as NodeJS.ErrnoException).code, "EIO");
      assert.match(gone.message, /outcome unknown: linking .*removed failed \(EIO\) and it is now absent; this save may have created it$/);

      // E. The link committed, someone replaced the entry, then EIO: a
      // foreign file does not prove it either, and it is never touched.
      const replaced = join(dir, "replaced");
      const foreign = await outcome(replaced, linkedThen("EIO", replace));
      assert.ok(foreign instanceof DeliveryUncertainError, String(foreign));
      assert.deepEqual(foreign.observed, { kind: "foreign" });
      assert.equal(readFileSync(replaced, "utf8"), "theirs");

      // G. The link succeeded, but by the time it is checked the destination
      // names another file: a successful link alone is not publication.
      const overtaken = join(dir, "overtaken");
      const unshown = await outcome(overtaken, linkedThen(undefined, replace));
      assert.ok(unshown instanceof DeliveryUncertainError, String(unshown));
      assert.equal(unshown.linked, true);
      assert.equal(unshown.cause, undefined);
      assert.deepEqual(unshown.observed, { kind: "foreign" });
      assert.equal(unshown.residue, undefined, "its file lost the destination name, and the staged name is removed");
      assert.match(unshown.message, /linking .*overtaken succeeded, but it now names another file, so this save cannot show it holds the saved bytes$/);
      assert.equal(readFileSync(overtaken, "utf8"), "theirs");

      // F. An ambiguous error and an uninspectable destination: both kept.
      const unknown = join(dir, "unknown");
      const linkError = failure("EIO");
      const inspection = failure("EACCES");
      const blind = await outcome(unknown, saveFiles({
        link: async () => { throw linkError; },
        lstat: async (path, options) => {
          if (path === unknown) throw inspection;
          return SAVE_FILES.lstat(path, options);
        },
      }));
      assert.ok(blind instanceof DeliveryUncertainError);
      assert.equal(blind.path, unknown);
      assert.equal(blind.cause, linkError);
      assert.deepEqual(blind.observed, { kind: "uninspectable", error: inspection });
      assert.equal(blind.residue, undefined, "staging was removed; uncertainty stays");
      assert.match(blind.message, /cannot be inspected \(EACCES\)/);

      // A code that is not a known rejection is not one either.
      const unexplained = await outcome(join(dir, "unexplained"), saveFiles({
        link: async () => { throw new Error("no code"); },
      }));
      assert.ok(unexplained instanceof DeliveryUncertainError);

      // B. A definite rejection with a foreign destination is a refusal.
      writeFileSync(join(dir, "occupied"), "kept");
      const refused = await outcome(join(dir, "occupied"), saveFiles({
        link: async () => { throw failure("EEXIST"); },
      }));
      assert.ok(!(refused instanceof DeliveryUncertainError));
      assert.match(String(refused), /already exists/);
      assert.equal(readFileSync(join(dir, "occupied"), "utf8"), "kept");

      assert.deepEqual(staging(dir), [], "every staged file was removed");
      assert.deepEqual(listing(dir), ["created EEXIST", "created EIO", "occupied", "overtaken", "replaced"]);
    });
  });

  it("stages in the destination's parent as the OS resolves it, through symlinks and `..`", async () => {
    await inDir("rustx-save-dotdot-", async (root) => {
      const workspace = join(root, "workspace");
      const other = join(root, "other");
      mkdirSync(join(other, "nested"), { recursive: true });
      mkdirSync(workspace);
      symlinkSync("../other/nested/", join(workspace, "link"));
      // Lexically `workspace/link/..` is `workspace`; the OS resolves `link`
      // first, so it is `other`.
      const destination = deliveryDestination("link/../报告 final.md", workspace);
      assert.equal(destination, `${workspace}/link/../报告 final.md`, "the meaningful components are kept");
      const settled = () => {
        assert.deepEqual(listing(workspace), ["link"], "nothing in the lexical parent");
        assert.deepEqual(listing(join(other, "nested")), []);
        assert.deepEqual(listing(root), ["other", "workspace"]);
      };

      // Cancelled mid-write: staging was in the real parent, and is gone.
      const abort = new AbortController();
      const cancelled = parkedWrite(2);
      const cancelling = saveDelivery(answer, destination, abort.signal, cancelled.files);
      await cancelled.parked;
      assert.equal(staging(other).length, 1, "staging beside the real destination");
      assert.deepEqual(staging(workspace), []);
      abort.abort();
      cancelled.release();
      await assert.rejects(cancelling, (error) => error === abort.signal.reason);
      assert.deepEqual(listing(other), ["nested"]);
      settled();

      // Failed write: same.
      await assert.rejects(saveDelivery(answer, destination, undefined, saveFiles({}, {
        write: async () => { throw failure("EIO"); },
      })), /EIO/);
      assert.deepEqual(listing(other), ["nested"]);
      settled();

      // Residue is reported by the spelled path, which names the real one.
      const kept = await saveDelivery(answer, destination, undefined, saveFiles({ unlink: async () => { throw failure("EIO"); } }));
      const residue = kept.residue?.path;
      assert.ok(residue !== undefined && residue.startsWith(`${workspace}/link/../.rustx-save-`), residue);
      // `realpathSync.native` is realpath(3); Node's JS realpath folds `..`
      // lexically first, the very mistake under test.
      const real = realpathSync.native(residue);
      assert.equal(real, join(realpathSync.native(other), residue.slice(`${workspace}/link/../`.length)));
      assert.equal(identity(real), identity(join(other, "报告 final.md")), "the published file's other name");
      rmSync(real);
      rmSync(join(other, "报告 final.md"));

      // Published: the staged file is written there and linked there.
      const linked: [string, string][] = [];
      const writes = parkedWrite(2);
      const saving = saveDelivery(answer, destination, undefined, {
        ...writes.files,
        link: async (existing, path) => { linked.push([existing, path]); return SAVE_FILES.link(existing, path); },
      });
      await writes.parked;
      const [stage] = staging(other);
      assert.ok(stage, "the staged file is in the real parent");
      writes.release();
      assert.deepEqual(await saving, { path: destination });
      assert.deepEqual(linked, [[`${workspace}/link/../${stage}`, destination]], "both exactly as spelled");
      assert.deepEqual(readFileSync(join(other, "报告 final.md")), BODY_BYTES);
      assert.deepEqual(listing(other), ["nested", "报告 final.md"]);
      settled();

      // Ordinary relative and absolute paths, with spaces and Unicode.
      mkdirSync(join(workspace, "sub dir"));
      for (const [typed, real] of [
        ["sub dir/ naïve résumé.txt ", join(workspace, "sub dir", " naïve résumé.txt ")],
        [join(workspace, "sub dir", "报告.md"), join(workspace, "sub dir", "报告.md")],
      ] as const) {
        const target = deliveryDestination(typed, workspace);
        assert.equal(target, real);
        assert.deepEqual(await saveDelivery(answer, target), { path: real });
        assert.deepEqual(readFileSync(real), BODY_BYTES);
      }
      assert.deepEqual(listing(join(workspace, "sub dir")), [" naïve résumé.txt ", "报告.md"].sort());
    });
  });

  // The cases below interfere as a process that may modify the destination's
  // parent: the documented trust boundary. Save cannot stop such a process
  // from substituting a name between two of its steps; what they prove is
  // that Save then claims only what its own handle shows, never writes into
  // anything it did not create, and reports its own file wherever it went.

  it("1. takes ownership only from its exclusive create, never from a name, owner or type", async () => {
    await inDir("rustx-save-create-", async (dir) => {
      mkdirSync(join(dir, "elsewhere"));
      writeFileSync(join(dir, "elsewhere", "file"), "theirs");
      const plants: [string, (path: string) => void, (path: string) => void][] = [
        ["a file", (path) => writeFileSync(path, "theirs"), (path) => assert.equal(readFileSync(path, "utf8"), "theirs")],
        ["a symlink", (path) => symlinkSync(join(dir, "elsewhere", "file"), path),
          (path) => assert.equal(readlinkSync(path), join(dir, "elsewhere", "file"))],
        ["a directory", (path) => mkdirSync(path), (path) => assert.deepEqual(readdirSync(path), [])],
      ];
      for (const [kind, plant, unchanged] of plants) {
        const destination = join(dir, `out ${kind}`);
        const effect = localEffect();
        let before = "";
        const files = saveFiles({}, { create: (path) => { plant(path); before = identity(path); } });
        const refused = await saveDelivery(answer, destination, undefined, files, effect).catch((error: unknown) => error);
        assert.equal((refused as NodeJS.ErrnoException).code, "EEXIST", `${kind}: ${String(refused)}`);
        assert.equal(identity(files.staged()), before, `${kind}: the planted entry is the same object`);
        unchanged(files.staged());
        assert.equal(existsSync(destination), false, kind);
        assert.deepEqual(effect, { committed: false, residue: false }, kind);
      }
      assert.equal(readFileSync(join(dir, "elsewhere", "file"), "utf8"), "theirs", "nothing written through the symlink");
      assert.equal(staging(dir).length, 3, "the three planted entries, and nothing of this save's");
    });
  });

  it("2, 5. a staged name replaced by another same-user file before the link is never reported as published", async () => {
    await inDir("rustx-save-substituted-", async (dir) => {
      const boundaries: [string, (files: () => Interposed, swap: () => void) => HandleHooks][] = [
        // 2. Mid-write, after the first chunk.
        ["mid-write", (_files, swap) => ({ write: async (call, write) => { if (call === 2) swap(); return write(); } })],
        // 5. After every byte is synced, before admission and the link.
        ["synced", (_files, swap) => ({ sync: async (sync) => { await sync(); swap(); } })],
      ];
      for (const [boundary, hooks] of boundaries) {
        const destination = join(dir, `out ${boundary}`);
        let ours = "";
        let theirs = "";
        let away = "";
        const files: Interposed = saveFiles({}, hooks(() => files, () => {
          ours = identity(files.staged());
          away = moved(files.staged(), join(dir, `ours ${boundary}`));
          writeFileSync(files.staged(), "theirs");
          theirs = identity(files.staged());
        }));
        const effect = localEffect();
        const outcome = await saveDelivery(answer, destination, undefined, files, effect).catch((error: unknown) => error);
        assert.ok(outcome instanceof DeliveryUncertainError, `${boundary}: ${String(outcome)}`);
        assert.equal(outcome.linked, true, "the link itself succeeded");
        assert.deepEqual(outcome.observed, { kind: "foreign" });
        assert.notEqual(ours, theirs, "same user, another inode");
        assert.equal(identity(destination), theirs, "the link read the substituted name: the excluded actor's doing");
        assert.equal(readFileSync(destination, "utf8"), "theirs", "their bytes untouched by this save");
        assert.equal(identity(away), ours);
        assert.deepEqual(readFileSync(away), BODY_BYTES, "every byte written through its own handle, none into theirs");
        assert.equal(outcome.residue, files.staged(), "its own file, moved, is still on disk and reported");
        assert.deepEqual(effect, { committed: true, residue: true });
      }
    });
  });

  it("3. a staged file renamed during writing still receives every byte and is reported, not chased", async () => {
    await inDir("rustx-save-renamed-", async (dir) => {
      const destination = join(dir, "out");
      let ours = "";
      let away = "";
      const files: Interposed = saveFiles({}, {
        write: async (call, write) => {
          if (call === 1) { ours = identity(files.staged()); away = moved(files.staged(), join(dir, "moved")); }
          return write();
        },
      });
      const outcome = await saveDelivery(answer, destination, undefined, files).catch((error: unknown) => error);
      assert.ok(outcome instanceof DeliveryResidueError, String(outcome));
      assert.equal((outcome.cause as NodeJS.ErrnoException).code, "ENOENT", "the staged name no longer resolves");
      assert.match(String((outcome.cleanup as Error).message), /linked elsewhere, so it was not removed/);
      assert.match(outcome.message, /^Nothing was saved, but this save's staged file \(created at .*\) was not removed$/);
      assert.equal(outcome.residue, files.staged());
      assert.equal(existsSync(destination), false);
      assert.equal(identity(away), ours);
      assert.deepEqual(readFileSync(away), BODY_BYTES, "every byte, through the handle");
      assert.deepEqual(listing(dir), ["moved"]);
    });
  });

  it("4. a staged name replaced by a symlink never writes, links as published, or removes the target", async () => {
    await inDir("rustx-save-symlinked-", async (dir) => {
      mkdirSync(join(dir, "elsewhere"));
      const target = join(dir, "elsewhere", "file");
      writeFileSync(target, "theirs");
      const before = identity(target);
      const destination = join(dir, "out");
      let ours = "";
      let away = "";
      const files: Interposed = saveFiles({}, {
        write: async (call, write) => {
          if (call === 1) {
            ours = identity(files.staged());
            away = moved(files.staged(), join(dir, "moved"));
            symlinkSync(target, files.staged());
          }
          return write();
        },
      });
      const outcome = await saveDelivery(answer, destination, undefined, files).catch((error: unknown) => error);
      // Linux links the symlink itself, macOS its target: either way the
      // destination is not this save's file, and it says so.
      assert.ok(outcome instanceof DeliveryUncertainError, String(outcome));
      assert.equal(outcome.linked, true);
      assert.deepEqual(outcome.observed, { kind: "foreign" });
      assert.notEqual(identity(destination), ours);
      assert.equal(readFileSync(destination, "utf8"), "theirs");
      assert.equal(readFileSync(target, "utf8"), "theirs", "never written through the symlink");
      assert.equal(identity(target), before, "never unlinked");
      assert.deepEqual(readFileSync(away), BODY_BYTES);
      assert.equal(outcome.residue, files.staged());
    });
  });

  it("6, 7, 8. after publication, cleanup is judged by its own file, never by the staged name", async () => {
    await inDir("rustx-save-cleanup-", async (dir) => {
      /** Publishes, then lets `meanwhile` act after the link and before cleanup. */
      const published = async (name: string, meanwhile: (staged: string) => void, unlink?: SaveFiles["unlink"]) => {
        const destination = join(dir, name);
        const files: Interposed = saveFiles({
          link: async (existing, path) => { await SAVE_FILES.link(existing, path); meanwhile(files.staged()); },
          ...(unlink === undefined ? {} : { unlink }),
        });
        const saved = await saveDelivery(answer, destination, undefined, files);
        assert.equal(saved.path, destination);
        assert.deepEqual(readFileSync(destination), BODY_BYTES);
        return { saved, destination, staged: files.staged() };
      };

      // 7. Renamed after the link and before cleanup: the name is gone, the
      // file is not, so it is reported.
      const seven = await published("seven", (staged) => moved(staged, join(dir, "moved 7")));
      assert.equal(seven.saved.residue?.path, seven.staged);
      assert.match(String((seven.saved.residue?.cause as Error).message), /linked elsewhere/);
      assert.equal(identity(join(dir, "moved 7")), identity(seven.destination), "its own file, now two names");

      // 6. Renamed and replaced by another file before cleanup: the file is
      // reported the same way. Excluded actor: no Linux or macOS call removes
      // a name only while it names a given file, so the unlink removes what
      // the staged name now holds; the documentation says so.
      const six = await published("six", (staged) => {
        moved(staged, join(dir, "moved 6"));
        writeFileSync(staged, "theirs");
      });
      assert.equal(six.saved.residue?.path, six.staged);
      assert.equal(identity(join(dir, "moved 6")), identity(six.destination));
      assert.equal(existsSync(six.staged), false, "documented limit: the substituted name was unlinked");

      // 8. A foreign empty directory substituted immediately before the
      // removal: unlink never removes a directory, so it survives.
      const eight = await published("eight", () => {}, async (path) => {
        moved(path, join(dir, "moved 8"));
        mkdirSync(path);
        return SAVE_FILES.unlink(path);
      });
      assert.equal(eight.saved.residue?.path, eight.staged);
      assert.match(String((eight.saved.residue?.cause as NodeJS.ErrnoException).code), /^(EISDIR|EPERM)$/);
      assert.ok(lstatSync(eight.staged).isDirectory(), "the foreign directory survives");
      assert.deepEqual(readdirSync(eight.staged), []);
      assert.equal(identity(join(dir, "moved 8")), identity(eight.destination));

      // A file already gone everywhere but the destination is done, whatever
      // the name says.
      const unlinked = await published("unlinked", (staged) => rmSync(staged));
      assert.equal(unlinked.saved.residue, undefined);
      assert.equal(lstatSync(unlinked.destination).nlink, 1);
    });
  });

  it("9. a destination parent renamed between staging and publication publishes nothing and reports the staged file", async () => {
    await inDir("rustx-save-parent-", async (dir) => {
      mkdirSync(join(dir, "parent"));
      const destination = join(dir, "parent", "out.md");
      for (const [boundary, after] of [
        ["renamed away", () => {}],
        ["replaced by a new directory", () => mkdirSync(join(dir, "parent"))],
      ] as const) {
        const files = saveFiles({}, {
          sync: async (sync) => { await sync(); moved(join(dir, "parent"), join(dir, `parent ${boundary}`)); after(); },
        });
        const outcome = await saveDelivery(answer, destination, undefined, files).catch((error: unknown) => error);
        assert.ok(outcome instanceof DeliveryResidueError, `${boundary}: ${String(outcome)}`);
        assert.equal((outcome.cause as NodeJS.ErrnoException).code, "ENOENT", "the staged name no longer resolves");
        assert.equal(outcome.residue, files.staged());
        const [stage] = staging(join(dir, `parent ${boundary}`));
        assert.ok(stage, boundary);
        assert.deepEqual(readFileSync(join(dir, `parent ${boundary}`, stage)), BODY_BYTES, "kept where it was made");
        assert.deepEqual(listing(join(dir, `parent ${boundary}`)), [stage]);
        assert.equal(existsSync(destination), false);
        if (boundary === "renamed away") mkdirSync(join(dir, "parent"));
        else assert.deepEqual(listing(join(dir, "parent")), [], "nothing in the new directory");
        rmSync(join(dir, "parent"), { recursive: true });
        mkdirSync(join(dir, "parent"));
      }
    });
  });

  it("14. reports a staged file that cannot be removed, keeping the publication outcome", async () => {
    await inDir("rustx-save-residue-", async (dir) => {
      // Published, then staging cleanup failed: still saved, with a warning.
      const published = join(dir, "published");
      const saved = await saveDelivery(answer, published, undefined, saveFiles({ unlink: async () => { throw failure("EACCES"); } }));
      assert.equal(saved.path, published);
      assert.match(String((saved.residue?.cause as Error).message), /EACCES/);
      assert.deepEqual(readFileSync(published), BODY_BYTES);
      assert.equal(identity(saved.residue!.path), identity(published), "retained, never deleted another way");

      // Not published (cancelled), staged name not removable: the original
      // cancellation is kept as the cause, and the destination is absent.
      const abort = new AbortController();
      const effect = localEffect();
      const kept = await saveDelivery(answer, join(dir, "cancelled"), abort.signal, saveFiles(
        { unlink: async () => { throw failure("EIO"); } },
        { sync: async (sync) => { await sync(); abort.abort(); } },
      ), effect).catch((error: unknown) => error);
      assert.ok(kept instanceof DeliveryResidueError);
      assert.equal(kept.cause, abort.signal.reason);
      assert.match(String((kept.cleanup as Error).message), /EIO/);
      assert.match(kept.message, /^Nothing was saved/);
      assert.equal(existsSync(join(dir, "cancelled")), false);
      assert.deepEqual(readFileSync(kept.residue), BODY_BYTES);
      assert.deepEqual(effect, { committed: false, residue: true });

      // Uncertain, and the staged file not removable: both are reported.
      const uncertain = await saveDelivery(answer, join(dir, "uncertain"), undefined, saveFiles({
        link: async () => { throw failure("EIO"); },
        unlink: async () => { throw failure("EIO"); },
      })).catch((error: unknown) => error);
      assert.ok(uncertain instanceof DeliveryUncertainError, String(uncertain));
      assert.ok(uncertain.residue !== undefined && existsSync(uncertain.residue));
      assert.match(uncertain.message, /; its staged file \(created at .*\) was not removed$/);
      assert.equal(existsSync(join(dir, "uncertain")), false);
    });
  });

  it("stages privately at 0600, through a parked write, publication, cancellation and residue", async () => {
    await inDir("rustx-save-private-", async (dir) => {
      const privateMode = (path: string) => lstatSync(path).mode & 0o777;
      // Parked mid-write, before publication: the staged bytes are private.
      const destination = join(dir, "published");
      const writes = parkedWrite(2);
      const saving = saveDelivery(answer, destination, undefined, writes.files);
      await writes.parked;
      assert.equal(writes.files.requested(), 0o600, "created 0600, never widened later");
      assert.equal(privateMode(writes.files.staged()), 0o600);
      writes.release();
      await saving;
      assert.equal(privateMode(destination), 0o600, "the published file is the same private inode");

      // Cancelled after sync: private until removed, and no destination.
      const abort = new AbortController();
      let synced = 0;
      const cancelled = saveFiles({}, { sync: async (sync) => { await sync(); synced = privateMode(cancelled.staged()); abort.abort(); } });
      await assert.rejects(saveDelivery(answer, join(dir, "cancelled"), abort.signal, cancelled), (error) => error === abort.signal.reason);
      assert.equal(synced, 0o600);
      assert.equal(existsSync(join(dir, "cancelled")), false);

      // Cleanup failed: the staged file left behind is still private.
      const kept = await saveDelivery(answer, join(dir, "kept"), undefined, saveFiles({
        link: async () => { throw failure("EACCES"); },
        unlink: async () => { throw failure("EIO"); },
      })).catch((error: unknown) => error);
      assert.ok(kept instanceof DeliveryResidueError, String(kept));
      assert.equal(privateMode(kept.residue), 0o600);
    });
  });

  it("no umask makes staged or saved bytes group- or world-accessible (isolated child per umask)", () => {
    const child = fileURLToPath(new URL("./support/save-under-umask.ts", import.meta.url));
    for (const mask of [0o000, 0o002, 0o022]) {
      const label = `umask ${mask.toString(8).padStart(3, "0")}`;
      const dir = mkdtempSync(join(tmpdir(), "rustx-save-umask-"));
      try {
        const run = spawnSync(process.execPath, [child, mask.toString(8), dir], { encoding: "utf8" });
        assert.equal(run.status, 0, `${label}: ${run.stderr}`);
        const seen = JSON.parse(run.stdout) as {
          control: number;
          published: Record<string, number>;
          cancelled: Record<string, number | boolean>;
          residue: Record<string, number | boolean>;
        };
        assert.equal(seen.control, 0o666 & ~mask, `${label}: the umask was in force for an ordinary creation`);
        assert.deepEqual(seen.published, { midWrite: 0o600, synced: 0o600, beforeLink: 0o600, destination: 0o600 }, label);
        assert.deepEqual(seen.cancelled, { midWrite: 0o600, synced: 0o600, destinationExists: false }, label);
        assert.deepEqual(seen.residue, { midWrite: 0o600, synced: 0o600, kept: 0o600, destinationExists: false }, label);
      } finally {
        rmSync(dir, { recursive: true, force: true });
      }
    }
  });

  it("requests 0600 under an inherited macOS ACL, which Save neither strips nor rewrites", { skip: process.platform !== "darwin" && "native macOS ACLs" }, async () => {
    await inDir("rustx-save-acl-", async (dir) => {
      const me = userInfo().username;
      const shared = join(dir, "shared");
      mkdirSync(shared, { mode: 0o700 });
      // The chosen directory's sharing policy, set with the system tool and
      // no privilege: its new files grant another principal read and write,
      // and this user execute; a 0600 mode grants neither. only_inherit leaves
      // the directory's own access unchanged.
      execFileSync("/bin/chmod", ["+a", "user:nobody allow read,write,file_inherit,only_inherit", shared]);
      execFileSync("/bin/chmod", ["+a", `user:${me} allow execute,file_inherit,only_inherit`, shared]);
      const policy = aclEntries(shared);
      assert.equal(policy.length, 2, policy.join("\n"));
      const inherited = [`user:${me} inherited allow execute`, "user:nobody inherited allow read,write"];
      try {
        const destination = join(shared, "报告 final.md");
        const writes = parkedWrite(2);
        const saving = saveDelivery(answer, destination, undefined, writes.files);
        await writes.parked;
        const staged = writes.files.staged();
        assert.equal(writes.files.requested(), 0o600);
        assert.equal(lstatSync(staged).mode & 0o777, 0o600, "the mode bits are exactly the requested 0600");
        assert.deepEqual(aclEntries(staged).sort(), inherited, "inherited by the create itself, before any byte");
        writes.release();
        assert.deepEqual(await saving, { path: destination });
        assert.deepEqual(readFileSync(destination), BODY_BYTES);
        assert.equal(lstatSync(destination).mode & 0o777, 0o600);
        assert.deepEqual(listing(shared), ["报告 final.md"]);
        // Save added, removed and rewrote no entry, here or on the directory.
        assert.deepEqual(aclEntries(destination).sort(), inherited);
        assert.deepEqual(aclEntries(shared), policy);
        // Effective access is the ACL's and the mode's together. The kernel
        // grants this user execute on the saved 0600 file through the
        // inherited entry, which a 0600 file outside that policy lacks. So the
        // mode alone does not bound access. (Access as `nobody` would need
        // privilege, so the other entry is shown inherited, not exercised.)
        accessSync(destination, constants.X_OK);
        const control = join(dir, "control");
        writeFileSync(control, "", { mode: 0o600 });
        assert.throws(() => accessSync(control, constants.X_OK), { code: "EACCES" });
      } finally {
        for (const path of [shared, ...readdirSync(shared).map((name) => join(shared, name))]) {
          execFileSync("/bin/chmod", ["-N", path]);
        }
      }
    });
  });

  it("reports cleanup as removed only on evidence: an uninspectable destination proves nothing", async () => {
    await inDir("rustx-save-cleanup-evidence-", async (dir) => {
      /** An unlink that fails without removing anything. */
      const unlinkFails = { unlink: async () => { throw failure("EIO"); } };
      /** A link that fails ambiguously without creating anything. */
      const ambiguous = { link: async () => { throw failure("EIO"); } };
      /** Destination inspection refused for `path` only. */
      const uninspectable = (path: string) => ({
        lstat: async (target: string, options: { bigint: true }) => {
          if (target === path) throw failure("EACCES");
          return SAVE_FILES.lstat(target, options);
        },
      });
      const outcome = async (name: string, over: Partial<SaveFiles>, hooks: HandleHooks = {}) => {
        const files = saveFiles(over, hooks);
        const effect = localEffect();
        const result = await saveDelivery(answer, join(dir, name), undefined, files, effect).catch((error: unknown) => error);
        return { result, effect, staged: files.staged(), destination: join(dir, name) };
      };
      /** The staged file is still on disk, linked once, holding the saved bytes. */
      const stillStaged = (staged: string) => {
        assert.equal(lstatSync(staged).nlink, 1);
        assert.deepEqual(readFileSync(staged), BODY_BYTES);
      };

      // Destination names F; unlink succeeds: published, clean.
      const one = await outcome("one", {});
      assert.deepEqual(one.result, { path: one.destination });
      assert.equal(existsSync(one.staged), false);
      assert.equal(lstatSync(one.destination).nlink, 1);
      assert.deepEqual(one.effect, { committed: true, residue: false });

      // Destination names F; unlink fails, F keeps its staged link: published with residue.
      const two = await outcome("two", unlinkFails);
      assert.equal((two.result as { path: string }).path, two.destination);
      assert.deepEqual((two.result as { residue?: unknown }).residue, { path: two.staged, staged: "remains", cause: failure("EIO") });
      assert.equal(identity(two.staged), identity(two.destination));
      assert.equal(lstatSync(two.destination).nlink, 2);
      assert.deepEqual(two.effect, { committed: true, residue: true });

      // Definite refusal; unlink succeeds, F has no links: refused, clean.
      writeFileSync(join(dir, "three"), "kept");
      const three = await outcome("three", {});
      assert.match(String(three.result), /already exists/);
      assert.equal(existsSync(three.staged), false);
      assert.deepEqual(three.effect, { committed: true, residue: false });

      // Definite refusal; unlink fails, F keeps its staged link: refused with residue.
      writeFileSync(join(dir, "four"), "kept");
      const four = await outcome("four", unlinkFails);
      assert.ok(four.result instanceof DeliveryResidueError, String(four.result));
      assert.equal(four.result.staged, "remains");
      assert.match(String(four.result.cause), /already exists/);
      stillStaged(four.staged);
      assert.equal(readFileSync(join(dir, "four"), "utf8"), "kept");

      // Ambiguous link error, destination absent or foreign; unlink fails:
      // uncertain, and the remaining link is attributed to the staged file.
      writeFileSync(join(dir, "six"), "theirs");
      for (const name of ["five", "six"]) {
        const ambiguousThen = await outcome(name, { ...ambiguous, ...unlinkFails });
        assert.ok(ambiguousThen.result instanceof DeliveryUncertainError, String(ambiguousThen.result));
        assert.deepEqual(ambiguousThen.result.observed, { kind: name === "five" ? "absent" : "foreign" });
        assert.equal(ambiguousThen.result.residue, ambiguousThen.staged);
        assert.equal(ambiguousThen.result.staged, "remains");
        assert.match(ambiguousThen.result.message, /; its staged file \(created at .*\) was not removed$/);
        stillStaged(ambiguousThen.staged);
        assert.equal(ambiguousThen.effect.residue, true);
      }

      // The combined failure: link EIO without publishing, destination lstat
      // EACCES, unlink EIO without removing. F, nlink 1, is still on disk; its
      // one link may be an uninspectable destination's, so cleanup is not
      // established, and the outcome says so.
      const seven = await outcome("seven", { ...ambiguous, ...uninspectable(join(dir, "seven")), ...unlinkFails });
      assert.ok(seven.result instanceof DeliveryUncertainError, String(seven.result));
      assert.equal(seven.result.observed.kind, "uninspectable");
      assert.equal(seven.result.residue, seven.staged);
      assert.equal(seven.result.staged, "unknown", "never reported clean without proof");
      assert.match(seven.result.message, /; whether its staged file \(created at .*\) was removed could not be established$/);
      stillStaged(seven.staged);
      assert.equal(existsSync(seven.destination), false);
      assert.deepEqual(seven.effect, { committed: true, residue: true }, "owed after /files retires");

      // Uninspectable destination; unlink succeeds, F has no links: established.
      const eight = await outcome("eight", { ...ambiguous, ...uninspectable(join(dir, "eight")) });
      assert.ok(eight.result instanceof DeliveryUncertainError, String(eight.result));
      assert.equal(eight.result.residue, undefined);
      assert.equal(eight.result.staged, undefined);
      assert.equal(existsSync(eight.staged), false);
      assert.deepEqual(eight.effect, { committed: true, residue: false });

      // Uninspectable destination; the link count cannot be read after the
      // unlink: cleanup is unknown, whatever the unlink did.
      const nine = await outcome("nine", { ...ambiguous, ...uninspectable(join(dir, "nine")) }, {
        stat: async (call, stat) => { if (call === 2) throw failure("EIO"); return stat(); },
      });
      assert.ok(nine.result instanceof DeliveryUncertainError, String(nine.result));
      assert.equal(nine.result.staged, "unknown");
      assert.equal(nine.result.residue, nine.staged);
      assert.equal(nine.effect.residue, true);

      // A link count that cannot be read after a verified publication is not
      // proof of removal either.
      const ten = await outcome("ten", {}, {
        stat: async (call, stat) => { if (call === 2) throw failure("EIO"); return stat(); },
      });
      assert.deepEqual((ten.result as { residue?: unknown }).residue, { path: ten.staged, staged: "unknown", cause: failure("EIO") });
      assert.deepEqual(readFileSync(ten.destination), BODY_BYTES);
      // The staged name replaced after publication is case 6 above: only
      // facts this save's own handle shows are reported.
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

  it("claims only what rustX can prove: an opener accepted or rejected a pathname", async () => {
    await inDir("rustx-open-contract-", async (dir) => {
      const path = join(dir, "报告 final.md");
      writeFileSync(path, "delivered");
      const leaf = lstatSync(path, { bigint: true });
      const location = { file: REPORT, path, device: leaf.dev.toString(), inode: leaf.ino.toString() };
      const opened: string[] = [];
      const options = (launch: (command: string, target: string) => Promise<number | null>) => {
        const effect = localEffect();
        return { effect, options: { sharedHost: true, opener: "xdg-open", effect, launch } };
      };

      // Verified local file: the opener is asked, with argv only.
      const verified = options(async (_command, target) => { opened.push(target); return 0; });
      assert.deepEqual(await openDelivery(async () => location, verified.options), { path, opener: "xdg-open" });
      assert.equal(verified.effect.committed, true);

      // Replaced or missing locally, a symlink to it, a directory: nothing is
      // launched and nothing is committed.
      const other = join(dir, "other.md");
      writeFileSync(other, "other");
      symlinkSync(path, join(dir, "symlink.md"));
      mkdirSync(join(dir, "directory.md"));
      for (const target of [
        { ...location, path: other },
        { ...location, path: join(dir, "missing.md") },
        { ...location, path: join(dir, "symlink.md") },
        { ...location, path: join(dir, "directory.md") },
      ]) {
        const refused = options(async () => assert.fail("never launched"));
        await assert.rejects(openDelivery(async () => target, refused.options), /not the delivered file/, target.path);
        assert.equal(refused.effect.committed, false);
      }

      // Replaced after verification, before the opener resolves the path:
      // the opener opens whatever the path names then. rustX reports the
      // accepted request and nothing about which file it showed.
      writeFileSync(join(dir, "swap"), "replacement");
      const raced = options(async (_command, target) => {
        renameSync(join(dir, "swap"), target);
        opened.push(readFileSync(target, "utf8"));
        return 0;
      });
      const requested = await openDelivery(async () => location, raced.options);
      assert.deepEqual(Object.keys(requested).sort(), ["opener", "path"], "no identity is claimed for what was opened");
      assert.equal(opened.at(-1), "replacement", "the limit is real: the opener resolves the pathname itself");

      // Rejected by the opener, or the opener could not start: committed, and
      // reported as the opener's answer.
      writeFileSync(path, "delivered");
      const fresh = lstatSync(path, { bigint: true });
      const current = { ...location, device: fresh.dev.toString(), inode: fresh.ino.toString() };
      const rejected = options(async () => 4);
      await assert.rejects(openDelivery(async () => current, rejected.options), /xdg-open did not accept the open request \(exit 4\)/);
      assert.equal(rejected.effect.committed, true);
      const missing = localEffect();
      await assert.rejects(openDelivery(async () => current, {
        sharedHost: true, opener: "rustx-no-such-opener", effect: missing,
      }), /rustx-no-such-opener could not take the open request \(ENOENT\)/);
      assert.equal(missing.committed, true, "the launch was dispatched");
    });
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
