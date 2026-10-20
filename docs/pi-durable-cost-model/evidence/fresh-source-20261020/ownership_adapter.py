"""Opt-in ownership mapping and outer safety controls; no workload implementation."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import threading
import time

GIB = 2**30
HEAD = '1dfdf7e53bf20e1869099578b8f41c434e275961'
TREE = '6b4e4e4b47592ab9369c364574331daac923ca4e'
BASE = '623b41b869a95c888eb04f907cbf607232699a2f'
BASE_TREE = '19ff7d6bd64993a6b9fc08bbcd687787dacfe08e'
SOURCES = {'baseline': (BASE, BASE_TREE), 'ownership': (HEAD, TREE)}
COHORT_ENTRY = 16 * GIB  # Preserve the existing wrapper's reserve.
BUILD_ENTRY = COHORT_ENTRY + 2 * GIB


def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def validate_candidate(identity, preserved, baseline):
    if identity.get('head') != HEAD or identity.get('production_tree') != TREE:
        raise ValueError('ownership candidate must be the exact reviewed source tree')
    if identity.get('verified') is not True or preserved.get('source_head') != HEAD:
        raise ValueError('missing verified candidate build provenance')
    if baseline['head'] != BASE or identity['harness'] != baseline['harness']:
        raise ValueError('baseline or fixture identity differs')
    for name, kind, test in [('rustx', ['lib'], False), ('conformance', ['test'], True)]:
        roots = [item for item in identity['root_artifacts'] if item['name'] == name and item['kind'] == kind]
        if len(roots) != 1 or roots[0]['fresh'] is not False:
            raise ValueError('ordinary library and conformance must be freshly compiled')
        items = [item for item in preserved['artifacts'] if item['name'] == name and item['kind'] == kind]
        if not items or not any(item['preserved'].endswith('.rlib') for item in items) and kind == ['lib']:
            raise ValueError('missing preserved ordinary product artifact')
        for item in items:
            profile = item['profile']
            if (str(profile['opt_level']), profile['debuginfo'], profile['test'], item['features']) != ('3', 0, test, []):
                raise ValueError('unit/O0/instrumented artifacts cannot be performance products')
            if profile.get('debug_assertions') is not False or profile.get('overflow_checks') is not False:
                raise ValueError('candidate profile differs from the preserved release baseline')
        if kind == ['lib']:
            library = next(item for item in items if item['preserved'].endswith('.rlib'))
            if sha(library['preserved']) != library['sha256'] or library['sha256'] != identity['library_sha256']:
                raise ValueError('ordinary library digest mismatch')
    if sha(identity['binary']) != identity['sha256']:
        raise ValueError('candidate executable digest mismatch')
    products = [item for item in preserved['artifacts'] if item['kind'] == ['test'] and item['name'] == 'conformance']
    if not any(Path(item['preserved']).resolve() == Path(identity['binary']).resolve()
               and item['sha256'] == identity['sha256'] for item in products):
        raise ValueError('conformance identity is not the preserved executable')


LOCKED_INPUTS = {
    'Cargo.toml': '59162af233790d4c4c0340046f6fc91c18bac5cfb8d416ca6ba4a109a6e998e4',
    'Cargo.lock': '5a88074d3762bdf268b191882358bd58d48805a9c079806897dbcdc4a920da51',
    'rust-toolchain.toml': 'c910997cb152c6dc8ed13b1fdf9fa28a4ed30d6776123e27dd6170e5c66067a0'}
PINNED_ENV = {'RUSTFLAGS': '-Copt-level=3 -Cdebuginfo=0', 'CARGO_PROFILE_RELEASE_OPT_LEVEL': '3',
              'CARGO_PROFILE_RELEASE_DEBUG': '0', 'CARGO_BUILD_JOBS': '2', 'CARGO_INCREMENTAL': '0'}


def validate_build_inputs(identity):
    if any(identity['source_sha256'].get(k) != v for k, v in LOCKED_INPUTS.items()):
        raise ValueError('locked build inputs differ')
    if any(identity['build_environment'].get(k) != v for k, v in PINNED_ENV.items()):
        raise ValueError('build profile environment differs')
    if ('rustc 1.98.1 (48a229cea 2026-09-01)' not in identity['rustc'] or
        'commit-hash: 48a229ceaefd4985c50990b14116b6d856af0985' not in identity['rustc'] or
        'host: x86_64-unknown-linux-gnu' not in identity['rustc'] or
        'LLVM version: 22.1.8' not in identity['rustc'] or
        identity['cargo'].strip() != 'cargo 1.98.1 (797e8a9bc 2026-08-05)'):
        raise ValueError('toolchain differs from pinned provenance')


def validate_product(identity, preserved, label):
    """Validate either fresh-host product, including every helper mapping."""
    head, tree = SOURCES[label]
    validate_build_inputs(identity)
    if (identity.get('head'), identity.get('production_tree')) != (head, tree):
        raise ValueError('wrong pinned build source')
    if identity.get('verified') is not True or preserved.get('source_head') != head:
        raise ValueError('unverified product')
    repo = Path(identity['repo']).resolve()
    target = Path(identity['build_environment']['CARGO_TARGET_DIR']).resolve()
    required = [('rustx', ['lib'], False, 'src/lib.rs'),
                ('conformance', ['test'], True, 'tests/conformance/main.rs'),
                ('rustx', ['bin'], False, 'src/main.rs'),
                ('bash-supervisor', ['bin'], False, 'src/bin/bash_supervisor.rs'),
                ('interactive-supervisor', ['bin'], False, 'src/bin/interactive_supervisor.rs')]
    for name, kind, test, source in required:
        roots = [r for r in identity['root_artifacts'] if r['name'] == name and r['kind'] == kind]
        if len(roots) != 1 or roots[0]['fresh'] is not False or Path(roots[0]['source_path']).resolve() != repo / source:
            raise ValueError('missing, cached or foreign root product/helper')
        items = [r for r in preserved['artifacts'] if r['name'] == name and r['kind'] == kind]
        if not items or (kind == ['lib'] and not any(r['preserved'].endswith('.rlib') for r in items)):
            raise ValueError('missing product/helper artifact')
        for item in items:
            profile = item['profile']
            if (str(profile['opt_level']), profile['debuginfo'], profile['test'], item['features'],
                profile.get('debug_assertions'), profile.get('overflow_checks')) != ('3', 0, test, [], False, False):
                raise ValueError('wrong ordinary optimized artifact profile')
            if kind == ['bin'] and Path(item['target']).resolve() != target / 'release' / name:
                raise ValueError('wrong embedded helper destination')
    if 'Linker: LLD 22.1.8' not in identity['linker_comment']:
        raise ValueError('unexpected final ELF linker')
    shared = {r['target']: r['sha256'] for r in preserved['artifacts'] if r['kind'] == ['shared-library']}
    if shared != identity['shared_libraries']:
        raise ValueError('shared-library inventory differs')
    for item in preserved['artifacts']:
        if not Path(item['target']).resolve().is_relative_to(target):
            raise ValueError('artifact target outside declared target directory')
        if sha(item['preserved']) != item['sha256']:
            raise ValueError('preserved artifact digest mismatch')
    library = next(r for r in preserved['artifacts'] if r['kind'] == ['lib'] and r['preserved'].endswith('.rlib'))
    if library['sha256'] != identity['library_sha256']:
        raise ValueError('ordinary library identity mismatch')
    products = [r for r in preserved['artifacts'] if r['name'] == 'conformance' and r['kind'] == ['test']]
    if not any(Path(r['preserved']).resolve() == Path(identity['binary']).resolve() and r['sha256'] == identity['sha256'] for r in products):
        raise ValueError('conformance identity mismatch')
    if sha(identity['binary']) != identity['sha256']:
        raise ValueError('conformance digest mismatch')


def validate_pair(baseline, candidate):
    if baseline['head'] != BASE or candidate['head'] != HEAD:
        raise ValueError('wrong baseline/candidate pair')
    for key in ('harness', 'rustc', 'cargo'):
        if baseline[key] != candidate[key]:
            raise ValueError('unmatched build provenance: ' + key)
    for key in ('RUSTFLAGS', 'CARGO_PROFILE_RELEASE_OPT_LEVEL', 'CARGO_PROFILE_RELEASE_DEBUG',
                'CARGO_BUILD_JOBS', 'CARGO_INCREMENTAL', 'RUSTC', 'LIBCLANG_PATH', 'BINDGEN_EXTRA_CLANG_ARGS'):
        if baseline['build_environment'][key] != candidate['build_environment'][key]:
            raise ValueError('unmatched build environment: ' + key)


def allocated(paths):
    """Count actual allocated regular-file blocks once, without following symlinks."""
    seen = set()
    total = 0
    for path in paths:
        if not path.exists():
            continue
        for directory, _, names in os.walk(path, followlinks=False):
            for name in names:
                file = Path(directory) / name
                try:
                    stat = file.lstat()
                except FileNotFoundError:
                    continue
                key = (stat.st_dev, stat.st_ino)
                if file.is_symlink() or key in seen:
                    continue
                seen.add(key)
                total += stat.st_blocks * 512
    return total


def process_start(pid):
    try:
        return int(Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19])
    except FileNotFoundError:
        return None


def resources(workspace):
    group = Path('/sys/fs/cgroup')
    stats = dict(line.split() for line in (group / 'memory.stat').read_text().splitlines())
    events = dict(line.split() for line in (group / 'memory.events').read_text().splitlines())
    pressure = dict(word.split('=') for line in (group / 'memory.pressure').read_text().splitlines()
                    if line.startswith('full ') for word in line.split()[1:])
    disk = os.statvfs(workspace)
    return {'monotonic': time.monotonic(), 'free': disk.f_bavail * disk.f_frsize,
            'memory_limit': int((group / 'memory.max').read_text()),
            'memory_current': int((group / 'memory.current').read_text()),
            'anon_shmem': int(stats['anon']) + int(stats['shmem']),
            'full_avg10': float(pressure['avg10']),
            'oom': int(events['oom']), 'oom_kill': int(events['oom_kill'])}


class Guard:
    """Monitor one owned build/cohort. Existing workload clocks stay untouched."""
    def __init__(self, workspace, output, kind, evidence=None):
        if kind not in ('build', 'cohort'):
            raise ValueError('unknown guard scope')
        self.workspace, self.output, self.kind = Path(workspace), Path(output), kind
        self.initial = resources(self.workspace)
        if self.initial['free'] < (BUILD_ENTRY if kind == 'build' else COHORT_ENTRY):
            raise RuntimeError('insufficient combined build/cohort reserve; no child started')
        self.started = time.monotonic()
        self.deadline = self.started + (3600 if kind == 'build' else 1800)
        self.product_deadline = None
        self.paths = [self.output, self.workspace / 'tmp']
        self.evidence = Path(evidence) if evidence is not None else self.output
        self.paths.append(self.evidence)
        if kind == 'build':
            self.paths.append(self.workspace / 'target')
        self.initial_allocated = allocated(self.paths)
        self.initial_temp = allocated([self.workspace / 'tmp'])
        self.failure = None
        self.diagnostic_errors = []
        self.owned = []
        self.pressure_since = self.memory_since = None
        self.done = threading.Event()
        self.lock = threading.Lock()
        self.thread = threading.Thread(target=self._monitor, daemon=True)

    def start(self):
        self.output.mkdir(parents=True, exist_ok=False)
        (self.output / 'watchdog-config.json').write_text(json.dumps({
            'kind': self.kind, 'adapter_sha256': sha(__file__), 'initial': self.initial,
            'evidence_directory': str(self.evidence), 'monitored_allocated_paths': [str(path) for path in self.paths],
            'entry_bytes': BUILD_ENTRY if self.kind == 'build' else COHORT_ENTRY,
            'hard_floor_bytes': 5 * GIB, 'soft_stop_bytes': BUILD_ENTRY if self.kind == 'build' else 6 * GIB,
            'protected_cohort_reserve_bytes': COHORT_ENTRY,
            'allocated_growth_cap_bytes': 2 * GIB if self.kind == 'build' else GIB,
            'evidence_cap_bytes': int(.75 * GIB), 'temporary_growth_cap_bytes': int(.25 * GIB),
            'deadline_monotonic': self.deadline, 'per_product_seconds': 180,
            'memory_stop': 'anon+shmem >= limit-2GiB for 3s; memory full avg10 >=10% for 5s; any new oom/oom_kill event',
            'sampling_seconds': .5, 'scope': 'external watchdog; overhead not estimated'}, indent=2) + '\n')
        self.thread.start()
        return self

    def register(self, process):
        try:
            group = os.getpgid(process.pid)
        except ProcessLookupError:
            group = None
        with self.lock:
            self.owned.append((process, process_start(process.pid), group))
        if self.failure:
            self._shutdown()

    def check(self):
        if self.failure:
            raise RuntimeError('owned operation stopped: ' + self.failure['reason'])

    def product(self, active):
        if active:
            self.check()
        self.product_deadline = time.monotonic() + 180 if active else None

    def reason(self, point, growth, evidence=0, temporary=0):
        now = point['monotonic']
        if self.kind == 'build' and point['free'] <= BUILD_ENTRY:
            return 'build-protected-cohort-reserve-18GiB'
        if point['free'] <= 6 * GIB:
            return 'disk-soft-stop-6GiB'
        if growth > (2 * GIB if self.kind == 'build' else GIB):
            return 'allocated-disk-budget'
        if self.kind == 'cohort' and (evidence > .75 * GIB or temporary > .25 * GIB):
            return 'evidence-or-temporary-allocation-budget'
        if now >= self.deadline:
            return 'build-60m-timeout' if self.kind == 'build' else 'cohort-30m-timeout'
        if self.product_deadline is not None and now >= self.product_deadline:
            return 'product-180s-timeout'
        if point['oom'] > self.initial['oom'] or point['oom_kill'] > self.initial['oom_kill']:
            return 'new-cgroup-oom-event'
        self.memory_since = (self.memory_since or now) if point['anon_shmem'] >= point['memory_limit'] - 2 * GIB else None
        self.pressure_since = (self.pressure_since or now) if point['full_avg10'] >= 10 else None
        if self.memory_since is not None and now - self.memory_since >= 3:
            return 'anon-plus-shmem-within-2GiB-of-limit-for-3s'
        if self.pressure_since is not None and now - self.pressure_since >= 5:
            return 'memory-full-avg10-at-least-10-percent-for-5s'
        return None

    def _shutdown(self):
        with self.lock:
            processes = list(self.owned)
        for sig in (signal.SIGTERM, signal.SIGKILL):
            for process, start, group in processes:
                try:
                    current = process_start(process.pid)
                    if start is not None and current in (None, start):
                        if group == process.pid:
                            # The created session can outlive its leader (uv/provider
                            # descendants). A reused leader PID is rejected above.
                            os.killpg(group, sig)
                        elif process.poll() is None and current == start:
                            process.send_signal(sig)
                except ProcessLookupError:
                    pass
            if sig == signal.SIGTERM:
                time.sleep(2)

    def _diagnostic(self, name, value):
        try:
            (self.output / name).write_text(json.dumps(value, indent=2) + '\n')
        except Exception as error:
            # Diagnostics must never prevent stopping an already-owned child.
            self.diagnostic_errors.append({'file': name, 'error': repr(error)})

    def _stop_owned(self):
        try:
            self._diagnostic('watchdog-stop.json', self.failure)
        finally:
            try:
                self._shutdown()
            finally:
                self._diagnostic('owned-process-status.json', [
                    {'pid': process.pid, 'start_ticks': start, 'process_group': group,
                     'returncode': process.poll(),
                     'same_process_still_present': start is not None and process_start(process.pid) == start}
                    for process, start, group in self.owned])

    def _monitor(self):
        try:
            with (self.output / 'watchdog-resources.jsonl').open('w') as log:
                while not self.done.is_set():
                    point = resources(self.workspace)
                    growth = max(0, allocated(self.paths) - self.initial_allocated, self.initial['free'] - point['free'])
                    evidence = allocated([self.evidence, self.output])
                    temporary = max(0, allocated([self.workspace / 'tmp']) - self.initial_temp)
                    log.write(json.dumps(point | {'allocated_growth': growth, 'evidence_allocated': evidence, 'temporary_growth': temporary}) + '\n')
                    log.flush()
                    reason = self.reason(point, growth, evidence, temporary)
                    if reason:
                        self.failure = point | {'reason': reason, 'allocated_growth': growth}
                        break
                    self.done.wait(.5)
        except Exception as error:
            self.failure = {'reason': 'watchdog-observation-error', 'error': repr(error)}
        finally:
            if self.failure:
                self._stop_owned()

    def close(self):
        self.done.set()
        self.thread.join()
        active = False
        for process, start, group in self.owned:
            if process.poll() is None:
                active = True
            elif group == process.pid and start is not None and process_start(process.pid) in (None, start):
                try:
                    os.killpg(group, 0)
                    active = True
                except ProcessLookupError:
                    pass
        if active:
            if self.failure is None:
                self.failure = {'reason': 'owner-exit-with-live-child'}
            self._stop_owned()
        self.check()


def accepted(result):
    if result.get('accepted') is not True:
        raise RuntimeError('first product/oracle failure; artifacts retained; cohort stopped')


def comparison_runner(original, output):
    def run(argv, *args, **kwargs):
        result = original(argv, *args, **kwargs)
        if list(argv[:2]) == ['node', 'benchmarks/runtime/compare-http.mjs'] and result.returncode:
            (Path(output) / 'first-comparison-failure.json').write_text(json.dumps({
                'argv': argv, 'returncode': result.returncode,
                'stdout': result.stdout, 'stderr': result.stderr}, indent=2) + '\n')
            raise RuntimeError('first cross-revision oracle failure; cohort stopped')
        return result
    return run


class _Once(argparse.Action):
    def __call__(self, parser, namespace, values, option_string=None):
        seen = getattr(namespace, '_seen_options', set())
        if self.dest in seen:
            parser.error('duplicate option: ' + option_string)
        seen.add(self.dest)
        namespace._seen_options = seen
        setattr(namespace, self.dest, values)


def parse_ownership_args(argv, workspace):
    parser = argparse.ArgumentParser(allow_abbrev=False)
    for name in ('before', 'after', 'output'):
        parser.add_argument('--' + name, type=Path, action=_Once, required=True)
    parser.add_argument('--pairs', type=int, action=_Once, default=3)
    options = parser.parse_args(argv)
    if options.pairs != 3:
        parser.error('ownership mode requires three exploratory pairs')
    for name in ('before', 'after', 'output'):
        setattr(options, name, getattr(options, name).resolve())
    workspace = Path(workspace).resolve()
    products = (workspace / 'evidence/products').resolve()
    if not products.is_relative_to(workspace) or not options.output.is_relative_to(products) or options.output == products:
        parser.error('output must be a new cohort under workspace/evidence/products')
    return options


BLOCKS = [('live-main', 'live-subscriber', ['short-small', 'short-large']),
          ('late-control', 'no-live-subscriber', ['short-large'])]


def cohort_arguments(options, driver):
    """Exactly the reviewed 18-product schedule; one shared 30-minute guard."""
    blocks = []
    for name, mode, cases in BLOCKS:
        block = [str(driver), '--before', str(options.before), '--after', str(options.after),
                 '--output', str(options.output / name), '--pairs', str(options.pairs), '--observer-mode', mode]
        for case in cases:
            block += ['--case', case]
        blocks.append(block)
    return blocks


def expected_plan(options):
    plan = []
    for name, mode, cases in BLOCKS:
        for pair in range(3):
            for case in cases:
                for side in (('before', 'after') if pair % 2 == 0 else ('after', 'before')):
                    plan.append({'label': 'baseline' if side == 'before' else 'ownership',
                                 'case': case, 'observer_mode': mode,
                                 'stem': str(options.output / name / f'{pair}-{case}-{side}')})
    return plan


def watch_build(repo, output, workspace, label, toolchain, libclang, baseline_build=None):
    """Fixed ordinary-product build; intentionally never runs cargo clean."""
    workspace = Path(workspace).resolve()
    toolchain = Path(toolchain).resolve()
    head, tree = SOURCES[label]
    repo, output = Path(repo).resolve(), Path(output).resolve()
    def git(*args):
        return subprocess.check_output(['git', *args], cwd=repo, text=True).strip()
    if git('rev-parse', 'HEAD') != head or git('rev-parse', 'HEAD:src') != tree:
        raise ValueError('build checkout is not the pinned source for this label')
    inputs = ['src', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'tests/conformance', 'tests/common/provider_emulator.rs', 'benchmarks/runtime', 'test-support/fake-provider']
    if git('status', '--porcelain', '--', *inputs):
        raise ValueError('uncommitted build input')
    env = dict(os.environ)
    for key in list(env):
        if key.startswith(('CARGO_PROFILE_', 'CARGO_TARGET_')) or key in ('CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'LD_PRELOAD', 'CPUPROFILE', 'RUSTUP_TOOLCHAIN', 'RUSTC', 'RUSTDOC', 'CARGO_BUILD_TARGET', 'CARGO_BUILD_RUSTC', 'CARGO_BUILD_RUSTFLAGS', 'CARGO_BUILD_RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER'):
            env.pop(key)
    env.update(PATH=str(toolchain / 'bin') + ':' + env['PATH'], CARGO_HOME='/workspace/.cargo',
               RUSTUP_HOME='/workspace/.rustup', RUSTC=str(toolchain / 'bin/rustc'), RUSTDOC=str(toolchain / 'bin/rustdoc'), CARGO_INCREMENTAL='0',
               CARGO_TARGET_DIR=str(workspace / 'target'), CARGO_BUILD_JOBS='2',
               CARGO_PROFILE_RELEASE_OPT_LEVEL='3', CARGO_PROFILE_RELEASE_DEBUG='0',
               RUSTFLAGS='-Copt-level=3 -Cdebuginfo=0', TMPDIR=str(workspace / 'tmp'),
               BINDGEN_EXTRA_CLANG_ARGS='-isystem /usr/lib/gcc/x86_64-linux-gnu/14/include', LIBCLANG_PATH=str(Path(libclang).resolve()))
    argv = [str(toolchain / 'bin/cargo'), 'test', '--release', '--locked', '--offline', '--test', 'conformance', '--no-run', '--message-format=json', '-vv', '-j', '2']
    guard = Guard(workspace, output, 'build')
    safe_env = ['CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS', 'CARGO_INCREMENTAL', 'CARGO_PROFILE_RELEASE_OPT_LEVEL', 'CARGO_PROFILE_RELEASE_DEBUG',
                'RUSTFLAGS', 'RUSTC', 'TMPDIR', 'BINDGEN_EXTRA_CLANG_ARGS', 'LIBCLANG_PATH']
    identity = {'head': head, 'production_tree': tree, 'repo': str(repo), 'verified': False, 'commands': [argv],
                'build_environment': {key: env[key] for key in safe_env},
                'rustc': subprocess.check_output(['rustc', '--version', '--verbose'], env=env, text=True),
                'cargo': subprocess.check_output(['cargo', '--version'], env=env, text=True),
                'source_sha256': {name: sha(repo / name) for name in ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'src/runtime_client/projection.rs']},
                'root_artifacts': [], 'harness': {}}
    if 'rustc 1.98.1 (48a229cea 2026-09-01)' not in identity['rustc'] or 'LLVM version: 22.1.8' not in identity['rustc'] or identity['cargo'].strip() != 'cargo 1.98.1 (797e8a9bc 2026-08-05)':
        raise ValueError('toolchain differs from pinned provenance')
    validate_build_inputs(identity)
    baseline = None
    if label == 'ownership':
        if baseline_build is None:
            raise ValueError('candidate requires a verified fresh baseline build')
        baseline = json.loads((Path(baseline_build) / 'identity.json').read_text())
        validate_product(baseline, json.loads((Path(baseline_build) / 'preserved-artifacts.json').read_text()), 'baseline')
    (workspace / 'tmp').mkdir(parents=True, exist_ok=True)
    guard.start()
    try:
        guard.check()
        with (output / 'build.stdout').open('w') as stdout, (output / 'build.stderr').open('w') as stderr:
            process = subprocess.Popen(argv, cwd=repo, env=env, stdout=stdout, stderr=stderr, start_new_session=True)
            guard.register(process)
            identity['build_exit'] = process.wait()
        guard.check()
        if identity['build_exit'] != 0:
            raise RuntimeError('candidate build failed; output retained')
        if git('rev-parse', 'HEAD') != head or git('status', '--porcelain', '--', *inputs):
            raise RuntimeError('build inputs changed')
        rows = [json.loads(line) for line in (output / 'build.stdout').read_text().splitlines() if line.startswith('{')]
        roots = [row for row in rows if row.get('reason') == 'compiler-artifact' and row['target']['name'] in ('rustx', 'conformance', 'bash-supervisor', 'interactive-supervisor')]
        for name, kind, source in [('rustx', ['lib'], 'src/lib.rs'), ('conformance', ['test'], 'tests/conformance/main.rs')]:
            selected = [row for row in roots if row['target']['name'] == name and row['target']['kind'] == kind]
            if len(selected) != 1 or selected[0]['fresh'] is not False or Path(selected[0]['target']['src_path']).resolve() != repo / source:
                raise RuntimeError('unexpected cached/foreign root artifact; stop before timing')
        artifacts = []
        for row in roots:
            identity['root_artifacts'].append({'name': row['target']['name'], 'kind': row['target']['kind'], 'source_path': row['target']['src_path'], 'fresh': row['fresh']})
            for filename in row['filenames']:
                source = Path(filename)
                destination = output / ('conformance' if row['target']['name'] == 'conformance' else 'artifacts/' + source.name)
                destination.parent.mkdir(exist_ok=True)
                shutil.copy2(source, destination)
                artifacts.append({'target': str(source), 'preserved': str(destination), 'name': row['target']['name'],
                                  'kind': row['target']['kind'], 'profile': row['profile'], 'features': row['features'],
                                  'sha256': sha(destination), 'bytes': destination.stat().st_size})
                guard.check()
        for source in (workspace / 'target/release/deps').glob('*.so'):
            destination = output / 'artifacts' / source.name
            shutil.copy2(source, destination)
            artifacts.append({'target': str(source), 'preserved': str(destination), 'name': source.name,
                              'kind': ['shared-library'], 'sha256': sha(destination), 'bytes': destination.stat().st_size})
            guard.check()
        for name in git('ls-files', '--', 'tests/common/provider_emulator.rs',
                        'tests/conformance/agent_loop/runtime_benchmark.rs',
                        'tests/conformance/agent_loop/runtime_benchmark', 'benchmarks/runtime',
                        'test-support/fake-provider').splitlines():
            identity['harness'][name] = sha(repo / name)
        identity['linker_comment'] = subprocess.check_output(['readelf', '-p', '.comment', str(output / 'conformance')], text=True)
        identity['shared_libraries'] = {r['target']: r['sha256'] for r in artifacts if r['kind'] == ['shared-library']}
        identity.update(binary=str(output / 'conformance'), sha256=sha(output / 'conformance'),
                        library_sha256=next(item['sha256'] for item in artifacts if item['kind'] == ['lib'] and item['preserved'].endswith('.rlib')))
        preserved = {'source_head': head, 'artifacts': artifacts}
        (output / 'preserved-artifacts.json').write_text(json.dumps(preserved, indent=2) + '\n')
        identity['verified'] = True
        validate_product(identity, preserved, label)
        if baseline is not None:
            validate_pair(baseline, identity)
            validate_candidate(identity, preserved, baseline)
        guard.check()
    except BaseException:
        identity['verified'] = False
        raise
    finally:
        try:
            guard.close()
        except BaseException:
            identity['verified'] = False
            raise
        finally:
            (output / 'identity.json').write_text(json.dumps(identity, indent=2) + '\n')


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--watch-build', action='store_true', required=True)
    parser.add_argument('--repo', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--label', choices=tuple(SOURCES), required=True)
    parser.add_argument('--toolchain', type=Path, required=True)
    parser.add_argument('--libclang', type=Path, required=True)
    parser.add_argument('--baseline-build', type=Path)
    args = parser.parse_args()
    watch_build(args.repo, args.output, args.workspace, args.label, args.toolchain, args.libclang, args.baseline_build)
