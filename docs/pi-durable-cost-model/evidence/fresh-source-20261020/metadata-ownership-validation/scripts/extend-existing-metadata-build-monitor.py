#!/usr/bin/env python3
"""Extend only supervisor waiting; keep the same Cargo/rustc and space guards."""
from pathlib import Path
import datetime, hashlib, json, os, signal, time

root = Path('/workspace/pr485-study')
out = root / 'evidence/metadata-ownership-candidate-20261020'
repo = root / 'metadata-ownership-candidate'
cargo_pid, guardian_pid = 39389, 39377

def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()

def proc(pid):
    path = Path('/proc') / str(pid)
    try:
        fields = (path / 'stat').read_text().rsplit(')', 1)[1].split()
        return {'state': fields[0], 'parent': int(fields[1]), 'pgrp': int(fields[2]),
                'start_ticks': int(fields[19]), 'cpu_ticks': int(fields[11]) + int(fields[12]),
                'raw_exit_code': int(fields[49]),
                'cmdline': (path / 'cmdline').read_bytes().replace(b'\0', b' ').decode()}
    except FileNotFoundError:
        return None

def point():
    value = os.statvfs(root)
    return {'utc': now(), 'monotonic_ns': time.monotonic_ns(),
            'available_bytes': value.f_bavail * value.f_frsize,
            'memory_current': int(Path('/sys/fs/cgroup/memory.current').read_text())}

manifest = out / 'build-orchestration.json'
build = json.loads(manifest.read_text())
assert build['state'] == 'compiling' and 'exit' not in build
assert point()['available_bytes'] > 6 * 2**30
cargo_before = proc(cargo_pid)
guardian_before = proc(guardian_pid)
assert cargo_before['start_ticks'] == 1641904 and cargo_before['parent'] == guardian_pid and cargo_before['pgrp'] == cargo_pid
assert 'cargo test --release --locked --offline --lib --no-run --message-format=json -vv -j 1' in cargo_before['cmdline']
assert 'build-metadata-ownership-tests.py' in guardian_before['cmdline']
extension = {'start_utc': now(), 'reason': 'Existing first O3 library-test LTO is progressing; extend supervisor waiting without restarting or signaling Cargo/rustc.',
             'cargo_before': cargo_before, 'guardian_before': guardian_before,
             'guard_floor_bytes': 5 * 2**30, 'soft_stop_available_bytes': 6 * 2**30,
             'original_timeout_seconds': 2700, 'replacement_total_timeout_seconds': 5400,
             'before': point(), 'state': 'attaching'}
extension_path = out / 'guard-extension.json'
assert not extension_path.exists()
(out / 'build-orchestration-before-extension.json').write_text(json.dumps(build, indent=2) + '\n')

def save():
    extension_path.write_text(json.dumps(extension, indent=2) + '\n')

save()
os.kill(guardian_pid, signal.SIGSTOP)
extension['state'] = 'guard-attached-original-supervisor-paused'
extension['guardian_after_stop'] = proc(guardian_pid)
save()
print(json.dumps({'state': extension['state'], 'cargo_pid': cargo_pid}), flush=True)
minimum = point()['available_bytes']
stop = None
try:
    with (out / 'extended-build-resources.jsonl').open('w') as log:
        while True:
            current = proc(cargo_pid)
            assert current is not None, 'Cargo disappeared while its original wait-parent was paused'
            assert current['start_ticks'] == cargo_before['start_ticks'], 'Cargo PID identity changed'
            if current['state'] == 'Z':
                extension['cargo_terminal'] = current
                extension['cargo_exit'] = os.waitstatus_to_exitcode(current['raw_exit_code'])
                break
            value = point()
            minimum = min(minimum, value['available_bytes'])
            log.write(json.dumps(value | {'cargo_cpu_ticks': current['cpu_ticks']}) + '\n')
            log.flush()
            elapsed = (value['monotonic_ns'] - build['launch']['monotonic_ns']) / 1e9
            if stop is None and (value['available_bytes'] <= 6 * 2**30 or elapsed > 5400):
                extension['guard_stop'] = value | {'reason': 'soft-space-stop' if value['available_bytes'] <= 6 * 2**30 else '90-minute-total-build-bound'}
                os.killpg(cargo_pid, signal.SIGTERM)
                stop = time.monotonic()
                save()
            elif stop is not None and time.monotonic() - stop > 10:
                os.killpg(cargo_pid, signal.SIGKILL)
                stop = float('inf')
            time.sleep(.5)
finally:
    guardian = proc(guardian_pid)
    if guardian and guardian['start_ticks'] == guardian_before['start_ticks']:
        os.kill(guardian_pid, signal.SIGCONT)

time.sleep(2)
extension.update(state='compile-completed-same-cargo-process', end_utc=now(), after=point(),
                 minimum_observed_available_bytes=minimum,
                 original_supervisor_final=json.loads(manifest.read_text()))
save()
rows = [json.loads(line) for line in (out / 'cargo.stdout.jsonl').read_text().splitlines() if line.startswith('{')]
finished = [row for row in rows if row.get('reason') == 'build-finished']
extension['cargo_build_finished'] = finished
save()
record = build | {'state': 'compile-finished', 'exit': extension['cargo_exit'], 'end_utc': extension['end_utc'],
                  'after': extension['after'], 'guard_extension': str(extension_path),
                  'minimum_observed_available_bytes': min(minimum, *(json.loads(line)['available_bytes'] for line in (out / 'build-resources.jsonl').read_text().splitlines())),
                  'exit_evidence': 'same Cargo PID/start identity terminal /proc exit_code; Cargo build-finished JSON also retained'}
if extension.get('guard_stop'):
    record['guard_stop'] = extension['guard_stop']
if record['exit'] == 0:
    assert finished == [{'reason': 'build-finished', 'success': True}]
for name, digest in record['source_sha256'].items():
    assert hashlib.sha256((repo / name).read_bytes()).hexdigest() == digest
(out / 'build-final-validated.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps({'state': record['state'], 'exit': record['exit'], 'minimum_available_bytes': record['minimum_observed_available_bytes']}), flush=True)
