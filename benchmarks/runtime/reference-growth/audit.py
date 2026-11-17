#!/usr/bin/env python3
"""Audit completed raw reference runs without executing either product again."""
import argparse
import hashlib
import json
from pathlib import Path
import sqlite3
import statistics

from run import quantile, write


def main(a):
    streams = {}
    products = []
    for root in a.runs:
        identity = json.loads((root / 'identity.json').read_text())
        combined = json.loads((root / 'result.json').read_text())
        assert combined['accepted']
        for name, expected in identity['inputs'].items():
            # summarize.py/audit.py are derived-output tools and may be added
            # after measurements. Measured provider/worker/controller must match.
            if Path(name).name in ['run.py', 'provider.py', 'pi-worker.mjs', 'pi-imports.mjs']:
                assert hashlib.sha256(Path(name).read_bytes()).hexdigest() == expected, name
        for item in combined['results']:
            engine, kind = item['engine'], item['scenario']
            path = root / f'{engine}-{kind}'
            assert item == json.loads((path / 'result.json').read_text())
            rows = json.loads((path / 'turns.json').read_text())
            assert [row['turn'] for row in rows] == list(range(1, identity['turns'] + 1))
            provider = json.loads((path / 'provider.json').read_text())
            assert provider['ok'] and not provider['failures']
            assert provider['requestCount'] == len(rows) * (2 if kind == 'tool' else 1)
            if kind in streams:
                assert streams[kind] == provider['wire_chunks']
            streams[kind] = provider['wire_chunks']
            for window in item['windows']:
                subset = rows[window['turn_start'] - 1:window['turn_end']]
                assert len(subset) == window['turns']
                values = [r['settled_ms'] for r in subset]
                assert statistics.mean(values) == window['wall_ms_per_turn']
                assert quantile(values, .5) == window['wall_p50_ms']
                assert quantile(values, .95) == window['wall_p95_ms']
                for key in ['cpu_ms', 'wchar', 'write_bytes']:
                    delta = window['after'][key] - window['before'][key]
                    assert delta >= 0 and delta / len(subset) == window[key + '_per_turn']
                assert window['after']['session_bytes'] == sum(f['logical_bytes'] for f in window['after']['session_files'])
            if engine == 'pi-durable':
                db = path / 'storage/conversation.db'
                with sqlite3.connect(f'file:{db.resolve()}?mode=ro&immutable=1', uri=True) as connection:
                    assert connection.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
                    assert connection.execute('PRAGMA page_size').fetchone()[0] == 4096
            products.append({'run': root.name, 'engine': engine, 'scenario': kind,
                             'turns': len(rows), 'requests': provider['requestCount'], 'windows': len(item['windows']),
                             'reopens': [r['history_turns'] for r in item['reopen']]})
    write(a.output, {'accepted': True, 'products': products, 'products_count': len(products),
                    'turns': sum(p['turns'] for p in products),
                    'requests': sum(p['requests'] for p in products), 'same_sse_chunks_across_all_runs': True,
                    'measured_source_hashes_unchanged': True, 'sqlite_integrity_checks': 'passed'})


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runs', nargs='+', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    main(parser.parse_args())
