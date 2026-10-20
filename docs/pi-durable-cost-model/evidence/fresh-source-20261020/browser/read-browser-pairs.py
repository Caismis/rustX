#!/usr/bin/env python3
"""Read immutable v3 browser products; never launches a measured product."""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path
import statistics


def sha(path):
    return hashlib.file_digest(path.open('rb'), 'sha256').hexdigest()


def proc(value):
    stat = value.get('stat')
    fields = stat.rsplit(')', 1)[1].split() if stat else None
    io = dict(line.split(':', 1) for line in value['io'].splitlines()) if value.get('io') else {}
    mem = dict(line.split(':', 1) for line in value['smaps_rollup'].splitlines() if ':' in line) if value.get('smaps_rollup') else {}
    return {'cpu_ms': (int(fields[11]) + int(fields[12])) * 10 if fields else None,
            'write_mib': int(io['write_bytes']) / 2**20 if 'write_bytes' in io else None,
            **{name: int(mem[field].split()[0]) / 1024 if field in mem else None
               for name, field in [('rss_mib', 'Rss'), ('pss_mib', 'Pss'), ('anon_pss_mib', 'Pss_Anon')]}}


def delta(before, after, field):
    first, last = proc(before)[field], proc(after)[field]
    return None if first is None or last is None else last - first


def mean(values):
    values = list(values)
    return statistics.mean(values) if values and all(x is not None for x in values) else None


def browser_delta(before, after):
    a = {(r['id'], r['type']): r['cpuTime'] for r in before}
    b = {(r['id'], r['type']): r['cpuTime'] for r in after}
    # An exited process's final CPU is unavailable. Never assign zero to it.
    if not a or not b or a.keys() != b.keys() or any(b[k] < a[k] for k in a):
        return None
    return sum(b[k] - a[k] for k in a) * 1000


def browser_proc_window(before, after, field):
    def identity(row):
        stat = row.get('stat')
        return (row['pid'], stat.rsplit(')', 1)[1].split()[19]) if stat else None
    a, b = {identity(r): r for r in before}, {identity(r): r for r in after}
    if not a or not b or None in a or None in b or a.keys() != b.keys():
        return None
    values = [delta(a[k], b[k], field) for k in a]
    return sum(values) if all(x is not None and x >= 0 for x in values) else None


