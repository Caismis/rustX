/**
 * Build-once browser inputs for sharded CI acceptance.
 *
 * `pack` (preparation job) records the compiled inputs every browser shard
 * consumes — the real Rust executables, the Artifact fixture and the
 * production Web bundle — with the identity they were built under and a
 * checksum per file. `install` (each shard) refuses an archive whose identity
 * differs from the shard's own checkout and runner, or whose files are
 * missing, extra or altered, and never overwrites an existing build output.
 * `check` (browser-tests.sh) re-verifies the installed files before Playwright
 * starts, so a prepared run never uses an output merely because it exists.
 *
 * Node native addons are not packaged: `pnpm install` rebuilds them from the
 * lockfile in every job (the package's own install script), so only the Node
 * ABI they were built against is part of the identity.
 */
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, copyFileSync, existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { arch, platform } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repository = fileURLToPath(new URL('../..', import.meta.url));
export const MANIFEST = 'browser-prepared.json';
/** Installed copy of the manifest that browser-tests.sh re-verifies. */
export const INSTALLED_MANIFEST = 'target/browser-prepared.json';
/** The exact Cargo invocation whose outputs are packaged. */
export const CARGO_BUILD = ['build', '--bins', '--example', 'document_artifact_fixture', '--locked'];
/** Executables of that build that the fixtures, Product Host and App Server exec. */
export const EXECUTABLES = ['target/debug/rustx', 'target/debug/bash-supervisor', 'target/debug/interactive-supervisor', 'target/debug/examples/document_artifact_fixture'];
const BUNDLE = 'web-console/dist';

export interface Identity {
  commit: string;
  platform: string;
  arch: string;
  rust: string;
  cargo: { profile: 'dev'; command: string; features: 'default'; debuginfo: 'stripped' };
  /** Built with no override of a build-affecting setting (BUILD_OVERRIDES). */
  build: 'repository-defaults';
  node: { version: string; abi: string };
  pnpm: string;
  playwright: string;
  /** Checksums of the sources that define dependencies, features and profiles. */
  sources: Record<string, string>;
}
/**
 * Settings that change what Cargo, rustc, the C toolchain or Vite produce.
 * The preparation build refuses to run under any of them, so its identity's
 * `build: 'repository-defaults'` means exactly the checked-in configuration.
 * Parallelism (CARGO_BUILD_JOBS) and incremental caching (CARGO_INCREMENTAL,
 * which rust-cache sets) change no artifact semantics and stay allowed.
 */
export const BUILD_OVERRIDES = /^(RUSTFLAGS|RUSTDOCFLAGS|RUSTC|RUSTC_WRAPPER|RUSTC_WORKSPACE_WRAPPER|RUSTC_BOOTSTRAP|CARGO_ENCODED_RUSTFLAGS|CARGO_BUILD_(?!JOBS$).+|CARGO_PROFILE_.+|CARGO_TARGET_.+|CARGO_UNSTABLE_.+|CARGO_FEATURES|CC|CXX|AR|CFLAGS|CXXFLAGS|CPPFLAGS|LDFLAGS|NODE_ENV|NODE_OPTIONS|VITE_.+)$/;
/** The build-affecting overrides set in `environment`, by name. */
export function buildOverrides(environment: NodeJS.ProcessEnv = process.env) {
  return Object.keys(environment).filter(name => environment[name] !== undefined && BUILD_OVERRIDES.test(name)).sort();
}

/** Recorded for diagnosis, never compared: runners differ in incidental setup. */
export interface Provenance { rustc: string; environment: Record<string, string> }
export interface FileRecord { sha256: string; size: number; executable: boolean }
export interface Manifest { schema: 1; identity: Identity; provenance: Provenance; files: Record<string, FileRecord> }

function sha256(path: string) { return createHash('sha256').update(readFileSync(path)).digest('hex'); }
function run(command: string, args: string[], cwd = repository) { return execFileSync(command, args, { cwd, encoding: 'utf8' }).trim(); }

/** The Rust release the checked-in toolchain file pins, e.g. `1.98.1`. */
export function pinnedRust(root = repository) {
  const channel = /^channel\s*=\s*"([^"]+)"/m.exec(readFileSync(join(root, 'rust-toolchain.toml'), 'utf8'))?.[1];
  if (!channel) throw new Error('rust-toolchain.toml pins no channel');
  return channel;
}
/** Build-relevant environment: what Cargo and rustc read beyond the manifests. */
export function buildEnvironment(environment: NodeJS.ProcessEnv = process.env) {
  return Object.fromEntries(Object.entries(environment).filter(([name]) => /^(CARGO_|RUST|CC$|CFLAGS$|CXX$|CXXFLAGS$|LDFLAGS$)/.test(name) && !/^(CARGO_HOME|RUSTUP_HOME|CARGO_TERM_|RUSTUP_TOOLCHAIN)/.test(name)).sort(([a], [b]) => a.localeCompare(b)).map(([name, value]) => [name, value ?? '']));
}

