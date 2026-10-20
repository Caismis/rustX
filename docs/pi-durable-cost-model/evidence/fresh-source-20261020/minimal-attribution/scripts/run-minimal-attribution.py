#!/usr/bin/env python3
"""Serial diagnostic controls/profiles around the unchanged published runner."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import resource
import runpy
import shutil
import subprocess
import sys
import threading
import time

ROOT = Path('/workspace/pr485-study')
REPO = ROOT / 'repo'
OUT = ROOT / 'evidence/minimal-attribution-20261020'
TOOLS = ROOT / 'diagnostic-tools'
PUBLISHED = REPO / 'benchmarks/runtime/profiling/run.py'
os.environ['TMPDIR'] = str(ROOT / 'tmp')
os.environ['TZ'] = 'UTC'
assert not os.environ.get('LD_PRELOAD')


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def sha(path):
    return hashlib.file_digest(Path(path).open('rb'), 'sha256').hexdigest()


def idle_and_space():
    s = os.statvfs(ROOT)
    assert s.f_bavail * s.f_frsize > 5368709120
    active = []
    for p in Path('/proc').iterdir():
        if not p.name.isdigit():
            continue
        try:
            c = (p / 'comm').read_text().strip()
            if c in ('cargo', 'rustc', 'rust-lld', 'cc1', 'cc1plus', 'xz', 'gzip', 'perf', 'strace', 'conformance', 'inspector-host', 'fake-provider'):
                active.append({'pid': p.name, 'comm': c})
        except OSError:
            pass
    assert not active, active
    return {'utc': now(), 'usable_bytes': s.f_bavail * s.f_frsize, 'active_products_builds_compression_profilers': active}


def restore(label):
    build = ROOT / 'evidence' / ('build-' + label)
    identity = json.loads((build / 'identity.json').read_text())
    assert identity['verified'] and sha(build / 'conformance') == identity['sha256']
    copied = []
    for item in json.loads((build / 'preserved-artifacts.json').read_text())['artifacts']:
        if item['kind'] not in (['bin'], ['test'], ['shared-library']):
            continue
        src, dst = Path(item['preserved']), Path(item['target'])
        assert sha(src) == item['sha256']
        if not dst.exists() or sha(dst) != item['sha256']:
            dst.parent.mkdir(parents=True, exist_ok=True)
            staging = dst.with_name(dst.name + '.restore')
            shutil.copy2(src, staging)
            staging.replace(dst)
            copied.append(str(dst))
        assert sha(dst) == item['sha256']
    return identity, copied


original_popen = subprocess.Popen
accounted = []
current_children = []


class AccountedPopen(original_popen):
    def __init__(self, argv, *args, **kwargs):
        super().__init__(argv, *args, **kwargs)
        current_children.append({'pid': self.pid, 'argv': argv})

    def _try_wait(self, flags):
        try:
            pid, status, usage = os.wait4(self.pid, flags)
        except ChildProcessError:
            return self.pid, 0
        if pid:
            accounted.append({'pid': pid, 'argv': self.args, 'wait4_status': status,
                              'user_seconds': usage.ru_utime, 'system_seconds': usage.ru_stime,
                              'maxrss_kib': usage.ru_maxrss, 'minor_faults': usage.ru_minflt,
                              'major_faults': usage.ru_majflt, 'voluntary_switches': usage.ru_nvcsw,
                              'involuntary_switches': usage.ru_nivcsw})
        return pid, status


def proc(pid):
    p = Path('/proc') / str(pid)
    try:
        return {'pid': pid, 'stat': (p / 'stat').read_text(), 'io': (p / 'io').read_text(),
                'comm': (p / 'comm').read_text().strip()}
    except OSError:
        return None


def descendants(pid):
    try:
        children = [int(x) for x in (Path('/proc') / str(pid) / 'task' / str(pid) / 'children').read_text().split()]
    except OSError:
        return []
    return children + [c for child in children for c in descendants(child)]


protected = [PUBLISHED, REPO / 'benchmarks/runtime/controller.py', REPO / 'benchmarks/runtime/observer_oracle.py']
hashes = {str(p): sha(p) for p in protected}
manifest = {'start_utc': now(), 'scope': 'Diagnostics only; not formal speed ratios. Unchanged fixed live short-large workload, preserved O3/debug0 binaries. Controls have no preload/socket; profiles use published gperftools250Hz/system allocator workflow. All runs serial, no builds/compression overlap.',
            'source_hashes': hashes, 'outer_driver_sha256': sha(__file__),
            'profiler_library_sha256': sha(TOOLS / 'unpacked/usr/lib/x86_64-linux-gnu/libprofiler.so.0'),
            'bridge_source_sha256': sha(REPO / 'benchmarks/runtime/profiling/bridge.c'),
            'bridge_sha256': sha(TOOLS / 'bridge.so'), 'process_accounting_scope': 'wait4 for direct product/provider command; waited descendants may be included in provider-command rusage. Descendant/self proc samples are separate and must not be summed again with ancestor rusage. Self controller CPU delta excludes children. Polling250ms can miss final descendant CPU; exact unavailable is not zero.', 'runs': []}
meta = OUT / 'orchestration.json'
meta.write_text(json.dumps(manifest, indent=2) + '\n')
sys.path.insert(0, str(REPO / 'benchmarks/runtime'))
os.chdir(REPO)
for label, mode in [('baseline', 'control'), ('geometry', 'control'), ('geometry', 'cpu'), ('baseline', 'cpu')]:
    run = {'label': label, 'mode': mode, 'preflight': idle_and_space()}
    identity, copied = restore(label)
    run.update(binary_identity=identity, helpers_restored=copied, ready=idle_and_space())
    output = OUT / (label + '-' + mode)
    assert not output.exists()
    argv = [str(PUBLISHED), '--binary', str(ROOT / 'evidence' / ('build-' + label) / 'conformance'), '--profiler-root', str(TOOLS), '--output', str(output), '--mode', mode, '--case', 'short-large', '--observer-mode', 'live-subscriber']
    run.update(argv=['python3', *argv], start_utc=now(), start_monotonic_ns=time.monotonic_ns())
    manifest['runs'].append(run)
    meta.write_text(json.dumps(manifest, indent=2) + '\n')
    accounted.clear(); current_children.clear()
    samples = []
    stopped = threading.Event()

    def sample():
        while not stopped.is_set():
            ids = [os.getpid(), *[x['pid'] for x in current_children]]
            ids += [p for x in current_children for p in descendants(x['pid'])]
            samples.append({'monotonic_ns': time.monotonic_ns(), 'processes': [v for pid in sorted(set(ids)) if (v := proc(pid)) is not None]})
            stopped.wait(.25)

    thread = threading.Thread(target=sample, daemon=True)
    before = resource.getrusage(resource.RUSAGE_SELF)
    subprocess.Popen = AccountedPopen
    thread.start()
    try:
        sys.argv = argv
        with (OUT / (label + '-' + mode + '.stdout')).open('w') as stdout, (OUT / (label + '-' + mode + '.stderr')).open('w') as stderr:
            previous_stdout, previous_stderr = sys.stdout, sys.stderr
            sys.stdout, sys.stderr = stdout, stderr
            try:
                runpy.run_path(str(PUBLISHED), run_name='__main__')
            finally:
                sys.stdout, sys.stderr = previous_stdout, previous_stderr
        run['completed'] = True
    except BaseException as error:
        run['error'] = repr(error)
        raise
    finally:
        stopped.set(); thread.join()
        subprocess.Popen = original_popen
        after = resource.getrusage(resource.RUSAGE_SELF)
        run.update(end_utc=now(), end_monotonic_ns=time.monotonic_ns(), wait4=accounted.copy(),
                   children=current_children.copy(), controller_self_cpu_seconds=(after.ru_utime-before.ru_utime)+(after.ru_stime-before.ru_stime))
        (OUT / (label + '-' + mode + '-processes.json')).write_text(json.dumps(samples) + '\n')
        assert {str(p): sha(p) for p in protected} == hashes
        assert sha(Path(identity['binary'])) == identity['sha256']
        run['space_after'] = idle_and_space()
        meta.write_text(json.dumps(manifest, indent=2) + '\n')
    print(json.dumps({'label': label, 'mode': mode, 'complete': True}), flush=True)
manifest['end_utc'] = now()
meta.write_text(json.dumps(manifest, indent=2) + '\n')