def metrics(sample, run):
    assert sample['oracle'] == 'passed' and sample['performance_eligible'] is False
    assert sample['inspector_workload_version'] == sample['complete']['workload_version'] == 3
    assert sample['complete']['oracle'] == 'passed'
    assert sample['model_time'] == sample['complete']['model_time'] == '2026-10-19 00:00:00'
    assert sample['source'] == run['native_build']['head']
    assert sample['native_binary_sha256'] == run['host_build']['binary_sha256']
    assert not sample['errors'] and not sample['failures']
    turns = [s for s in sample['samples'] if s['measured']]
    assert len(turns) == (1 if sample['case'] == 'details-resync' else 32)
    assert len(sample['samples']) == {'short-small': 40, 'short-large': 96, 'details-resync': 1}[sample['case']]
    for turn in sample['samples']:
        proof = turn['full_repair_proof']
        assert proof['ready'] and proof['loaded_rows'] == proof['covered_rows'] == len(proof['loaded_ids'])
        assert not proof['missing_ids'] and not proof['pending_browser_requests']
        assert turn['full_repair_end_to_end_ms'] >= turn['accepted_and_settled_ms']
        assert math.isclose(turn['repair_tail_after_current_row_ms'], turn['full_repair_end_to_end_ms'] - turn['accepted_and_settled_ms'], abs_tol=1e-8)
    result = {
        'current_row_wall_ms': mean(t['accepted_and_settled_ms'] for t in turns),
        'wire_drain_wall_ms': mean(t['full_repair_end_to_end_ms'] for t in turns),
        'wire_tail_after_current_row_ms': mean(t['repair_tail_after_current_row_ms'] for t in turns),
        'native_current_row_cpu_ms': mean(delta(t['before'], t['after'], 'cpu_ms') for t in turns),
        'native_wire_drain_cpu_ms': mean(delta(t['before'], t['full_repair_after'], 'cpu_ms') for t in turns),
        'native_current_row_write_mib': mean(delta(t['before'], t['after'], 'write_mib') for t in turns),
        'native_wire_drain_write_mib': mean(delta(t['before'], t['full_repair_after'], 'write_mib') for t in turns),
        'cdp_current_row_browser_cpu_ms': mean(browser_delta(t['browserBefore'], t['browserAfter']) for t in turns),
        'cdp_wire_drain_browser_cpu_ms': mean(browser_delta(t['browserBefore'], t['full_repair_browser_after']) for t in turns),
        'runner_start_to_exit_wall_ms': (run['product_end_monotonic_ns'] - run['product_start_monotonic_ns']) / 1e6,
    }
    memory_points = {p['phase']: p for p in sample['memory']}
    window_before, window_after = memory_points['before_measured_turns'], memory_points['after_measured_turns']
    for field in ('cpu_ms', 'write_mib'):
        result['native_measured_window_' + field] = delta(window_before['native'], window_after['native'], field)
        result['browser_proc_measured_window_' + field] = browser_proc_window(window_before['browser'], window_after['browser'], field)
    points = sample['complete']['lifecycle']
    assert [x['phase'] for x in points] == ['before_initialization', 'after_provider_start', 'after_initialization', 'after_turns', 'after_verification', 'after_shutdown', 'after_driver_drop', 'after_final_writer_close', 'after_reopen_verification', 'after_cleanup']
    assert [x['elapsed_us'] for x in points] == sorted(x['elapsed_us'] for x in points)
    first, last = points[0], points[-1]
    result.update(native_logical_lifecycle_self_cpu_ms_through_cleanup=proc(last['process'])['cpu_ms'],
                  native_logical_lifecycle_write_mib_through_cleanup=proc(last['process'])['write_mib'],
                  native_initialization_to_cleanup_wall_ms=(last['elapsed_us'] - first['elapsed_us']) / 1000)
    for before, after in zip(points, points[1:]):
        prefix = 'native_phase_' + after['phase'] + '_'
        result[prefix + 'wall_ms'] = (after['elapsed_us'] - before['elapsed_us']) / 1000
        result[prefix + 'cpu_ms'] = delta(before['process'], after['process'], 'cpu_ms')
        result[prefix + 'write_mib'] = delta(before['process'], after['process'], 'write_mib')
    native_mem = [proc(m['native']) for m in sample['memory']]
    native_mem += [proc(p['process']) for p in points]
    native_mem += [proc(t[key]) for t in sample['samples'] for key in ('before', 'after', 'full_repair_after')]
    for field in ('rss_mib', 'pss_mib', 'anon_pss_mib'):
        result['native_max_sampled_' + field] = max((p[field] for p in native_mem if p[field] is not None), default=None)
        totals = []
        for point in sample['memory']:
            values = [proc(p)[field] for p in point['browser']]
            totals.append(sum(values) if values and all(x is not None for x in values) else None)
        result['browser_max_of_sampled_process_sum_' + field] = max((x for x in totals if x is not None), default=None)
        result['browser_memory_points_unavailable_' + field] = sum(x is None for x in totals)
    for field in ('usedSize', 'totalSize', 'embedderHeapUsedSize', 'backingStorageSize'):
        result['browser_max_sampled_js_' + field + '_mib'] = max((m['js'][field] / 2**20 for m in sample['memory'] if field in m['js']), default=None)
    result['browser_max_sampled_dom_nodes'] = max(m['dom']['nodes'] for m in sample['memory'])
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('directories', nargs='+', type=Path)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--allow-incomplete', action='store_true', help='Retain stopped cohorts, report missing pairs; never turn failures into accepted products')
    a = p.parse_args()
    a.output.mkdir(parents=True, exist_ok=False)
    records, hashes, groups, runs_out, run_costs, turn_rows, coverage = [], {}, {}, [], [], [], []
    for directory in a.directories:
        manifest_path = directory / 'cohort-manifest.json'
        manifest = json.loads(manifest_path.read_text())
        if manifest.get('completed_products') != 18 or manifest.get('stopped_on_failure'):
            assert a.allow_incomplete and manifest.get('stopped_on_failure'), 'Incomplete cohort requires explicit --allow-incomplete and stopped_on_failure'
        assert manifest['pairs'] == 3 and manifest['comparison'][0] == 'baseline'
        candidate = manifest['comparison'][1]
        assert candidate in ('geometry', 'scoped')
        values = {}
        for run in manifest['runs']:
            case, pair, label = run['case'], run['pair'], run['label']
            flagged = candidate == 'geometry' and case == 'short-small' and pair == 1 and label == 'geometry'
            accepted = run.get('exit') == 0 and run.get('accepted_by_producer') is True
            product_path = directory / f'{pair}-{case}-{label}'
            row = {'candidate': candidate, 'case': case, 'pair': pair, 'label': label,
                   'status': 'accepted' if accepted else 'failed',
                   'reported_disconnect_overlap': flagged,
                   'start_utc': run.get('product_start_utc'), 'end_utc': run.get('product_end_utc'),
                   'sample': str(product_path / ('sample.json' if accepted else 'failure.json'))}
            runs_out.append(row)
            if not accepted:
                failure_path = product_path / 'failure.json'
                if failure_path.exists():
                    hashes[str(failure_path)] = sha(failure_path)
                    failure = json.loads(failure_path.read_text())
                    for t in failure.get('samples', []):
                        turn_rows.append({k: row[k] for k in ('candidate','case','pair','label','status')} | {
                            'turn': t['turn'], 'measured': t['measured'],
                            'current_row_wall_ms': t['accepted_and_settled_ms'],
                            'wire_drain_wall_ms': t.get('full_repair_end_to_end_ms'),
                            'wire_tail_ms': t.get('repair_tail_after_current_row_ms')})
                continue
            sample_path = directory / f'{pair}-{case}-{label}' / 'sample.json'
            assert sha(sample_path) == run['sample_sha256']
            sample = json.loads(sample_path.read_text())
            assert sample['fixture_source'] == manifest['fixture_sha256']
            for phase in ('space_before', 'space_at_product_start'):
                assert all(x['usable_bytes'] > 5368709120 for x in run[phase]['paths'])
            # Explicitly retain the overlapping disconnect sample in primary results.
            values[(case, pair, label)] = metrics(sample, run)
            for metric, value in values[(case, pair, label)].items():
                run_costs.append({k: row[k] for k in ('candidate','case','pair','label','status')} | {'metric': metric, 'value': value})
            for t in sample['samples']:
                turn_rows.append({k: row[k] for k in ('candidate','case','pair','label','status')} | {
                    'turn': t['turn'], 'measured': t['measured'],
                    'current_row_wall_ms': t['accepted_and_settled_ms'],
                    'wire_drain_wall_ms': t['full_repair_end_to_end_ms'],
                    'wire_tail_ms': t['repair_tail_after_current_row_ms']})
            hashes[str(sample_path)] = sha(sample_path)
        hashes[str(manifest_path)] = sha(manifest_path)
        for case in manifest['cases']:
            for pair in range(3):
                present = [label for label in ('baseline', candidate) if (case, pair, label) in values]
                coverage.append({'candidate':candidate,'case':case,'pair':pair,'accepted_labels':present,'complete_pair':len(present)==2})
                if len(present) != 2:
                    continue
                baseline, after = values[(case, pair, 'baseline')], values[(case, pair, candidate)]
                for metric, before in baseline.items():
                    value = after[metric]
                    ratio = value / before if before is not None and before > 0 and value is not None else None
                    row = {'candidate': candidate, 'case': case, 'pair': pair, 'metric': metric,
                           'before': before, 'after': value, 'after_over_before': ratio,
                           'reported_disconnect_in_pair': candidate == 'geometry' and case == 'short-small' and pair == 1}
                    records.append(row)
                    groups.setdefault((candidate, case, metric), []).append(row)
    summaries = []
    for key, rows in sorted(groups.items()):
        assert 1 <= len(rows) <= 3
        # All three pairs remain primary. A separate sensitivity is explicitly labelled.
        for scope, subset in [('all_pairs_including_flagged', rows), ('unflagged_sensitivity', [r for r in rows if not r['reported_disconnect_in_pair']])]:
            if not subset:
                continue
            ratios = [r['after_over_before'] for r in subset]
            available = all(x is not None for x in ratios)
            positive = available and all(x > 0 for x in ratios)
            center = low = high = None
            if positive:
                logs = [math.log(x) for x in ratios]
                average = statistics.mean(logs)
                center = math.exp(average)
                if len(logs) > 1:
                    critical = {2: 6.313751514675041, 3: 2.919985580355516}[len(logs)]
                    half = critical * statistics.stdev(logs) / math.sqrt(len(logs))
                    low, high = math.exp(average - half), math.exp(average + half)
            summaries.append(dict(zip(('candidate', 'case', 'metric'), key)) | {
                'scope': scope, 'pairs': len(subset), 'flagged_pairs': sum(r['reported_disconnect_in_pair'] for r in subset),
                'interpretation': 'descriptive_one_pair_only' if len(subset)==1 else 'two_pair_sensitivity_is_not_three_pair_acceptance' if len(subset)==2 else 'three_pairs_with_environment_flag_not_clean_acceptance' if any(r['reported_disconnect_in_pair'] for r in subset) else 'three_pair_conditional_comparison',
                'ratio_pairs_available': sum(x is not None for x in ratios),
                'median_before': statistics.median(r['before'] for r in subset) if all(r['before'] is not None for r in subset) else None,
                'median_after': statistics.median(r['after'] for r in subset) if all(r['after'] is not None for r in subset) else None,
                'median_paired_ratio': statistics.median(ratios) if available else None,
                'minimum_paired_ratio': min(ratios) if available else None,
                'maximum_paired_ratio': max(ratios) if available else None,
                'geometric_mean_paired_ratio': center, 'log_t_90_lower': low, 'log_t_90_upper': high})
    for name, rows in [('paired-costs.csv', records), ('paired-summary.csv', summaries), ('runs.csv', runs_out), ('run-costs.csv', run_costs), ('all-turn-wall-times.csv', turn_rows)]:
        with (a.output / name).open('w') as f:
            writer = csv.DictWriter(f, fieldnames=list(rows[0]), lineterminator='\n')
            writer.writeheader(); writer.writerows(rows)
    (a.output / 'raw-input-sha256.json').write_text(json.dumps(hashes, indent=2) + '\n')
    (a.output / 'pair-coverage.json').write_text(json.dumps(coverage, indent=2) + '\n')
    (a.output / 'method.json').write_text(json.dumps({
        'clock_ticks': 100,
        'scope': 'Conditional Rust/Rust, planned three alternating pairs per case. Stopped cohorts retain failed products and all accepted unpaired products. Only complete pairs enter paired arithmetic. Actual n is explicit; n1 has no interval and n2 does not establish the planned three-pair acceptance. Fixed producer performance_eligible:false retained; oracle pass is not formal independent performance acceptance.',
        'wire_barrier': 'Observed wire coverage at exact latest Trace cut, no current-epoch pending browser RPC, stable generation across two rAF callbacks. Observation precedes route.send. Does not establish all React work, offscreen rendering or completed paints.',
        'cpu': 'Native self user+system /proc ticks. CDP sums only matching id/type process sets at both endpoints; process churn yields unavailable, never zero. Separate browser /proc measured-window totals require identical nonempty PID/start-time sets at memory endpoints. A transient process absent from both endpoints is unobserved. Node driver, WorkspaceHost and provider CPU are not included. Per-turn means exclude gaps between turns; separate measured-window totals include gaps and endpoint sampling overhead.',
        'writes_and_memory': 'Native process write_bytes, not device traffic. Memory is maximum of observed points, not true peak. Browser RSS sums double-count shared pages; PSS and anonymous PSS are separate. CDP CPU and /proc memory have distinct process scopes.',
        'lifecycle': 'Native cumulative counters through the last recorded after_cleanup point plus each adjacent phase, separately from UI turn boundaries. Final evidence-JSON emission and OS process-exit CPU/rusage are unmeasured. Runner start-to-exit wall includes setup/cleanup and browser container preparations. No full-exit Chrome CPU available; do not label measured-turn CPU as entire-browser lifecycle.',
        'ratios': 'Within-pair candidate/baseline. No ratio for unavailable or zero baseline. n=3 log-t90 uses df2 critical2.919985580355516; n=2 sensitivity df1 critical6.313751514675041. Approximately independent normal log ratios assumed; no general population guarantee.',
        'disconnect': 'Coordinator reported 2026-10-20 03:18 UTC; local exec-server transport disconnected and recovered while geometry short-small pair1 ran. Actual timing impact uncertain. All pairs retained as primary, explicitly separate unflagged sensitivity; no replacement product.',
    }, indent=2) + '\n')
    print(json.dumps({'products': len(runs_out), 'paired_metrics': len(records), 'output': str(a.output)}))


if __name__ == '__main__':
    main()
