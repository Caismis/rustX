#!/usr/bin/env python3
"""Summarize retained runtime samples without discarding failures or slow values."""
import argparse
import json
from pathlib import Path
import statistics


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    grouped = {}
    failed = []
    for path in sorted(args.directory.glob("*.process.json")):
        process = json.loads(path.read_text())
        if process["returncode"]:
            failed.append(process)
            continue
        sample_path = path.with_name(path.name.replace(".process.json", ".json"))
        sample = json.loads(sample_path.read_text())
        assert sample["oracle"] == "passed"
        measured = [s for s in sample["samples"] if s["measured"]]
        grouped.setdefault((sample["case"], process["label"]), []).append({
            "pair":process["pair"], "process_seconds":process["elapsed_seconds"],
            "turn_median_us":statistics.median(s["settled_us"] for s in measured),
            "turn_max_us":max(s["settled_us"] for s in measured),
            "accepted_median_us":statistics.median(s["accepted_us"] for s in measured),
            "open_us":sample["open_us"], "shutdown_us":sample["shutdown_us"],
            "close_us":sample["close_us"],"cleanup_us":sample["cleanup_us"],
            "reopen_us":sample["reopen_us"],
            "max_live_logical_bytes":max(sum((f["logical"] or 0) for f in s["footprint"]) for s in sample["samples"]),
            "max_live_allocated_bytes":max(sum((f["allocated"] or 0) for f in s["footprint"]) for s in sample["samples"]),
            "last_live_wal_bytes":sum((f["logical"] or 0) for f in sample["before_close"] if f["path"].endswith('-wal')),
        })
    print(json.dumps({"groups":[{"case":c,"label":l,"runs":runs} for (c,l),runs in grouped.items()],
                      "failed":failed},indent=2))


if __name__ == "__main__":
    main()
