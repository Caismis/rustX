"""Recompute the published paired medians without unpacking an archive."""
from pathlib import Path
import csv
import json
import statistics

root = Path(__file__).resolve().parent
data = json.loads((root / 'paired-samples.json').read_text())
with (root / 'paired-samples.csv').open(newline='') as stream:
    rows = list(csv.DictReader(stream))
assert len(rows) == 162
for row in rows:
    pair = data['samples'][row['case']][int(row['pair'])]
    key = row['metric']
    before, after = float(row['before']), float(row['after'])
    assert before == pair['before'][key] and after == pair['after'][key]
    assert after / before == float(row['after_over_before'])
    assert after / before == pair['after_over_before'][key]
for case, pairs in sorted(data['samples'].items()):
    assert len(pairs) == 3
    for side in ('before', 'after', 'after_over_before'):
        for key, expected in data['medians'][case][side].items():
            actual = statistics.median(pair[side][key] for pair in pairs)
            assert actual == expected, (case, side, key, actual, expected)
print('PASS: 18 pairs, 162 ratios, all before/after and paired-ratio medians match')
