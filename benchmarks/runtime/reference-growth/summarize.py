#!/usr/bin/env python3
"""Recompute comparison tables and slopes from accepted raw runs."""
import argparse
from collections import defaultdict
import csv
import json
from pathlib import Path
import statistics

from run import regress, write

METRICS = ['cpu_ms_per_turn', 'wall_ms_per_turn', 'wall_p50_ms', 'wall_p95_ms',
           'wchar_per_turn', 'write_bytes_per_turn', 'rss_bytes', 'pss_bytes',
           'session_bytes', 'session_allocated_bytes', 'sqlite_main_bytes', 'sqlite_wal_bytes',
           'provider_body_bytes_per_turn', 'wchar_minus_provider_body_bytes_per_turn']
FIT = ['cpu_ms_per_turn', 'wall_ms_per_turn', 'wchar_per_turn', 'write_bytes_per_turn',
       'rss_bytes', 'pss_bytes', 'session_bytes', 'provider_body_bytes_per_turn',
       'wchar_minus_provider_body_bytes_per_turn']


def distribution(values):
    return {'median': statistics.median(values), 'min': min(values), 'max': max(values), 'values': values}


def main(a):
    groups = defaultdict(list)
    all_rows = []
    for directory in a.runs:
        result = json.loads((directory / 'result.json').read_text())
        assert result['accepted']
        for item in result['results']:
            assert item['accepted']
            engine, scenario = item['engine'], item['scenario']
            requests = json.loads((directory / f'{engine}-{scenario}' / 'provider.json').read_text())['requests']
            count = 2 if scenario == 'tool' else 1
            first_in_generation = set()
            windows = []
            for raw in item['windows']:
                row = {k: v for k, v in raw.items() if k not in ('before', 'after')}
                subset = requests[(row['turn_start'] - 1) * count:row['turn_end'] * count]
                row['provider_body_bytes_per_turn'] = sum(r['body_bytes'] for r in subset) / row['turns']
                row['wchar_minus_provider_body_bytes_per_turn'] = row['wchar_per_turn'] - row['provider_body_bytes_per_turn']
                generation = row['process_generation']
                row['first_window_in_process'] = generation not in first_in_generation
                first_in_generation.add(generation)
                windows.append(row)
                all_rows.append({'run': directory.name, 'engine': engine, 'scenario': scenario, **row})
            steady = [r for r in windows if not r['first_window_in_process']]
            late = [r for r in steady if r['process_generation'] == 3]
            full_turns = json.loads((directory / f'{engine}-{scenario}' / 'turns.json').read_text())
            wall = sorted(row['settled_ms'] for row in full_turns)
            groups[(engine, scenario)].append({'run': directory.name, 'windows': windows,
                'slopes_all_windows': regress(windows, FIT), 'slopes_steady_windows': regress(steady, FIT),
                'slopes_generation_3_steady': regress(late, FIT) if len(late) > 1 else {},
                'whole_run': {'mean_wall_ms': statistics.mean(wall), 'p50_wall_ms': wall[len(wall)//2],
                              'p95_wall_ms': wall[min(len(wall)-1, int(len(wall)*.95))],
                              **{k: sum(row[k] * row['turns'] for row in windows)/len(wall)
                                 for k in ['cpu_ms_per_turn', 'wchar_per_turn', 'write_bytes_per_turn']}},
                'reopen': [{'history_turns': row['history_turns'], 'startup_wall_ms': row['startup_wall_ms'],
                            'startup_cpu_ms': row['loaded']['cpu_ms'],
                            'rss_bytes': row['loaded']['rss_bytes'], 'pss_bytes': row['loaded']['pss_bytes'],
                            'open_and_subscribe_ms': row['info'].get('open_and_subscribe_ms'),
                            'first_post_reopen_turn_ms': next((turn['settled_ms'] for turn in full_turns
                                if turn['turn'] == row['history_turns'] + 1), None)}
                           for row in item['reopen']],
                'closed_session_bytes': item['closed_footprint']['session_bytes']})
    output = {'accepted': True, 'runs': [str(p) for p in a.runs], 'groups': []}
    for (engine, scenario), records in sorted(groups.items()):
        assert all(len(r['windows']) == len(records[0]['windows']) for r in records)
        windows = []
        for index, first in enumerate(records[0]['windows']):
            windows.append({**{k: first[k] for k in ['turn_start', 'turn_end', 'turn_midpoint', 'process_generation', 'first_window_in_process']},
                **{key: distribution([r['windows'][index][key] for r in records]) for key in METRICS}})
        slopes = {}
        for fit in ['slopes_all_windows', 'slopes_steady_windows', 'slopes_generation_3_steady']:
            slopes[fit] = {key: distribution([r[fit][key]['slope_per_history_turn'] for r in records]) for key in FIT}
        cold = []
        for index, first in enumerate(records[0]['reopen']):
            cold.append({'history_turns': first['history_turns'],
                         **{k: distribution([r['reopen'][index][k] for r in records])
                            for k in ['startup_wall_ms', 'startup_cpu_ms', 'rss_bytes', 'pss_bytes']},
                         'open_and_subscribe_ms': distribution([r['reopen'][index]['open_and_subscribe_ms'] for r in records])
                             if first['open_and_subscribe_ms'] is not None else None,
                         'first_post_reopen_turn_ms': distribution([r['reopen'][index]['first_post_reopen_turn_ms'] for r in records])
                             if first['first_post_reopen_turn_ms'] is not None else None})
        output['groups'].append({'engine': engine, 'scenario': scenario, 'repetitions': len(records),
            'windows': windows, 'slopes': slopes, 'reopen': cold,
            'whole_run': {key: distribution([r['whole_run'][key] for r in records]) for key in records[0]['whole_run']},
            'closed_session_bytes': distribution([r['closed_session_bytes'] for r in records]), 'per_run': records})
    a.output.mkdir(parents=True, exist_ok=True)
    write(a.output / 'summary.json', output)
    with (a.output / 'windows.csv').open('w') as file:
        writer = csv.DictWriter(file, fieldnames=list(all_rows[0]))
        writer.writeheader()
        writer.writerows(all_rows)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runs', nargs='+', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    main(parser.parse_args())
