#!/usr/bin/env python3
"""Verify two raw three-pair Pi cohorts and independently recompute paired costs.

Usage: python3 recompute.py --root /path/to/extracted/raw > recomputed.json
No producer statistics or modules are imported. Recorded semantic-oracle results
are checked, not independently reimplemented. Missing counters stay unavailable.
Strict mode checks actual binaries and the recorded build identity. --raw-only
never follows historical manifest paths; an explicitly supplied --identity may
name an archived build-identity file. No current Git HEAD is consulted.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import statistics

CASES = ['short-small', 'short-large', 'stream', 'large', 'tool', 'cancel']
MODEL_TIME = '2026-10-19 00:00:00'
PI_REVISION = '90d863f7c1183b965f099855715d6e88c7b8a52c'
COHORTS = [('pi-no-live', 'no-live-subscriber', 'rust', 'pi'),
           ('pi-live', 'live-subscriber', 'rust', 'pi')]
PHASES = ['before_initialization', 'after_provider_start', 'after_initialization',
          'after_turns', 'after_late_snapshot', 'after_verification',
          'after_shutdown', 'after_driver_drop', 'after_final_writer_close',
          'after_reopen_verification', 'after_cleanup']
POLICY = ['mean_turn_ms', 'turn_cpu_ms_per_turn', 'through_cleanup_self_cpu_ms',
          'through_cleanup_process_write_mib', 'max_sampled_anon_pss_mib']
CRITICAL = 2.919985580355516  # n=3, df=2; two-sided 90% / one-sided 95% upper.


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value):
    require(type(value) is int and value >= 0, f'invalid nonnegative integer: {value!r}')
    return value


def load(path):
    blob = path.read_bytes()
    return json.loads(blob), hashlib.sha256(blob).hexdigest()


def counters(point):
    process = point['process']
    require(set(process) == {'stat', 'io', 'smaps_rollup'}, 'process counter shape')
    ticks = None
    if process['stat'] is not None:
        fields = process['stat'].rsplit(')', 1)[1].split()
        ticks = integer(int(fields[11])) + integer(int(fields[12]))
    io = dict(line.split(':', 1) for line in (process['io'] or '').splitlines())
    memory = dict(line.split(':', 1) for line in (process['smaps_rollup'] or '').splitlines() if ':' in line)
    result = {'cpu_ticks': ticks, 'write_bytes': integer(int(io['write_bytes'])) if 'write_bytes' in io else None}
    for key in ('Rss', 'Pss', 'Pss_Anon'):
        if key in memory:
            value, unit = memory[key].split()
            require(unit == 'kB', 'unexpected smaps unit')
            result[key] = integer(int(value))
        else:
            result[key] = None
    return result


def scale(value, divisor):
    return None if value is None else value / divisor


def difference(end, start):
    if end is None or start is None:
        return None
    require(end >= start, 'counter decreased across measurement window')
    return end - start


def derive(data, process, case, mode, engine, manifest):
    rust = engine != 'pi'
    argv = process['argv']
    if rust:
        executable = manifest['executables'][engine]['path'] if engine in ('before', 'after') else manifest['rust']
        require(argv == [executable, 'agent_loop::runtime_benchmark::measure_runtime', '--exact', '--ignored', '--nocapture'], 'Rust process/manifest executable identity')
    else:
        require(data['revision'] == PI_REVISION, 'Pi revision differs from pinned source')
        require(len(argv) == 6 and argv[:5] == ['node', '--conditions=source', 'benchmarks/runtime/pi-http.mjs', manifest['pi'], case], 'Pi process/manifest checkout identity')
    history = {'short-small': 8, 'short-large': 64}.get(case, 0)
    turns = 32 if history else 1
    requests = {'short-small': 40, 'short-large': 96, 'tool': 2}.get(case, 1)
    require(process['accepted'] is True and process['product_exit'] == process['provider_exit'] == 0, 'unaccepted product')
    require(process['case'] == case and process['observer_mode'] == mode, 'process identity')
    provider = process['provider_report']
    require(provider['ok'] is True and provider['requestCount'] == requests, 'provider validation')
    require(data['oracle'] == 'passed' and data['workload_version'] == 3, 'sample oracle/version')
    require(data['model_time'] == MODEL_TIME and data['observer_mode'] == mode, 'sample time/mode')
    require(data.get('case', data.get('scenario')) == case, 'sample case')
    require(data['history'] == history and data['measured'] == turns, 'workload dimensions')
    samples = data['samples']
    require(len(samples) == history + turns, 'turn count')
    for i, point in enumerate(samples):
        require(point['turn'] == i and point['measured'] is (i >= history), 'turn identity')
        require(point['observer_mode'] == mode, 'turn observer mode')
        integer(point['settled_us'])
    wire = data['wire_requests'] if rust else data['requests']
    require(len(wire) == requests and wire == provider['requests'], 'captured/provider requests')
    if rust:
        require(data['measurement_version'] == 4, 'Rust measurement version')
        require(data['verified_live_requests'] == data['verified_reopened_requests'] == requests, 'reopen count')
        require(len(data['live_requests']) == requests and data['live_requests'] == data['reopened_requests'], 'reopened Rust inputs')
    else:
        require('measurement_version' not in data, 'Pi product identity')
        require(len(data['captures']) == requests and [r['messages'] for r in data['captures']] == data['reopenedRequests'], 'reopened Pi inputs')
    points = data['lifecycle']
    expected = PHASES if rust else [p for p in PHASES if p not in ('after_shutdown', 'after_driver_drop')]
    require([p['phase'] for p in points] == expected, 'missing/reordered lifecycle endpoint')
    elapsed = [integer(p['elapsed_us']) for p in points]
    require(elapsed == sorted(elapsed), 'lifecycle clock moved backwards')
    life = {p['phase']: counters(p) for p in points}
    sample_counters = [counters(p) for p in samples]
    start = sample_counters[history - 1] if history else life['after_initialization']
    end, final = sample_counters[-1], life['after_cleanup']
    settled = [p['settled_us'] for p in samples[history:]]
    turn_ticks = difference(end['cpu_ticks'], start['cpu_ticks'])
    metrics = dict(mean_turn_ms=sum(settled) / turns / 1000,
                   turn_cpu_ms_per_turn=None if turn_ticks is None else turn_ticks * 10 / turns,
                   through_cleanup_self_cpu_ms=None if final['cpu_ticks'] is None else final['cpu_ticks'] * 10,
                   through_cleanup_process_write_mib=scale(final['write_bytes'], 2**20),
                   lifecycle_ms=(elapsed[-1] - elapsed[0]) / 1000)
    memories = [*life.values(), *sample_counters]
    for key, name in [('Pss_Anon', 'anon_pss'), ('Pss', 'pss'), ('Rss', 'rss')]:
        values = [p[key] for p in memories]
        metrics[f'max_sampled_{name}_mib'] = None if any(v is None for v in values) else max(values) / 1024
    arithmetic = dict(measured_turns=turns, measured_settled_us=settled,
                      start_cpu_ticks=start['cpu_ticks'], end_cpu_ticks=end['cpu_ticks'],
                      cleanup_cpu_ticks=final['cpu_ticks'], cleanup_write_bytes=final['write_bytes'],
                      lifecycle_begin_us=elapsed[0], lifecycle_end_us=elapsed[-1],
                      memory_sample_count=len(memories),
                      missing_memory_samples={key: sum(p[key] is None for p in memories) for key in ('Pss_Anon', 'Pss', 'Rss')})
    return dict(metrics=metrics, arithmetic=arithmetic)


def summary(values):
    if any(v is None for v in values):
        available = [v for v in values if v is not None]
        return dict(state='indeterminate_missing_or_zero_denominator', n=len(values), values=values,
                    available=len(available), median=statistics.median(available) if available else None,
                    minimum=min(available) if available else None, maximum=max(available) if available else None,
                    log_interval_unavailable_reason='one or more paired ratios are unavailable')
    if any(v == 0 for v in values):
        return dict(state='determinate_arithmetic_log_interval_unavailable', n=len(values), values=values,
                    available=len(values), median=statistics.median(values), minimum=min(values), maximum=max(values),
                    log_interval_unavailable_reason='a zero ratio is retained; log(0) is undefined')
    logs = [math.log(v) for v in values]
    center = statistics.mean(logs)
    width = CRITICAL * statistics.stdev(logs) / math.sqrt(3)
    return dict(state='determinate', n=3, values=values, median=statistics.median(values),
                minimum=min(values), maximum=max(values), geometric_mean=math.exp(center),
                lower90=math.exp(center - width), upper90_one_sided95=math.exp(center + width))


def paired_ratio(numerator, denominator):
    if numerator is None or denominator is None:
        return None, 'missing_counter'
    if denominator == 0:
        return None, 'zero_denominator'
    return numerator / denominator, None


def sha256_file(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def identity_audit(manifests, args):
    """Separate raw evidence identities from optional actual-host verification."""
    errors = []
    expected = args.expect_rust_sha256
    first = manifests['pi-no-live']['manifest']
    advertised_sha = first['rust_sha256']
    valid_sha = isinstance(advertised_sha, str) and len(advertised_sha) == 64 and set(advertised_sha) <= set('0123456789abcdef')
    if not valid_sha:
        errors.append('invalid advertised candidate SHA256')
    if expected is not None and advertised_sha != expected:
        errors.append('manifest candidate SHA256 differs from explicit expected identity')
    supplied_identity = args.identity
    identity_path = supplied_identity if supplied_identity else None if args.raw_only else Path(first['rust']).parent / 'identity.json'
    identity, identity_hash, recorded_verified = None, None, False
    if identity_path is not None:
        try:
            identity, identity_hash = load(identity_path)
            require(isinstance(identity, dict), 'build identity is not an object')
            require(identity.get('verified') is True and identity.get('build_exit') == 0, 'build identity is not verified/successful')
            require(identity.get('sha256') == advertised_sha, 'build identity and manifest candidate SHA256 differ')
            require(identity.get('binary') == first['rust'], 'build identity and manifest candidate paths differ')
            for key in ('head', 'production_tree'):
                value = identity.get(key)
                require(isinstance(value, str) and len(value) == 40 and set(value) <= set('0123456789abcdef'), 'invalid recorded source ' + key)
            require(isinstance(identity.get('harness'), dict) and identity['harness'], 'build identity has no benchmark source hashes')
            require(isinstance(identity.get('rustc'), str) and identity['rustc'], 'build identity has no compiler identity')
            recorded_verified = True
        except (OSError, ValueError, TypeError, KeyError) as error:
            errors.append(f'build identity: {type(error).__name__}: {error}')
    actual_checks = []
    if not args.raw_only:
        for cohort, record in manifests.items():
            manifest = record['manifest']
            check = {'cohort': cohort, 'binary': manifest['rust'], 'verified': False}
            try:
                check['actual_sha256'] = sha256_file(manifest['rust'])
                require(check['actual_sha256'] == manifest['rust_sha256'], 'actual binary differs from recorded manifest')
                if expected is not None:
                    require(check['actual_sha256'] == expected, 'actual binary differs from explicit expected identity')
                check['verified'] = True
            except (OSError, ValueError, TypeError) as error:
                check['error'] = f'{type(error).__name__}: {error}'
                errors.append(f'{cohort}: {check["error"]}')
            actual_checks.append(check)
    return {'mode': 'raw-only' if args.raw_only else 'strict', 'strict_identity_verified': not args.raw_only and recorded_verified and len(actual_checks) == 2 and all(c['verified'] for c in actual_checks) and not errors,
            'recorded_build_identity_verified': recorded_verified, 'actual_binary_files_verified': not args.raw_only and len(actual_checks) == 2 and all(c['verified'] for c in actual_checks),
            'expected_rust_sha256': expected, 'advertised_rust_sha256': advertised_sha,
            'identity_file': str(identity_path) if identity_path is not None else None, 'identity_sha256': identity_hash, 'build_identity': identity,
            'actual_binary_checks': actual_checks, 'errors': errors}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--expect-rust-sha256')
    parser.add_argument('--identity', type=Path)
    parser.add_argument('--raw-only', action='store_true', help='recompute archived raw evidence without accessing historical host binaries or inferred identity paths')
    args = parser.parse_args()
    root = args.root
    manifests, products, comparisons, paired, groups = {}, [], [], [], {}
    for cohort, mode, numerator, denominator in COHORTS:
        directory = root / cohort
        manifest, manifest_hash = load(directory / 'manifest.json')
        require(manifest['observer_mode'] == mode and type(manifest['clock_ticks']) is int and manifest['clock_ticks'] == 100, f'{cohort}: manifest mode/ticks')
        require(manifest['cases'] == CASES and manifest['pairs'] == 3 and manifest.get('pair_offset', 0) == 0, f'{cohort}: incomplete matrix')
        manifests[cohort] = dict(manifest=manifest, sha256=manifest_hash)
        stems = {f'{pair}-{case}-{engine}' for pair in range(3) for case in CASES for engine in (numerator, denominator)}
        require({p.name for p in directory.glob('*.process.json')} == {f'{s}.process.json' for s in stems}, f'{cohort}: product matrix')
        require({p.name for engine in (numerator, denominator) for p in directory.glob(f'*-{engine}.json')} == {f'{s}.json' for s in stems}, f'{cohort}: raw sample matrix')
        require({p.name for p in directory.glob('*-comparison.json')} == {f'{pair}-{case}-comparison.json' for pair in range(3) for case in CASES}, f'{cohort}: comparison matrix')
        require(not list(directory.glob('*-comparison-error.log')), f'{cohort}: failed comparison retained')
        for pair in range(3):
            for case in CASES:
                values = {}
                for engine in (numerator, denominator):
                    stem = f'{pair}-{case}-{engine}'
                    data, raw_hash = load(directory / f'{stem}.json')
                    process, process_hash = load(directory / f'{stem}.process.json')
                    if engine == 'pi':
                        require(Path(process['argv'][-1]).name == f'{stem}.json', 'Pi process output/sample identity')
                    result = derive(data, process, case, mode, engine, manifest)
                    values[engine] = result['metrics']
                    products.append(dict(cohort=cohort, file=f'{cohort}/{stem}.json', raw_sha256=raw_hash,
                                         process_sha256=process_hash, **result))
                comparison, digest = load(directory / f'{pair}-{case}-comparison.json')
                require(comparison['workload_version'] == 3 and comparison['model_time'] == MODEL_TIME and comparison['case'] == case and comparison['observer_mode'] == mode, 'comparison identity')
                checked = comparison['checked']
                require(len(checked) == {'short-small': 40, 'short-large': 96, 'tool': 2}.get(case, 1), 'comparison request count')
                same_engine = denominator == 'before'
                schemas = set()
                for i, row in enumerate(checked):
                    require(row['index'] == i and row['coreSemanticEqual'] is True and row['fullSemanticRequestEqual'] is same_engine, 'comparison result')
                    left, right = row['rustToolNames'], row['piToolNames']
                    require(left == ['read', 'write', 'edit', 'glob', 'grep', 'bash', 'todo'], 'Rust native tool surface')
                    require(right == (left if same_engine else ['read', 'write', 'edit', 'bash']), 'paired native tool surface')
                    require(all(isinstance(row[key], str) and len(row[key]) == 64 and set(row[key]) <= set('0123456789abcdef') for key in ('rustToolsSha256', 'piToolsSha256')), 'tool schema digest')
                    require((row['rustToolsSha256'] == row['piToolsSha256']) is same_engine, 'tool schema equality')
                    schemas.add(json.dumps({k: row[k] for k in ('rustToolNames', 'piToolNames', 'rustToolsSha256', 'piToolsSha256')}, sort_keys=True))
                comparisons.append(dict(cohort=cohort, pair=pair, case=case, sha256=digest,
                                        checked_requests=len(checked), full_semantic_request_equal=same_engine,
                                        tool_schema_variants=[json.loads(s) for s in sorted(schemas)]))
                for metric, value in values[numerator].items():
                    base = values[denominator][metric]
                    ratio, unavailable_reason = paired_ratio(value, base)
                    paired.append(dict(cohort=cohort, mode=mode, pair=pair, case=case, metric=metric,
                                       numerator_engine=numerator, denominator_engine=denominator,
                                       numerator=value, denominator=base, ratio=ratio,
                                       ratio_unavailable_reason=unavailable_reason))
                    groups.setdefault((cohort, mode, case, metric), []).append(ratio)
    hashes = {name: record['manifest'] for name, record in manifests.items()}
    require(hashes['pi-no-live']['rust_sha256'] == hashes['pi-live']['rust_sha256'], 'candidate binary differs across modes')
    require(hashes['pi-no-live']['rust'] == hashes['pi-live']['rust'], 'candidate binary path differs across modes')
    require(hashes['pi-no-live']['pi'] == hashes['pi-live']['pi'], 'Pi checkout differs across observation modes')
    intervals = [dict(cohort=k[0], mode=k[1], case=k[2], metric=k[3], **summary(v)) for k, v in groups.items()]
    identity = identity_audit(manifests, args)
    print(json.dumps(dict(verified=not identity['errors'], raw_data_verified=True, strict_identity_verified=identity['strict_identity_verified'], identity_audit=identity,
                         script_sha256=sha256_file(__file__), accepted_products=len(products), request_comparisons=sum(r['checked_requests'] for r in comparisons),
                         clock_ticks=100, policy_metrics=POLICY, manifests=manifests, products=products,
                         comparisons=comparisons, paired_records=paired, summaries=intervals,
                         scope='Independent raw-counter arithmetic; recorded semantic oracles checked, not reimplemented. Actual Rust binary/build-record verification is separate from raw arithmetic; raw-only never follows historical host paths and is not strict identity verification. The build records retain source head/tree/harness/compiler identity; this audit does not prove that a recorded source tree produced a binary. Pi revision is checked from retained raw data, not from a current checkout. Self CPU includes driver/verification; writes are process-attributed; memory is sampled, not peak. Zero ratios remain zero and have no logarithmic interval; zero denominators remain unavailable. No timing-isolation or parity claim.'), indent=2, allow_nan=False))
    return 0 if not identity['errors'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