/**
 * The identity a run on this checkout and runner requires. It never asks a
 * Rust toolchain: the pin comes from source, and the preparation job proves
 * its compiler matched that pin before packing.
 */
export function expectedIdentity(root = repository): Identity {
  const web = join(root, 'web-console');
  return {
    commit: run('git', ['rev-parse', 'HEAD'], root),
    platform: platform(),
    arch: arch(),
    rust: pinnedRust(root),
    cargo: { profile: 'dev', command: `cargo ${CARGO_BUILD.join(' ')}`, features: 'default', debuginfo: 'stripped' },
    build: 'repository-defaults',
    node: { version: process.version, abi: process.versions.modules },
    pnpm: run('pnpm', ['--version'], web),
    playwright: JSON.parse(readFileSync(join(web, 'node_modules/@playwright/test/package.json'), 'utf8')).version,
    // Cargo configuration the repository may carry is part of the build.
    sources: Object.fromEntries(['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.cargo/config.toml', 'web-console/package.json', 'web-console/pnpm-lock.yaml', 'web-console/vite.config.ts']
      .map(file => [file, existsSync(join(root, file)) ? sha256(join(root, file)) : 'absent'])),
  };
}

/** Differences between a manifest's identity and the required one, by field. */
export function identityErrors(actual: Identity, expected: Identity): string[] {
  const errors: string[] = [];
  const compare = (path: string, a: unknown, b: unknown) => {
    if (typeof b === 'object' && b !== null) {
      const keys = new Set([...Object.keys(b), ...Object.keys((a ?? {}) as object)]);
      for (const key of keys) compare(`${path}.${key}`, (a as Record<string, unknown> | undefined)?.[key], (b as Record<string, unknown>)[key]);
    } else if (a !== b) errors.push(`${path}: prepared ${JSON.stringify(a)}, required ${JSON.stringify(b)}`);
  };
  compare('identity', actual, expected);
  return errors;
}

function walk(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => entry.isDirectory() ? walk(join(directory, entry.name)) : [join(directory, entry.name)]);
}
/** The repository-relative files a prepared run consumes. */
export function packagedFiles(root = repository) {
  const bundle = join(root, BUNDLE);
  if (!existsSync(join(bundle, 'index.html'))) throw new Error(`${BUNDLE} holds no production build`);
  return [...EXECUTABLES, ...walk(bundle).map(file => relative(root, file))].sort();
}
export function record(path: string): FileRecord {
  const stat = statSync(path);
  return { sha256: sha256(path), size: stat.size, executable: (stat.mode & 0o111) !== 0 };
}

/** Files missing, extra, or differing from the manifest under `root`. */
export function fileErrors(manifest: Manifest, root: string): string[] {
  const errors: string[] = [];
  const required = Object.keys(manifest.files);
  for (const executable of EXECUTABLES) if (!manifest.files[executable]?.executable) errors.push(`${executable}: not a prepared executable`);
  if (!required.some(file => file === `${BUNDLE}/index.html`)) errors.push(`${BUNDLE}/index.html: no production build`);
  const bundle = join(root, BUNDLE);
  if (existsSync(bundle)) for (const file of walk(bundle).map(path => relative(root, path))) if (!manifest.files[file]) errors.push(`${file}: not part of the prepared bundle`);
  for (const file of required) {
    if (file.startsWith('/') || file.split('/').includes('..')) { errors.push(`${file}: escapes the repository`); continue; }
    const path = join(root, file);
    if (!existsSync(path) || !lstatSync(path).isFile()) { errors.push(`${file}: missing`); continue; }
    const actual = record(path), expected = manifest.files[file];
    if (actual.sha256 !== expected.sha256 || actual.size !== expected.size) errors.push(`${file}: content differs from the prepared checksum`);
    if (actual.executable !== expected.executable) errors.push(`${file}: executable mode differs`);
  }
  return errors;
}

/**
 * Preparation: run the recorded builds, verify the compiler matched the pin,
 * then stage every input with its checksum.
 */
