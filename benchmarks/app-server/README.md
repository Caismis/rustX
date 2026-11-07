# App Server process measurements

Run from the repository root with the locked provider emulator installed:

```sh
cargo build --bins --all-features
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 node benchmarks/app-server/measure.mts /tmp/app-server-measurements.json
```

The committed [raw measurements](measurements.json) were collected on Linux x86_64
using a debug all-features binary. Each mode starts a fresh real App Server and
real HTTP provider emulator. Setup/shutdown are excluded from process-counter
deltas. Twenty sequential explicit queued inputs are each observed in a distinct
settled native Attempt. Host modes execute one application tool and one model
continuation per input. All clients subscribe to every addressed Thread; the
three-Session mode uses round-robin input. Names prevent unrelated title generation.

The paused-client mode stops reading through a transparent TCP proxy after
subscription. This sample can fit in kernel buffers; it is not a saturated-output
proof. Deterministic bounded-queue and physical carrier tests own overflow and
isolation correctness. A single run of 20 samples does not establish a latency
guarantee, statistical significance or zero overhead.

This is a non-isolated developer host. The recorded run began after this task’s
compilation and deterministic test processes completed. An earlier exploratory
run overlapped protocol generation and is not used for the comparison below.
There is no claim about production/release performance or causation for individual
latency differences. Process CPU sums threads; RSS is a point sample; VmHWM includes
setup. `/proc/<pid>/io` write counters are kernel accounting, not physical-device I/O.

| Mode | Native Turns / model requests | CPU ms | RSS after / peak MiB | Write calls | Kernel written MiB | Latency p50 / p95 ms |
| --- | --- | --- | --- | --- | --- | --- |
| no-host-tools | 20 / 20 | 4750 | 317.2 / 317.2 | 30545 | 61.9 | 176.8 / 213.3 |
| host-one-client | 20 / 40 | 17820 | 337.3 / 337.3 | 70605 | 143.0 | 622.3 / 711.7 |
| host-three-clients | 20 / 40 | 23740 | 341.2 / 341.2 | 70555 | 143.2 | 755.7 / 988.0 |
| host-three-clients-one-paused | 20 / 40 | 22070 | 343.2 / 343.2 | 70665 | 143.1 | 634.2 / 747.8 |
| host-three-resident-sessions | 20 / 40 | 23520 | 354.6 / 354.6 | 60036 | 123.1 | 596.2 / 679.3 |

Raw data includes every latency sample, before/after RSS, write characters,
elapsed wall time, kernel/CPU information, base revision, measured source digest
and binary SHA-256. `workingTree: true` records that the implementation was
measured before its commit. The source digest hashes tracked measured paths and
their current bytes; the recorded base SHA alone does not identify the binary.
The captured source digest was verified against implementation commit
`fbcf66f66d0c199a61bb5100f1182bb82bf9f753` before later documentation and browser
assertion corrections. Those corrections change no measured execution behavior.

The comparable Host modes all issue exactly 40 model requests for 20 Turns.
Tool continuations and extra subscribers do not add native Turns or model calls.
The no-Host mode has a different workload (20 model requests), so its lower cost
must not be interpreted as the isolated cost of callback arbitration.
