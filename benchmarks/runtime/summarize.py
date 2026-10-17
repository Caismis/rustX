#!/usr/bin/env python3
"""Summarize retained runtime samples without discarding failures or slow values."""
import argparse
import json
from pathlib import Path
import statistics


def cpu_ticks(sample):
    raw = sample["stat"]
    if raw is None:
        return None
    fields = raw.rsplit(")", 1)[1].split()
    return int(fields[11]) + int(fields[12])


def proc_value(sample, source, key):
    raw = sample[source]
    if raw is None:
        return None
    for line in raw.splitlines():
        if line.startswith(key + ":"):
            return int(line.split()[1])
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    ticks_per_second = int(json.loads((args.directory / "manifest.json").read_text())["clock_ticks"]["stdout"])
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
        starting = sample["samples"][sample["history"] - 1]["process"] if sample["history"] else sample["initial_process"]
        ending = measured[-1]["process"]
        ticks_start, ticks_end = cpu_ticks(starting), cpu_ticks(ending)
        writes_start, writes_end = (proc_value(p, "io", "write_bytes") for p in (starting, ending))
        rss = [proc_value(s["process"], "smaps_rollup", "Rss") for s in sample["samples"]]
        pss = [proc_value(s["process"], "smaps_rollup", "Pss") for s in sample["samples"]]
        publications = [s["first_consumed_publication_us"] for s in measured if s["first_consumed_publication_us"] is not None]
        grouped.setdefault((sample["case"], process["label"]), []).append({
            "measured_cpu_us_per_turn":None if ticks_start is None or ticks_end is None else (ticks_end-ticks_start)*1e6/ticks_per_second/len(measured),
            "measured_process_write_bytes":None if writes_start is None or writes_end is None else writes_end-writes_start,
            "sampled_max_rss_kib":max((v for v in rss if v is not None), default=None),
            "sampled_max_pss_kib":max((v for v in pss if v is not None), default=None),
            "first_consumed_publication_median_us":statistics.median(publications) if publications else None,
            "final_writer_close_us":sample.get("final_writer_close_us"),
            "checkpoint_logical_bytes":sum((f["logical"] or 0) for f in sample.get("checkpoint_footprint", [])),
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
