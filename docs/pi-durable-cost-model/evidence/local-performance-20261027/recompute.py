#!/usr/bin/env python3
"""Verify four raw three-pair cohorts and independently recompute paired costs.

Usage: python3 recompute.py --root /path/to/extracted/raw > recomputed.json
No producer statistics or modules are imported. Recorded semantic-oracle results
are checked, not independently reimplemented. Missing counters stay unavailable.
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
COHORTS = [('revision-no-live', 'no-live-subscriber', 'after', 'before'),
           ('revision-live', 'live-subscriber', 'after', 'before'),
           ('pi-no-live', 'no-live-subscriber', 'rust', 'pi'),
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
        return dict(state='indeterminate_missing_or_nonpositive', n=len(values), values=values)
    logs = [math.log(v) for v in values]
    center = statistics.mean(logs)
    width = CRITICAL * statistics.stdev(logs) / math.sqrt(3)
    return dict(state='determinate', n=3, values=values, median=statistics.median(values),
                minimum=min(values), maximum=max(values), geometric_mean=math.exp(center),
                lower90=math.exp(center - width), upper90_one_sided95=math.exp(center + width))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True, type=Path)
    root = parser.parse_args().root
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
                    ratio = value / base if value is not None and base is not None and value > 0 and base > 0 else None
                    paired.append(dict(cohort=cohort, mode=mode, pair=pair, case=case, metric=metric,
                                       numerator_engine=numerator, denominator_engine=denominator,
                                       numerator=value, denominator=base, ratio=ratio))
                    groups.setdefault((cohort, mode, case, metric), []).append(ratio)
    hashes = {name: record['manifest'] for name, record in manifests.items()}
    for engine in ('before', 'after'):
        require(hashes['revision-no-live']['executables'][engine]['sha256'] == hashes['revision-live']['executables'][engine]['sha256'], f'{engine}: mode binary differs')
    candidate = hashes['revision-live']['executables']['after']['sha256']
    require(candidate != hashes['revision-live']['executables']['before']['sha256'], 'identical revision binaries')
    require(all(hashes[name]['rust_sha256'] == candidate for name in ('pi-no-live', 'pi-live')), 'candidate binary differs across comparison cohorts')
    require(hashes['pi-no-live']['pi'] == hashes['pi-live']['pi'], 'Pi checkout differs across observation modes')
    intervals = [dict(cohort=k[0], mode=k[1], case=k[2], metric=k[3], **summary(v)) for k, v in groups.items()]
    print(json.dumps(dict(verified=True, accepted_products=len(products), request_comparisons=sum(r['checked_requests'] for r in comparisons),
                         clock_ticks=100, policy_metrics=POLICY, manifests=manifests, products=products,
                         comparisons=comparisons, paired_records=paired, summaries=intervals,
                         scope='Independent raw-counter arithmetic; recorded semantic oracles checked, not reimplemented. Self CPU includes driver/verification; writes are process-attributed; memory is sampled, not peak. No timing-isolation or parity claim.'), indent=2, allow_nan=False))


if __name__ == '__main__':
    main()
