import { spawn } from 'node:child_process';
import { closeSync, constants, fstatSync } from 'node:fs';
import { mkdtemp, open, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { isAbsolute, join } from 'node:path';
import { FILE_PREVIEW_LIMITS, PreviewCancelled, type FilePreviewOperation, type NativeStepPreviewTiming } from '../shared/file-preview.ts';
import { WorkspaceHostError } from '../src/workspaces/host.ts';
import { descriptorVersion, openPreviewPath, openWorkspaceBytePreview } from './workspace-preview.ts';

export interface StepPreviewConverterConfig {
  executable: string;
  threads: 1 | 8;
}

async function convert(config: StepPreviewConverterConfig, fd: number, cwd: string, signal: AbortSignal) {
  signal.throwIfAborted();
  return new Promise<void>((resolve, reject) => {
    const child = spawn(config.executable, [String(config.threads)], {
      cwd, stdio: ['ignore', 'ignore', 'ignore', fd],
      env: { LANG: 'C', LD_LIBRARY_PATH: process.env.LD_LIBRARY_PATH },
    });
    const cancel = () => child.kill('SIGKILL');
    signal.addEventListener('abort', cancel, { once: true });
    if (signal.aborted) cancel();
    let failed = false;
    child.on('error', () => { failed = true; });
    child.once('close', (code, exitSignal) => {
      signal.removeEventListener('abort', cancel);
      if (signal.aborted) reject(signal.reason);
      else if (failed) reject(new Error('native_step_unavailable'));
      else if (code === 3) reject(new Error('native_step_external'));
      else if (exitSignal === 'SIGXFSZ') reject(new Error('too_large'));
      else if (code !== 0) reject(new Error('parser_failure'));
      else resolve();
    });
  });
}

async function readTiming(directory: string, processMs: number, threads: 1 | 8): Promise<NativeStepPreviewTiming> {
  const fd = await open(join(directory, 'timing.json'), constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    if (!(await fd.stat()).isFile()) throw new Error('parser_failure');
    const buffer = Buffer.alloc(4096);
    const { bytesRead } = await fd.read(buffer, 0, buffer.length, 0);
    if (!bytesRead || bytesRead === buffer.length) throw new Error('parser_failure');
    const report = JSON.parse(buffer.toString('utf8', 0, bytesRead));
    const { readMs, parseMs, transferMs, meshMs, encodeMs, roots, peakRssKiB } = report;
    if (report.occt !== '7.6.1' || report.threads !== threads
      || ![readMs, parseMs, transferMs, meshMs, encodeMs].every(value => Number.isFinite(value) && value >= 0)
      || ![roots, peakRssKiB].every(value => Number.isSafeInteger(value) && value > 0)) throw new Error('parser_failure');
    return { threads, readMs, parseMs, transferMs, meshMs, encodeMs, processMs, roots, peakRssKiB };
  } finally { await fd.close(); }
}

/** The existing preview owner holds the source and the temporary output.
 * Conversion admission remains held until the converter has physically exited. */
export class NativeStepPreview {
  private converting = false;
  private readonly config: StepPreviewConverterConfig;
  private readonly timeoutMs: number;

  constructor(config: StepPreviewConverterConfig, timeoutMs = 55_000) {
    if (process.platform !== 'linux' || typeof config.executable !== 'string' || !isAbsolute(config.executable)
      || ![1, 8].includes(config.threads)) throw new Error('Invalid Linux native STEP converter configuration');
    this.config = config;
    this.timeoutMs = timeoutMs;
  }

  async open(root: string, path: string, reauthorize: () => Promise<void>, signal: AbortSignal): Promise<FilePreviewOperation> {
    signal.throwIfAborted();
    if (this.converting) throw new WorkspaceHostError('Native STEP conversion capacity reached', 'capacity');
    this.converting = true;
    const timeout = new AbortController(), deadline = setTimeout(() => timeout.abort(new Error('parser_timeout')), this.timeoutMs);
    deadline.unref();
    let dir: string | undefined, fd: number | undefined, output: FilePreviewOperation | undefined, adopted = false;
    const closeSource = () => {
      if (fd === undefined) return;
      try { closeSync(fd); }
      catch { throw new WorkspaceHostError('Native STEP source descriptor did not retire', 'file_settlement_unknown'); }
    };
    const retireAsset = async () => {
      try { await output?.close(); }
      finally {
        if (dir) await rm(dir, { recursive: true, force: true }).catch(() => {
          throw new WorkspaceHostError('Native STEP temporary asset did not retire', 'file_settlement_unknown');
        });
      }
    };
    try {
      const active = AbortSignal.any([signal, timeout.signal]);
      fd = openPreviewPath(root, path);
      const source = fd, stat = fstatSync(source, { bigint: true }), version = descriptorVersion(source, stat);
      if (!stat.isFile() || stat.size === 0n) throw new WorkspaceHostError('source_unavailable', 'source_unavailable');
      if (stat.size > BigInt(FILE_PREVIEW_LIMITS.geometryBytes)) throw new Error('too_large');
      const verify = async () => {
        signal.throwIfAborted();
        await reauthorize(); signal.throwIfAborted();
        const current = openPreviewPath(root, path);
        try {
          if (descriptorVersion(source) !== version || descriptorVersion(current) !== version)
            throw new WorkspaceHostError('source_changed', 'source_changed');
        } finally { closeSync(current); }
      };
      dir = await mkdtemp(join(tmpdir(), 'rustx-step-preview-'));
      const start = performance.now();
      await convert(this.config, source, dir, active);
      const nativeStep = await readTiming(dir, performance.now() - start, this.config.threads).catch(() => { throw new Error('parser_failure'); });
      const asset = openWorkspaceBytePreview(dir, 'preview.glb', signal);
      output = asset;
      if (asset.bytes < 20) throw new Error('parser_failure');
      if (asset.bytes > FILE_PREVIEW_LIMITS.nativeStepGlbBytes) throw new Error('too_large');
      await verify(); active.throwIfAborted();
      let closing: Promise<void> | undefined;
      adopted = true;
      return {
        version, bytes: asset.bytes, nativeStep,
        read: async (range, active) => {
          await verify();
          const page = await asset.read(range, active);
          await verify();
          return { ...page, version };
        },
        close: () => closing ??= (async () => {
          try {
            await retireAsset();
            if (signal.aborted) throw new PreviewCancelled();
            // Successful settlement witnesses the original source after all
            // asynchronous retirement. Keep its descriptor until this check.
            await verify();
          } finally { closeSource(); }
        })(),
      };
    } finally {
      clearTimeout(deadline);
      try {
        if (!adopted) {
          try { await retireAsset().catch(error => { if (!(error instanceof PreviewCancelled)) throw error; }); }
          finally { closeSource(); }
        }
      } finally { this.converting = false; }
    }
  }
}