export function pack(stage: string, root = repository) {
  const overrides = buildOverrides();
  if (overrides.length) throw new Error(`Browser inputs build only with repository defaults; unset ${overrides.join(', ')}`);
  execFileSync('cargo', CARGO_BUILD, { cwd: root, stdio: 'inherit' });
  // The one production build: Vite plus the artifact provenance check.
  execFileSync('pnpm', ['build'], { cwd: join(root, 'web-console'), stdio: 'inherit' });
  const rustc = run('rustc', ['-vV'], root);
  const release = /^release: (.+)$/m.exec(rustc)?.[1];
  if (release !== pinnedRust(root)) throw new Error(`Built with rustc ${release}, but rust-toolchain.toml pins ${pinnedRust(root)}`);
  const bins = (JSON.parse(run('cargo', ['metadata', '--no-deps', '--format-version', '1'], root)).packages as { targets: { kind: string[]; name: string }[] }[])
    .flatMap(item => item.targets).filter(target => target.kind.includes('bin')).map(target => `target/debug/${target.name}`).sort();
  const listed = EXECUTABLES.filter(file => !file.includes('/examples/')).sort();
  if (JSON.stringify(bins) !== JSON.stringify(listed)) throw new Error(`Packaged executables ${listed.join(', ')} differ from Cargo binaries ${bins.join(', ')}`);
  mkdirSync(stage, { recursive: true });
  const files: Record<string, FileRecord> = {};
  for (const file of packagedFiles(root)) {
    const target = join(stage, file);
    mkdirSync(dirname(target), { recursive: true });
    if (EXECUTABLES.includes(file)) {
      // Debug info is most of a debug executable and never read by a test;
      // symbols stay, so panics and backtraces keep their names.
      execFileSync('strip', ['--strip-debug', '-o', target, join(root, file)]);
      chmodSync(target, 0o755);
    } else copyFileSync(join(root, file), target);
    files[file] = record(target);
  }
  const manifest: Manifest = { schema: 1, identity: expectedIdentity(root), provenance: { rustc, environment: buildEnvironment() }, files };
  writeFileSync(join(stage, MANIFEST), `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

/** Shard: verify the staged archive against this checkout, then install it. */
export function install(stage: string, root = repository, expected = expectedIdentity(root)) {
  const path = join(stage, MANIFEST);
  if (!existsSync(path)) throw new Error(`No prepared browser manifest at ${path}`);
  const manifest = JSON.parse(readFileSync(path, 'utf8')) as Manifest;
  if (manifest.schema !== 1) throw new Error(`Unsupported prepared manifest schema ${String(manifest.schema)}`);
  const errors = [...identityErrors(manifest.identity, expected), ...fileErrors(manifest, stage)];
  for (const file of Object.keys(manifest.files)) if (existsSync(join(root, file))) errors.push(`${file}: a build output already exists; a prepared run installs only into a clean checkout`);
  if (errors.length) throw new Error(`Refusing prepared browser inputs:\n${errors.join('\n')}`);
  for (const file of Object.keys(manifest.files)) {
    mkdirSync(dirname(join(root, file)), { recursive: true });
    copyFileSync(join(stage, file), join(root, file));
    if (manifest.files[file].executable) chmodSync(join(root, file), 0o755);
  }
  mkdirSync(dirname(join(root, INSTALLED_MANIFEST)), { recursive: true });
  writeFileSync(join(root, INSTALLED_MANIFEST), readFileSync(path));
  return manifest;
}

/** browser-tests.sh: the installed inputs still match this checkout exactly. */
export function check(root = repository, expected = expectedIdentity(root)) {
  const path = join(root, INSTALLED_MANIFEST);
  if (!existsSync(path)) throw new Error(`RUSTX_BROWSER_PREPARED is set but ${INSTALLED_MANIFEST} is absent; install the prepared inputs first`);
  const manifest = JSON.parse(readFileSync(path, 'utf8')) as Manifest;
  const errors = [...identityErrors(manifest.identity, expected), ...fileErrors(manifest, root)];
  if (errors.length) throw new Error(`Installed browser inputs differ from their preparation:\n${errors.join('\n')}`);
  return manifest;
}

if (import.meta.main) {
  const [command, stage] = process.argv.slice(2);
  const manifest = command === 'pack' && stage ? pack(resolve(stage))
    : command === 'install' && stage ? install(resolve(stage))
      : command === 'check' ? check()
        : (() => { throw new Error('usage: browser-artifact.ts pack <stage> | install <stage> | check'); })();
  console.log(`${command}: ${Object.keys(manifest.files).length} prepared files for ${manifest.identity.commit} (${manifest.identity.platform}/${manifest.identity.arch}, Rust ${manifest.identity.rust}, Node ABI ${manifest.identity.node.abi})`);
}
