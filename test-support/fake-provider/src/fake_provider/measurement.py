"""Opt-in local scalar measurement; no request payloads or credential values.

Three fixed windows and cumulative retained-input sizes are the complete owned
workset. Parsing/matching are timed at their existing call sites, never repeated
for measurement. Logical JSON bytes do not estimate the parsed Python heap.
"""

from __future__ import annotations

import json
import os
import sys
import time
from typing import Any


def stamp() -> tuple[int, int]:
    return time.perf_counter_ns(), time.process_time_ns()


def elapsed(start: tuple[int, int], phase: str) -> dict[str, int]:
    wall, cpu = stamp()
    return {f"{phase}_wall_ns": wall - start[0], f"{phase}_cpu_ns": cpu - start[1]}


class Measurement:
    def __init__(self, turns: int) -> None:
        self.turns = turns
        self.windows = [
            ("early", 0, 9, {}),
            ("middle", turns // 2 - 5, turns // 2 + 4, {}),
            ("late", turns - 10, turns - 1, {}),
        ]
        self.retained_text_bytes = 0
        self.retained_json_logical_bytes = 0
        self.retained_requests = 0

    @staticmethod
    def configured() -> Measurement | None:
        if os.environ.get("RUSTX_FAKE_PROVIDER_PROFILE") != "1":
            return None
        turns = int(os.environ.get("RUSTX_MEASUREMENT_TURNS", "300"))
        if turns not in (100, 300, 1000):
            raise ValueError("provider measurement requires 100, 300 or 1000 turns")
        return Measurement(turns)

    def retain(self, text: str, json_value: Any, original_bytes: int) -> None:
        self.retained_requests += 1
        self.retained_text_bytes += sys.getsizeof(text)
        if isinstance(json_value, dict):
            self.retained_json_logical_bytes += original_bytes

    def sample(self, index: int, values: dict[str, int]) -> None:
        retained = {
            "retained_requests": self.retained_requests,
            "retained_text_owned_bytes": self.retained_text_bytes,
            "retained_json_logical_bytes": self.retained_json_logical_bytes,
        }
        print(json.dumps({
            "kind": "provider_request_measurement", "turns": self.turns,
            "request": index + 1, **values, **retained,
        }, separators=(",", ":")), file=sys.stderr, flush=True)
        for name, first, last, totals in self.windows:
            if not first <= index <= last:
                continue
            totals["samples"] = totals.get("samples", 0) + 1
            for key, value in values.items():
                totals[key] = totals.get(key, 0) + value
            if index == last:
                print(json.dumps({
                    "kind": "provider_measurement", "turns": self.turns,
                    "window": name, "first_request": first + 1, "last_request": last + 1,
                    **totals, **retained,
                }, separators=(",", ":")), file=sys.stderr, flush=True)
