# Native latency/CPU repair and128-session memory values

Fresh Rust cohort: four balanced triads,12ordinary native products,128sessions each with8seedturns, then16rounds at widths1/4/8. All pairs and outliers retained. Legacy=1039/schema93/4KiB; previous=b0ac/schema94/2KiB; fixed=e1d4/schema94/2KiB plus TCP_NODELAY/catalog work reuse.

## Ordinary activity medians

### Active width 1

| Side | Settled median ms | Accepted median ms | Product P95 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- |
| legacy | 36.73297 | 8.06680 | 63.20189 | 16.56250 | 910.37500 |
| previous | 23.05790 | 7.55383 | 59.51103 | 17.81250 | 685.12500 |
| fixed | 10.77506 | 1.08034 | 16.29911 | 12.18750 | 641.12500 |

| Fixed versus | Metric | Pair0 % | Pair1 % | Pair2 % | Pair3 % | Paired median % |
| --- | --- | --- | --- | --- | --- | --- |
| legacy | settled_ms | -87.09298 | -80.75362 | -44.04768 | 323.15023 | -62.40065 |
| legacy | accepted_ms | -96.38077 | -89.50150 | -83.08012 | 564.48917 | -86.29081 |
| legacy | p95_ms | -94.25940 | -78.32816 | -70.29727 | 358.36594 | -74.31271 |
| legacy | p99_ms | -94.25940 | -78.32816 | -70.29727 | 358.36594 | -74.31271 |
| legacy | cpu_ms | -28.00000 | -44.11765 | -28.57143 | 37.50000 | -28.28571 |
| legacy | write_kib | -29.56355 | -29.81928 | -29.51045 | -28.07692 | -29.53700 |
| previous | settled_ms | -81.16887 | -58.28184 | -42.68251 | 210.42947 | -50.48217 |
| previous | accepted_ms | -88.27870 | -86.89468 | -85.03927 | 639.96476 | -85.96697 |
| previous | p95_ms | -77.16344 | -76.21295 | -69.16554 | 364.51475 | -72.68924 |
| previous | p99_ms | -77.16344 | -76.21295 | -69.16554 | 364.51475 | -72.68924 |
| previous | cpu_ms | -47.05882 | -32.14286 | -31.03448 | 22.22222 | -31.58867 |
| previous | write_kib | -6.48688 | -6.45985 | -6.45985 | -4.48741 | -6.45985 |

### Active width 4

| Side | Settled median ms | Accepted median ms | Product P95 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- |
| legacy | 48.13019 | 15.45791 | 76.86214 | 13.67188 | 941.84375 |
| previous | 40.71444 | 15.32952 | 77.32482 | 13.98438 | 710.78125 |
| fixed | 29.75413 | 5.33629 | 37.43421 | 11.95312 | 667.06250 |

| Fixed versus | Metric | Pair0 % | Pair1 % | Pair2 % | Pair3 % | Paired median % |
| --- | --- | --- | --- | --- | --- | --- |
| legacy | settled_ms | -58.67895 | -23.73103 | -19.22564 | -36.82524 | -30.27813 |
| legacy | accepted_ms | -69.47212 | -73.31139 | -68.92197 | -4.53284 | -69.19704 |
| legacy | p95_ms | -94.96306 | -56.36730 | -45.37542 | 526.51654 | -50.87136 |
| legacy | p99_ms | -98.87710 | -57.25904 | -45.47617 | 565.97583 | -51.36761 |
| legacy | cpu_ms | -21.64948 | -17.04545 | -11.49425 | 3.44828 | -14.26985 |
| legacy | write_kib | -29.21445 | -29.16722 | -29.13511 | -29.17580 | -29.17151 |
| previous | settled_ms | -29.02961 | -29.26510 | -49.82110 | 0.12705 | -29.14736 |
| previous | accepted_ms | -61.79569 | -75.19744 | -59.78533 | -31.41744 | -60.79051 |
| previous | p95_ms | -54.99091 | -56.28008 | -48.24113 | 544.53683 | -51.61602 |
| previous | p99_ms | -56.46157 | -55.86155 | -47.64053 | 589.66036 | -51.75104 |
| previous | cpu_ms | -14.60674 | -18.88889 | -13.48315 | 0.00000 | -14.04494 |
| previous | write_kib | -6.16047 | -6.18249 | -6.07501 | -6.17632 | -6.16839 |

### Active width 8

| Side | Settled median ms | Accepted median ms | Product P95 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- |
| legacy | 66.09771 | 23.11871 | 107.42290 | 13.35938 | 975.75000 |
| previous | 64.48456 | 22.72744 | 107.79863 | 13.35938 | 737.26562 |
| fixed | 57.57082 | 10.23402 | 75.14187 | 11.79688 | 692.89062 |

| Fixed versus | Metric | Pair0 % | Pair1 % | Pair2 % | Pair3 % | Paired median % |
| --- | --- | --- | --- | --- | --- | --- |
| legacy | settled_ms | -7.75872 | -17.67867 | -15.00602 | -7.51917 | -11.38237 |
| legacy | accepted_ms | -46.81154 | -58.83207 | -56.66519 | -56.01126 | -56.33822 |
| legacy | p95_ms | 1141.81493 | -38.94195 | -38.71275 | -21.61248 | -30.16261 |
| legacy | p99_ms | 1075.95970 | -39.85383 | -41.69640 | -10.97730 | -25.41556 |
| legacy | cpu_ms | -1.12994 | -11.69591 | -12.86550 | -11.69591 | -11.69591 |
| legacy | write_kib | -28.99372 | -28.97080 | -28.99372 | -28.94087 | -28.98226 |
| previous | settled_ms | 0.56284 | -18.18232 | -12.87586 | -8.57060 | -10.72323 |
| previous | accepted_ms | -46.47956 | -57.80735 | -51.09341 | -57.75145 | -54.42243 |
| previous | p95_ms | 1179.94515 | -39.27465 | -37.37185 | -25.15320 | -31.26252 |
| previous | p99_ms | 1096.55007 | -40.97733 | -38.69435 | -11.41532 | -25.05483 |
| previous | cpu_ms | 5.42169 | -13.71429 | -12.35294 | -12.20930 | -12.28112 |
| previous | write_kib | -5.94349 | -6.00076 | -6.01128 | -6.02645 | -6.00602 |

## Ordinary memory endpoints

| Phase | Side | Anon PSS MiB | RSS MiB | Four anon values |
| --- | --- | --- | --- | --- |
| empty_server | legacy | 14.19727 | 46.13867 | 14.19531, 14.19922, 14.19922, 14.19531 |
| empty_server | previous | 14.20508 | 46.07422 | 14.20312, 14.29297, 14.20703, 14.20312 |
| empty_server | fixed | 14.19531 | 46.02539 | 14.19531, 14.19531, 14.19531, 14.19141 |
| empty_attached_128 | legacy | 336.67188 | 390.86719 | 336.69531, 336.17188, 337.86719, 336.64844 |
| empty_attached_128 | previous | 329.42773 | 383.42969 | 329.73047, 329.52734, 329.28516, 329.32812 |
| empty_attached_128 | fixed | 323.41992 | 377.86523 | 323.34766, 322.95703, 323.62891, 323.49219 |
| all_subscribed_empty | legacy | 337.08398 | 392.52930 | 337.08594, 336.56641, 338.07422, 337.08203 |
| all_subscribed_empty | previous | 329.61523 | 384.71289 | 329.87109, 329.71484, 329.47656, 329.51562 |
| all_subscribed_empty | fixed | 323.67578 | 379.32031 | 323.60547, 323.08984, 323.82031, 323.74609 |
| seeded_history_8 | legacy | 925.60742 | 989.29297 | 924.98047, 921.02734, 926.23438, 927.41797 |
| seeded_history_8 | previous | 780.86719 | 843.92383 | 779.28906, 782.44531, 778.44922, 784.70703 |
| seeded_history_8 | fixed | 766.86719 | 830.14648 | 769.74219, 764.10938, 766.15234, 767.58203 |
| all_unloaded | legacy | 1016.54688 | 1077.26172 | 1015.87109, 1013.90625, 1018.84766, 1017.22266 |
| all_unloaded | previous | 849.82031 | 910.10156 | 848.46094, 851.00391, 848.63672, 855.85547 |
| all_unloaded | fixed | 840.20898 | 900.83203 | 845.42578, 836.55469, 838.81641, 841.60156 |
| reload_unload_cycle_2 | legacy | 1016.56055 | 1077.43164 | 1015.88672, 1013.91406, 1018.87891, 1017.23438 |
| reload_unload_cycle_2 | previous | 849.84375 | 910.21484 | 848.47656, 851.02734, 848.66016, 856.14844 |
| reload_unload_cycle_2 | fixed | 840.25977 | 901.07031 | 845.44141, 836.59766, 838.85938, 841.66016 |

## Broader lifecycle counters

| Side | Empty→all-unloaded CPU ms | Four values ms | Process writes MiB |
| --- | --- | --- | --- |
| legacy | 25390.00000 | 25390.00000, 25900.00000, 25390.00000, 24640.00000 | 1279.26758 |
| previous | 24420.00000 | 24980.00000, 24520.00000, 24320.00000, 24230.00000 | 892.33008 |
| fixed | 21285.00000 | 21630.00000, 21010.00000, 20870.00000, 21560.00000 | 831.95312 |

This window includes creation, seeding, active probes, history/Trace, slow-reader and cancellation work, followed by a zero-loaded/loading/unloading barrier. It is not per-turn chat CPU.

## Memory-only Pi128×8 sample

| Phase | Anon PSS MiB | RSS MiB | Open SQLite files | Node heapUsed MiB | Node heapTotal MiB |
| --- | --- | --- | --- | --- | --- |
| before_imports | 10.28516 | 51.62500 | 0 | 5.26516 | 6.24609 |
| after_imports | 64.92578 | 118.98828 | 0 | 21.23228 | 34.59766 |
| empty_attached_1 | 65.24219 | 120.93750 | 1 | 21.90089 | 34.84766 |
| empty_attached_8 | 65.58203 | 121.49609 | 8 | 23.64169 | 35.34766 |
| empty_attached_32 | 72.67188 | 129.33594 | 32 | 24.22614 | 37.09766 |
| empty_attached_64 | 81.64453 | 139.30859 | 64 | 24.84122 | 53.59766 |
| empty_attached_128 | 103.49219 | 163.15625 | 128 | 26.38601 | 54.09766 |
| all_subscribed_empty | 105.47656 | 165.14062 | 128 | 29.62076 | 54.59766 |
| seeded_history_1 | 181.80469 | 245.08594 | 128 | 41.46912 | 102.86328 |
| seeded_history_8 | 310.46484 | 373.74609 | 128 | 109.53296 | 217.62109 |
| all_closed | 311.25391 | 370.65625 | 0 | 139.23567 | 218.12109 |
| reopen_verified | 314.81641 | 374.21875 | 0 | 124.66784 | 221.62109 |

One authorized Pi memory-only sample,128independent workspaces/Harness/SQLite files and128native watchEvents.1024HTTPrequest full histories and native/API records verified; reopen checks executed by the saved Node script after the authoritative seed memory point. Ordinary Node allocator, no forcedGC. No Pi latency/CPU/write benchmark. Compare raw native PID RSS/anonymousPSS endpoints, with library-versus-AppServer and tool/inspection capability limits.

Fixed Rust / Pi at seeded8: {'anon_pss_mib': 2.4700612740472327, 'rss_mib': 2.2211509317614104}. These are count/history-matched process endpoints, not full capability equivalence. Pi is one sample; no new Pi latency, CPU or write benchmark.

## Controlled TCP experiment, same saved executable

| Product | Settled median ms | CPU ms/turn | Write KiB/turn |
| --- | --- | --- | --- |
| current-plain | 20.25283 | 18.43750 | 736.87500 |
| current-nodelay | 10.88212 | 13.75000 | 737.75000 |
| baseline-nodelay | 11.41558 | 13.43750 | 970.75000 |
| baseline-plain | 13.59427 | 16.56250 | 972.87500 |

Diagnostic32-round cases. An accepted-socket-only LD_PRELOAD helper toggles TCP_NODELAY, actual target ELF SHA is unchanged, all request/native oracles pass. Not pooled into the formal cohort.

## CPU sample denominators and overlapping groups

| Engine | Raw samples | Records | Catalog activity samples | Catalog JSON samples | SQLite samples | Sample CPU ms/256attempts |
| --- | --- | --- | --- | --- | --- | --- |
| baseline | 994 | 976 | 193 | 140 | 173 | 3976 |
| current | 985 | 956 | 178 | 126 | 151 | 3940 |
| clustered | 998 | 979 | 169 | 117 | 185 | 3992 |
| fixed | 747 | 728 | 35 | 11 | 177 | 2988 |

250Hz signal samples in native child only,256rotating active attempts keep histories near8–10. Diagnostic allocator/profiler timings are separate from ordinary. Raw/collapsed/text denominators agree. Inclusive groups overlap; optimized aliases/inlining and finite samples limit attribution. No driver/provider CPU or precise phase-timing claims.

## Catalog file bytes at native close

| Side | Four logical byte sizes |
| --- | --- |
| legacy | 152016, 152013, 152016, 152016 |
| previous | 152269, 152272, 152272, 152266 |
| fixed | 109335, 109335, 109335, 109335 |

The field/value schema and atomic replacement boundaries are preserved; compact encoding reduces repeated file bytes. These sizes are not WAL or device-write counters.

## Every accepted native product

### native-0-legacy

Diagnostic False; requests 1235; native databases 128; schema [93].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.19531 | 46.25000 | 0/0/0 | 0 |
| metadata_only | 31.05078 | 73.66406 | 0/0/0 | 0 |
| empty_attached_1 | 103.91016 | 154.49219 | 1/0/0 | 0 |
| empty_attached_8 | 119.40625 | 170.39453 | 8/0/0 | 0 |
| empty_attached_32 | 163.10938 | 214.83594 | 32/0/0 | 0 |
| empty_attached_64 | 221.01562 | 273.74219 | 64/0/0 | 0 |
| empty_attached_128 | 336.69531 | 391.41016 | 128/0/0 | 0 |
| subscribed_1 | 336.87500 | 392.77734 | 128/0/0 | 0 |
| subscribed_8 | 337.04688 | 392.94922 | 128/0/0 | 0 |
| subscribed_32 | 337.05859 | 392.96094 | 128/0/0 | 0 |
| subscribed_64 | 337.06250 | 392.96484 | 128/0/0 | 0 |
| subscribed_128 | 337.08594 | 392.98828 | 128/0/0 | 0 |
| all_subscribed_empty | 337.08594 | 392.98828 | 128/0/0 | 0 |
| seeded_history_1 | 533.52344 | 597.26172 | 128/0/0 | 0 |
| seeded_history_8 | 924.98047 | 988.71875 | 128/0/0 | 0 |
| active_held_1 | 925.53906 | 989.27734 | 128/0/0 | 1 |
| active_finished_1 | 933.08594 | 996.82422 | 128/0/0 | 0 |
| active_held_4 | 934.10938 | 997.84766 | 128/0/0 | 4 |
| active_finished_4 | 960.05469 | 1023.79297 | 128/0/0 | 0 |
| active_held_8 | 964.73828 | 1028.47656 | 128/0/0 | 8 |
| active_finished_8 | 1013.25781 | 1076.99609 | 128/0/0 | 0 |
| extra_controller_connections | 1015.38672 | 1079.75000 | 128/0/0 | 0 |
| history_browsed | 1017.09375 | 1081.58203 | 128/0/0 | 0 |
| slow_consumer_held | 1019.20703 | 1083.82031 | 128/0/0 | 1 |
| slow_consumer_settled | 1019.21094 | 1083.82422 | 128/0/0 | 0 |
| all_unloaded | 1015.87109 | 1077.29688 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 1015.87891 | 1077.42969 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 1015.88672 | 1077.43750 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 1015.88672 | 1077.43750 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 74.07906 | 235.14678 | 235.14678 | 15.62500 | 910.75000 |
| 4 | 60 | 72.40408 | 685.29582 | 3199.26421 | 15.15625 | 942.81250 |
| 8 | 120 | 68.34095 | 107.24042 | 113.93833 | 13.82812 | 975.75000 |

Cancel-to-settled 19.99723 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### native-0-previous

Diagnostic False; requests 1235; native databases 128; schema [94].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.20312 | 45.97656 | 0/0/0 | 0 |
| metadata_only | 30.71484 | 73.05078 | 0/0/0 | 0 |
| empty_attached_1 | 104.21484 | 154.70703 | 1/0/0 | 0 |
| empty_attached_8 | 119.33984 | 170.17578 | 8/0/0 | 0 |
| empty_attached_32 | 161.13281 | 212.71875 | 32/0/0 | 0 |
| empty_attached_64 | 217.41797 | 270.00391 | 64/0/0 | 0 |
| empty_attached_128 | 329.73047 | 384.29688 | 128/0/0 | 0 |
| subscribed_1 | 329.80078 | 385.49219 | 128/0/0 | 0 |
| subscribed_8 | 329.87109 | 385.56250 | 128/0/0 | 0 |
| subscribed_32 | 329.87109 | 385.56250 | 128/0/0 | 0 |
| subscribed_64 | 329.87109 | 385.56250 | 128/0/0 | 0 |
| subscribed_128 | 329.87109 | 385.56250 | 128/0/0 | 0 |
| all_subscribed_empty | 329.87109 | 385.56250 | 128/0/0 | 0 |
| seeded_history_1 | 491.52344 | 554.67188 | 128/0/0 | 0 |
| seeded_history_8 | 779.28906 | 842.43750 | 128/0/0 | 0 |
| active_held_1 | 779.71875 | 842.86719 | 128/0/0 | 1 |
| active_finished_1 | 788.67578 | 851.82422 | 128/0/0 | 0 |
| active_held_4 | 789.92969 | 853.07812 | 128/0/0 | 4 |
| active_finished_4 | 806.48828 | 869.63672 | 128/0/0 | 0 |
| active_held_8 | 807.64844 | 870.79688 | 128/0/0 | 8 |
| active_finished_8 | 844.22266 | 907.37109 | 128/0/0 | 0 |
| extra_controller_connections | 844.91797 | 908.69141 | 128/0/0 | 0 |
| history_browsed | 847.07812 | 910.85156 | 128/0/0 | 0 |
| slow_consumer_held | 848.04297 | 911.94141 | 128/0/0 | 1 |
| slow_consumer_settled | 848.32422 | 912.22266 | 128/0/0 | 0 |
| all_unloaded | 848.46094 | 909.04688 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 848.47656 | 909.18750 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 848.47656 | 909.18750 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 848.47656 | 909.18750 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 50.77443 | 59.11060 | 59.11060 | 21.25000 | 686.00000 |
| 4 | 60 | 42.15579 | 76.69103 | 82.51210 | 13.90625 | 711.18750 |
| 8 | 120 | 62.68575 | 104.04568 | 111.97766 | 12.96875 | 736.62500 |

Cancel-to-settled 7.96814 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### native-0-fixed

Diagnostic False; requests 1235; native databases 128; schema [94].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.19531 | 45.87109 | 0/0/0 | 0 |
| metadata_only | 25.42188 | 68.28516 | 0/0/0 | 0 |
| empty_attached_1 | 100.37109 | 150.70312 | 1/0/0 | 0 |
| empty_attached_8 | 112.95312 | 163.69141 | 8/0/0 | 0 |
| empty_attached_32 | 155.93359 | 207.42188 | 32/0/0 | 0 |
| empty_attached_64 | 211.14453 | 263.63281 | 64/0/0 | 0 |
| empty_attached_128 | 323.34766 | 377.83594 | 128/0/0 | 0 |
| subscribed_1 | 323.43750 | 379.11328 | 128/0/0 | 0 |
| subscribed_8 | 323.56250 | 379.23828 | 128/0/0 | 0 |
| subscribed_32 | 323.57812 | 379.25391 | 128/0/0 | 0 |
| subscribed_64 | 323.60547 | 379.28125 | 128/0/0 | 0 |
| subscribed_128 | 323.60547 | 379.28125 | 128/0/0 | 0 |
| all_subscribed_empty | 323.60547 | 379.28125 | 128/0/0 | 0 |
| seeded_history_1 | 481.21875 | 544.39453 | 128/0/0 | 0 |
| seeded_history_8 | 769.74219 | 832.91797 | 128/0/0 | 0 |
| active_held_1 | 770.18359 | 833.35938 | 128/0/0 | 1 |
| active_finished_1 | 774.21094 | 837.38672 | 128/0/0 | 0 |
| active_held_4 | 774.87891 | 838.05469 | 128/0/0 | 4 |
| active_finished_4 | 792.79688 | 855.97266 | 128/0/0 | 0 |
| active_held_8 | 795.97266 | 859.14844 | 128/0/0 | 8 |
| active_finished_8 | 834.81250 | 897.98828 | 128/0/0 | 0 |
| extra_controller_connections | 838.36328 | 902.03906 | 128/0/0 | 0 |
| history_browsed | 843.08594 | 906.76172 | 128/0/0 | 0 |
| slow_consumer_held | 843.23047 | 907.03125 | 128/0/0 | 1 |
| slow_consumer_settled | 843.28125 | 907.08203 | 128/0/0 | 0 |
| all_unloaded | 845.42578 | 905.97656 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 845.44141 | 906.11719 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 845.44141 | 906.11719 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 845.44141 | 906.11719 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 9.56140 | 13.49883 | 13.49883 | 11.25000 | 641.50000 |
| 4 | 60 | 29.91813 | 34.51793 | 35.92447 | 11.87500 | 667.37500 |
| 8 | 120 | 63.03857 | 1331.72757 | 1339.86880 | 13.67188 | 692.84375 |

Cancel-to-settled 10.47632 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### native-1-fixed

Diagnostic False; requests 1235; native databases 128; schema [94].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.19531 | 46.17969 | 0/0/0 | 0 |
| metadata_only | 24.38281 | 67.42969 | 0/0/0 | 0 |
| empty_attached_1 | 98.35547 | 148.93359 | 1/0/0 | 0 |
| empty_attached_8 | 112.94141 | 163.92578 | 8/0/0 | 0 |
| empty_attached_32 | 154.77734 | 206.51172 | 32/0/0 | 0 |
| empty_attached_64 | 210.12500 | 262.83984 | 64/0/0 | 0 |
| empty_attached_128 | 322.95703 | 377.65234 | 128/0/0 | 0 |
| subscribed_1 | 322.95703 | 378.90234 | 128/0/0 | 0 |
| subscribed_8 | 323.02344 | 378.96875 | 128/0/0 | 0 |
| subscribed_32 | 323.08984 | 379.03516 | 128/0/0 | 0 |
| subscribed_64 | 323.08984 | 379.03516 | 128/0/0 | 0 |
| subscribed_128 | 323.08984 | 379.03516 | 128/0/0 | 0 |
| all_subscribed_empty | 323.08984 | 379.03516 | 128/0/0 | 0 |
| seeded_history_1 | 480.01562 | 543.50000 | 128/0/0 | 0 |
| seeded_history_8 | 764.10938 | 827.59375 | 128/0/0 | 0 |
| active_held_1 | 764.57812 | 828.06250 | 128/0/0 | 1 |
| active_finished_1 | 769.16797 | 832.65234 | 128/0/0 | 0 |
| active_held_4 | 769.98047 | 833.46484 | 128/0/0 | 4 |
| active_finished_4 | 788.51172 | 851.99609 | 128/0/0 | 0 |
| active_held_8 | 790.83203 | 854.31641 | 128/0/0 | 8 |
| active_finished_8 | 828.04297 | 891.52734 | 128/0/0 | 0 |
| extra_controller_connections | 830.98047 | 894.83984 | 128/0/0 | 0 |
| history_browsed | 834.63281 | 898.49219 | 128/0/0 | 0 |
| slow_consumer_held | 834.96484 | 898.88672 | 128/0/0 | 1 |
| slow_consumer_settled | 835.21875 | 899.14062 | 128/0/0 | 0 |
| all_unloaded | 836.55469 | 897.28906 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 836.58203 | 897.44141 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 836.59766 | 897.45703 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 836.59766 | 897.45703 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 10.25387 | 13.45864 | 13.45864 | 11.87500 | 640.75000 |
| 4 | 60 | 27.77977 | 34.23203 | 35.88286 | 11.40625 | 667.68750 |
| 8 | 120 | 54.45237 | 65.70174 | 69.04984 | 11.79688 | 693.15625 |

Cancel-to-settled 7.06206 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### native-1-previous

Diagnostic False; requests 1235; native databases 128; schema [94].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.29297 | 46.32031 | 0/0/0 | 0 |
| metadata_only | 32.22656 | 74.31641 | 0/0/0 | 0 |
| empty_attached_1 | 104.89453 | 154.32812 | 1/0/0 | 0 |
| empty_attached_8 | 119.40234 | 169.24219 | 8/0/0 | 0 |
| empty_attached_32 | 161.19141 | 211.78125 | 32/0/0 | 0 |
| empty_attached_64 | 217.05078 | 268.64062 | 64/0/0 | 0 |
| empty_attached_128 | 329.52734 | 383.09766 | 128/0/0 | 0 |
| subscribed_1 | 329.59766 | 384.29297 | 128/0/0 | 0 |
| subscribed_8 | 329.71484 | 384.41016 | 128/0/0 | 0 |
| subscribed_32 | 329.71484 | 384.41016 | 128/0/0 | 0 |
| subscribed_64 | 329.71484 | 384.41016 | 128/0/0 | 0 |
| subscribed_128 | 329.71484 | 384.41016 | 128/0/0 | 0 |
| all_subscribed_empty | 329.71484 | 384.41016 | 128/0/0 | 0 |
| seeded_history_1 | 487.71484 | 550.67969 | 128/0/0 | 0 |
| seeded_history_8 | 782.44531 | 845.41016 | 128/0/0 | 0 |
| active_held_1 | 782.45703 | 845.42188 | 128/0/0 | 1 |
| active_finished_1 | 791.25781 | 854.22266 | 128/0/0 | 0 |
| active_held_4 | 792.58984 | 855.55469 | 128/0/0 | 4 |
| active_finished_4 | 809.54297 | 872.50781 | 128/0/0 | 0 |
| active_held_8 | 812.30078 | 875.26562 | 128/0/0 | 8 |
| active_finished_8 | 844.88672 | 907.85156 | 128/0/0 | 0 |
| extra_controller_connections | 846.18359 | 909.46094 | 128/0/0 | 0 |
| history_browsed | 848.94922 | 912.28906 | 128/0/0 | 0 |
| slow_consumer_held | 849.76172 | 913.16406 | 128/0/0 | 1 |
| slow_consumer_settled | 849.89453 | 913.29688 | 128/0/0 | 0 |
| all_unloaded | 851.00391 | 911.15625 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 851.01953 | 911.23438 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 851.01953 | 911.23438 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 851.02734 | 911.24219 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 24.57892 | 56.57968 | 56.57968 | 17.50000 | 685.00000 |
| 4 | 60 | 39.27308 | 78.29848 | 81.29614 | 14.06250 | 711.68750 |
| 8 | 120 | 66.55331 | 108.19492 | 116.98868 | 13.67188 | 737.40625 |

Cancel-to-settled 8.33108 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### native-1-legacy

Diagnostic False; requests 1235; native databases 128; schema [93].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.19922 | 45.94922 | 0/0/0 | 0 |
| metadata_only | 29.70703 | 71.58203 | 0/0/0 | 0 |
| empty_attached_1 | 103.18359 | 152.96484 | 1/0/0 | 0 |
| empty_attached_8 | 119.11719 | 169.30469 | 8/0/0 | 0 |
| empty_attached_32 | 163.17578 | 214.11328 | 32/0/0 | 0 |
| empty_attached_64 | 221.07812 | 273.01562 | 64/0/0 | 0 |
| empty_attached_128 | 336.17188 | 390.08984 | 128/0/0 | 0 |
| subscribed_1 | 336.37500 | 391.41797 | 128/0/0 | 0 |
| subscribed_8 | 336.47266 | 391.51562 | 128/0/0 | 0 |
| subscribed_32 | 336.49219 | 391.53516 | 128/0/0 | 0 |
| subscribed_64 | 336.56641 | 391.60938 | 128/0/0 | 0 |
| subscribed_128 | 336.56641 | 391.60938 | 128/0/0 | 0 |
| all_subscribed_empty | 336.56641 | 391.60938 | 128/0/0 | 0 |
| seeded_history_1 | 531.36719 | 594.36719 | 128/0/0 | 0 |
| seeded_history_8 | 921.02734 | 984.02734 | 128/0/0 | 0 |
| active_held_1 | 922.94922 | 985.94922 | 128/0/0 | 1 |
| active_finished_1 | 931.60938 | 994.60938 | 128/0/0 | 0 |
| active_held_4 | 933.15234 | 996.15234 | 128/0/0 | 4 |
| active_finished_4 | 957.96484 | 1020.96484 | 128/0/0 | 0 |
| active_held_8 | 962.53516 | 1025.53516 | 128/0/0 | 8 |
| active_finished_8 | 1010.05859 | 1073.05859 | 128/0/0 | 0 |
| extra_controller_connections | 1011.66406 | 1075.10156 | 128/0/0 | 0 |
| history_browsed | 1015.28906 | 1078.72656 | 128/0/0 | 0 |
| slow_consumer_held | 1016.60938 | 1080.17188 | 128/0/0 | 1 |
| slow_consumer_settled | 1016.64062 | 1080.20312 | 128/0/0 | 0 |
| all_unloaded | 1013.90625 | 1074.28125 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 1013.91016 | 1074.34766 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 1013.91406 | 1074.35156 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 1013.91406 | 1074.35156 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 53.27688 | 62.10195 | 62.10195 | 21.25000 | 913.00000 |
| 4 | 60 | 36.42343 | 78.45500 | 83.95427 | 13.75000 | 942.62500 |
| 8 | 120 | 66.14613 | 107.60538 | 114.80339 | 13.35938 | 975.87500 |

Cancel-to-settled 11.20175 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### native-2-previous

Diagnostic False; requests 1235; native databases 128; schema [94].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.20703 | 46.03516 | 0/0/0 | 0 |
| metadata_only | 30.42188 | 72.56250 | 0/0/0 | 0 |
| empty_attached_1 | 104.16797 | 153.96484 | 1/0/0 | 0 |
| empty_attached_8 | 117.40234 | 167.60547 | 8/0/0 | 0 |
| empty_attached_32 | 160.89453 | 211.84766 | 32/0/0 | 0 |
| empty_attached_64 | 216.73438 | 268.68750 | 64/0/0 | 0 |
| empty_attached_128 | 329.28516 | 383.21094 | 128/0/0 | 0 |
| subscribed_1 | 329.42188 | 384.53516 | 128/0/0 | 0 |
| subscribed_8 | 329.47266 | 384.58594 | 128/0/0 | 0 |
| subscribed_32 | 329.47656 | 384.58984 | 128/0/0 | 0 |
| subscribed_64 | 329.47656 | 384.58984 | 128/0/0 | 0 |
| subscribed_128 | 329.47656 | 384.58984 | 128/0/0 | 0 |
| all_subscribed_empty | 329.47656 | 384.58984 | 128/0/0 | 0 |
| seeded_history_1 | 490.55078 | 553.56641 | 128/0/0 | 0 |
| seeded_history_8 | 778.44922 | 841.46484 | 128/0/0 | 0 |
| active_held_1 | 780.10547 | 843.12109 | 128/0/0 | 1 |
| active_finished_1 | 786.67188 | 849.68750 | 128/0/0 | 0 |
| active_held_4 | 787.28516 | 850.30078 | 128/0/0 | 4 |
| active_finished_4 | 807.06641 | 870.08203 | 128/0/0 | 0 |
| active_held_8 | 807.66797 | 870.68359 | 128/0/0 | 8 |
| active_finished_8 | 843.84766 | 906.86328 | 128/0/0 | 0 |
| extra_controller_connections | 844.69922 | 908.02734 | 128/0/0 | 0 |
| history_browsed | 847.91016 | 911.30078 | 128/0/0 | 0 |
| slow_consumer_held | 848.80078 | 912.25391 | 128/0/0 | 1 |
| slow_consumer_settled | 848.80078 | 912.25391 | 128/0/0 | 0 |
| all_unloaded | 848.63672 | 908.96484 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 848.65234 | 909.04297 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 848.66016 | 909.05078 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 848.66016 | 909.05078 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 19.70821 | 61.94172 | 61.94172 | 18.12500 | 685.00000 |
| 4 | 60 | 58.96925 | 77.95861 | 85.40542 | 13.90625 | 709.87500 |
| 8 | 120 | 64.43440 | 107.40234 | 112.36214 | 13.28125 | 737.15625 |

Cancel-to-settled 8.02207 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### native-2-legacy

Diagnostic False; requests 1235; native databases 128; schema [93].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.19922 | 46.14453 | 0/0/0 | 0 |
| metadata_only | 30.88281 | 73.70312 | 0/0/0 | 0 |
| empty_attached_1 | 102.85156 | 153.45312 | 1/0/0 | 0 |
| empty_attached_8 | 119.90625 | 170.91406 | 8/0/0 | 0 |
| empty_attached_32 | 163.27344 | 215.01953 | 32/0/0 | 0 |
| empty_attached_64 | 221.55078 | 274.29688 | 64/0/0 | 0 |
| empty_attached_128 | 337.86719 | 392.60156 | 128/0/0 | 0 |
| subscribed_1 | 337.93750 | 393.92188 | 128/0/0 | 0 |
| subscribed_8 | 338.00781 | 393.99219 | 128/0/0 | 0 |
| subscribed_32 | 338.07422 | 394.05859 | 128/0/0 | 0 |
| subscribed_64 | 338.07422 | 394.05859 | 128/0/0 | 0 |
| subscribed_128 | 338.07422 | 394.05859 | 128/0/0 | 0 |
| all_subscribed_empty | 338.07422 | 394.05859 | 128/0/0 | 0 |
| seeded_history_1 | 526.71875 | 590.35156 | 128/0/0 | 0 |
| seeded_history_8 | 926.23438 | 989.86719 | 128/0/0 | 0 |
| active_held_1 | 927.21094 | 990.84375 | 128/0/0 | 1 |
| active_finished_1 | 937.50391 | 1001.13672 | 128/0/0 | 0 |
| active_held_4 | 939.11719 | 1002.75000 | 128/0/0 | 4 |
| active_finished_4 | 964.25391 | 1027.88672 | 128/0/0 | 0 |
| active_held_8 | 968.44531 | 1032.07812 | 128/0/0 | 8 |
| active_finished_8 | 1015.33984 | 1078.97266 | 128/0/0 | 0 |
| extra_controller_connections | 1016.86719 | 1081.06250 | 128/0/0 | 0 |
| history_browsed | 1020.09375 | 1084.28906 | 128/0/0 | 0 |
| slow_consumer_held | 1021.71094 | 1086.03125 | 128/0/0 | 1 |
| slow_consumer_settled | 1021.71094 | 1086.03125 | 128/0/0 | 0 |
| all_unloaded | 1018.84766 | 1079.98047 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 1018.86719 | 1080.18750 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 1018.87500 | 1080.19531 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 1018.87891 | 1080.19922 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 20.18906 | 64.30183 | 64.30183 | 17.50000 | 909.00000 |
| 4 | 60 | 36.63306 | 73.86875 | 82.01519 | 13.59375 | 940.87500 |
| 8 | 120 | 66.04929 | 109.75220 | 118.14766 | 13.35938 | 975.75000 |

Cancel-to-settled 12.18377 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### native-2-fixed

Diagnostic False; requests 1235; native databases 128; schema [94].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.19531 | 46.34375 | 0/0/0 | 0 |
| metadata_only | 25.31250 | 68.08594 | 0/0/0 | 0 |
| empty_attached_1 | 98.92578 | 149.10547 | 1/0/0 | 0 |
| empty_attached_8 | 114.80078 | 165.38672 | 8/0/0 | 0 |
| empty_attached_32 | 155.55469 | 206.89062 | 32/0/0 | 0 |
| empty_attached_64 | 212.05078 | 264.36328 | 64/0/0 | 0 |
| empty_attached_128 | 323.62891 | 377.91797 | 128/0/0 | 0 |
| subscribed_1 | 323.62891 | 379.16797 | 128/0/0 | 0 |
| subscribed_8 | 323.79297 | 379.33203 | 128/0/0 | 0 |
| subscribed_32 | 323.80469 | 379.34375 | 128/0/0 | 0 |
| subscribed_64 | 323.81250 | 379.35156 | 128/0/0 | 0 |
| subscribed_128 | 323.82031 | 379.35938 | 128/0/0 | 0 |
| all_subscribed_empty | 323.82031 | 379.35938 | 128/0/0 | 0 |
| seeded_history_1 | 482.07812 | 545.41406 | 128/0/0 | 0 |
| seeded_history_8 | 766.15234 | 829.48828 | 128/0/0 | 0 |
| active_held_1 | 766.40625 | 829.74219 | 128/0/0 | 1 |
| active_finished_1 | 769.72266 | 833.05859 | 128/0/0 | 0 |
| active_held_4 | 770.90234 | 834.23828 | 128/0/0 | 4 |
| active_finished_4 | 789.31250 | 852.64844 | 128/0/0 | 0 |
| active_held_8 | 792.59766 | 855.93359 | 128/0/0 | 8 |
| active_finished_8 | 829.39062 | 892.72656 | 128/0/0 | 0 |
| extra_controller_connections | 832.54688 | 896.25781 | 128/0/0 | 0 |
| history_browsed | 837.42969 | 901.26562 | 128/0/0 | 0 |
| slow_consumer_held | 837.62500 | 901.52344 | 128/0/0 | 1 |
| slow_consumer_settled | 837.83203 | 901.73047 | 128/0/0 | 0 |
| all_unloaded | 838.81641 | 899.52734 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 838.83203 | 899.79297 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 838.85938 | 899.82031 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 838.85938 | 899.82031 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 11.29625 | 19.09940 | 19.09940 | 12.50000 | 640.75000 |
| 4 | 60 | 29.59012 | 40.35049 | 44.71782 | 12.03125 | 666.75000 |
| 8 | 120 | 56.13792 | 67.26411 | 68.88434 | 11.64062 | 692.84375 |

Cancel-to-settled 7.22175 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### native-3-fixed

Diagnostic False; requests 1235; native databases 128; schema [94].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.19141 | 45.85156 | 0/0/0 | 0 |
| metadata_only | 26.17578 | 68.96094 | 0/0/0 | 0 |
| empty_attached_1 | 99.26953 | 149.52344 | 1/0/0 | 0 |
| empty_attached_8 | 113.12109 | 163.78125 | 8/0/0 | 0 |
| empty_attached_32 | 155.13672 | 206.54688 | 32/0/0 | 0 |
| empty_attached_64 | 211.89453 | 264.30078 | 64/0/0 | 0 |
| empty_attached_128 | 323.49219 | 377.89453 | 128/0/0 | 0 |
| subscribed_1 | 323.49609 | 379.33594 | 128/0/0 | 0 |
| subscribed_8 | 323.69922 | 379.53906 | 128/0/0 | 0 |
| subscribed_32 | 323.71094 | 379.55078 | 128/0/0 | 0 |
| subscribed_64 | 323.72266 | 379.56250 | 128/0/0 | 0 |
| subscribed_128 | 323.74609 | 379.58594 | 128/0/0 | 0 |
| all_subscribed_empty | 323.74609 | 379.58594 | 128/0/0 | 0 |
| seeded_history_1 | 482.09766 | 545.32031 | 128/0/0 | 0 |
| seeded_history_8 | 767.58203 | 830.80469 | 128/0/0 | 0 |
| active_held_1 | 768.00781 | 831.23047 | 128/0/0 | 1 |
| active_finished_1 | 772.29688 | 835.51953 | 128/0/0 | 0 |
| active_held_4 | 773.20312 | 836.42578 | 128/0/0 | 4 |
| active_finished_4 | 791.32812 | 854.55078 | 128/0/0 | 0 |
| active_held_8 | 794.42188 | 857.64453 | 128/0/0 | 8 |
| active_finished_8 | 831.57812 | 894.80078 | 128/0/0 | 0 |
| extra_controller_connections | 835.37500 | 898.97266 | 128/0/0 | 0 |
| history_browsed | 840.41797 | 904.07812 | 128/0/0 | 0 |
| slow_consumer_held | 840.87109 | 904.65625 | 128/0/0 | 1 |
| slow_consumer_settled | 840.93750 | 904.72266 | 128/0/0 | 0 |
| all_unloaded | 841.60156 | 902.13672 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 841.63672 | 902.29688 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 841.64453 | 902.30469 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 841.66016 | 902.32031 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 66.85683 | 278.29760 | 278.29760 | 20.62500 | 654.50000 |
| 4 | 60 | 37.66941 | 471.57455 | 540.04693 | 14.06250 | 666.50000 |
| 8 | 120 | 59.00371 | 83.01964 | 104.13703 | 11.79688 | 692.93750 |

Cancel-to-settled 7.27337 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### native-3-legacy

Diagnostic False; requests 1235; native databases 128; schema [93].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.19531 | 46.13281 | 0/0/0 | 0 |
| metadata_only | 30.47656 | 72.41797 | 0/0/0 | 0 |
| empty_attached_1 | 104.05469 | 153.58984 | 1/0/0 | 0 |
| empty_attached_8 | 118.99219 | 168.93359 | 8/0/0 | 0 |
| empty_attached_32 | 163.01562 | 213.70703 | 32/0/0 | 0 |
| empty_attached_64 | 220.92188 | 272.61328 | 64/0/0 | 0 |
| empty_attached_128 | 336.64844 | 390.32422 | 128/0/0 | 0 |
| subscribed_1 | 336.89844 | 391.88672 | 128/0/0 | 0 |
| subscribed_8 | 336.93750 | 391.92578 | 128/0/0 | 0 |
| subscribed_32 | 337.03906 | 392.02734 | 128/0/0 | 0 |
| subscribed_64 | 337.03906 | 392.02734 | 128/0/0 | 0 |
| subscribed_128 | 337.08203 | 392.07031 | 128/0/0 | 0 |
| all_subscribed_empty | 337.08203 | 392.07031 | 128/0/0 | 0 |
| seeded_history_1 | 534.44141 | 597.13281 | 128/0/0 | 0 |
| seeded_history_8 | 927.41797 | 990.17188 | 128/0/0 | 0 |
| active_held_1 | 928.28516 | 991.03906 | 128/0/0 | 1 |
| active_finished_1 | 939.46094 | 1002.21484 | 128/0/0 | 0 |
| active_held_4 | 941.82031 | 1004.57422 | 128/0/0 | 4 |
| active_finished_4 | 964.32031 | 1027.07422 | 128/0/0 | 0 |
| active_held_8 | 968.27734 | 1031.03125 | 128/0/0 | 8 |
| active_finished_8 | 1014.44922 | 1077.20312 | 128/0/0 | 0 |
| extra_controller_connections | 1015.91406 | 1079.04297 | 128/0/0 | 0 |
| history_browsed | 1017.88281 | 1081.01172 | 128/0/0 | 0 |
| slow_consumer_held | 1019.03906 | 1082.23047 | 128/0/0 | 1 |
| slow_consumer_settled | 1019.03906 | 1082.23047 | 128/0/0 | 0 |
| all_unloaded | 1017.22266 | 1077.22656 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 1017.22266 | 1077.41406 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 1017.23047 | 1077.42188 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 1017.23438 | 1077.42578 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 15.79979 | 60.71516 | 60.71516 | 15.00000 | 910.00000 |
| 4 | 60 | 59.62731 | 75.26929 | 81.09107 | 13.59375 | 941.06250 |
| 8 | 120 | 63.80102 | 105.90925 | 116.97806 | 13.35938 | 975.15625 |

Cancel-to-settled 48.50762 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### native-3-previous

Diagnostic False; requests 1235; native databases 128; schema [94].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.20312 | 46.11328 | 0/0/0 | 0 |
| metadata_only | 30.44922 | 72.92188 | 0/0/0 | 0 |
| empty_attached_1 | 104.16797 | 154.42188 | 1/0/0 | 0 |
| empty_attached_8 | 118.44141 | 169.03906 | 8/0/0 | 0 |
| empty_attached_32 | 160.48438 | 211.83203 | 32/0/0 | 0 |
| empty_attached_64 | 217.01953 | 269.36719 | 64/0/0 | 0 |
| empty_attached_128 | 329.32812 | 383.64844 | 128/0/0 | 0 |
| subscribed_1 | 329.39844 | 384.71875 | 128/0/0 | 0 |
| subscribed_8 | 329.44922 | 384.76953 | 128/0/0 | 0 |
| subscribed_32 | 329.51562 | 384.83594 | 128/0/0 | 0 |
| subscribed_64 | 329.51562 | 384.83594 | 128/0/0 | 0 |
| subscribed_128 | 329.51562 | 384.83594 | 128/0/0 | 0 |
| all_subscribed_empty | 329.51562 | 384.83594 | 128/0/0 | 0 |
| seeded_history_1 | 488.82031 | 551.79297 | 128/0/0 | 0 |
| seeded_history_8 | 784.70703 | 847.74219 | 128/0/0 | 0 |
| active_held_1 | 784.80469 | 847.83984 | 128/0/0 | 1 |
| active_finished_1 | 793.54688 | 856.58203 | 128/0/0 | 0 |
| active_held_4 | 794.64453 | 857.67969 | 128/0/0 | 4 |
| active_finished_4 | 809.31250 | 872.34766 | 128/0/0 | 0 |
| active_held_8 | 810.65234 | 873.68750 | 128/0/0 | 8 |
| active_finished_8 | 850.34766 | 913.38281 | 128/0/0 | 0 |
| extra_controller_connections | 851.78516 | 915.44531 | 128/0/0 | 0 |
| history_browsed | 853.14844 | 916.80859 | 128/0/0 | 0 |
| slow_consumer_held | 854.23438 | 917.95703 | 128/0/0 | 1 |
| slow_consumer_settled | 854.28516 | 918.00781 | 128/0/0 | 0 |
| all_unloaded | 855.85547 | 916.26562 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 856.12500 | 916.72266 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 856.13281 | 916.73047 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 856.14844 | 916.74609 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 21.53688 | 59.91147 | 59.91147 | 16.87500 | 685.25000 |
| 4 | 60 | 37.62161 | 73.16487 | 78.30622 | 14.06250 | 710.37500 |
| 8 | 120 | 64.53473 | 110.91941 | 117.55648 | 13.43750 | 737.37500 |

Cancel-to-settled 7.91043 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### profile-baseline

Diagnostic True; requests 1283; native databases 128; schema [93].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.36328 | 48.66016 | 0/0/0 | 0 |
| metadata_only | 31.02344 | 75.44531 | 0/0/0 | 0 |
| empty_attached_1 | 104.16797 | 156.30859 | 1/0/0 | 0 |
| empty_attached_8 | 120.12109 | 172.66797 | 8/0/0 | 0 |
| empty_attached_32 | 162.66797 | 215.96484 | 32/0/0 | 0 |
| empty_attached_64 | 221.56250 | 275.85938 | 64/0/0 | 0 |
| empty_attached_128 | 337.99609 | 394.28125 | 128/0/0 | 0 |
| subscribed_1 | 338.06641 | 395.41406 | 128/0/0 | 0 |
| subscribed_8 | 338.13672 | 395.48438 | 128/0/0 | 0 |
| subscribed_32 | 338.13672 | 395.48438 | 128/0/0 | 0 |
| subscribed_64 | 338.13672 | 395.48438 | 128/0/0 | 0 |
| subscribed_128 | 338.13672 | 395.48438 | 128/0/0 | 0 |
| all_subscribed_empty | 338.13672 | 395.48438 | 128/0/0 | 0 |
| seeded_history_1 | 527.85938 | 592.78125 | 128/0/0 | 0 |
| seeded_history_8 | 931.51172 | 996.43359 | 128/0/0 | 0 |
| active_held_1 | 940.84375 | 1007.67969 | 128/0/0 | 1 |
| active_finished_1 | 1047.35547 | 1119.00391 | 128/0/0 | 0 |
| extra_controller_connections | 1047.36328 | 1119.32422 | 128/0/0 | 0 |
| history_browsed | 1048.26562 | 1120.22656 | 128/0/0 | 0 |
| slow_consumer_held | 1050.01562 | 1122.03906 | 128/0/0 | 1 |
| slow_consumer_settled | 1050.01953 | 1122.04297 | 128/0/0 | 0 |
| all_unloaded | 1054.14453 | 1122.98047 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 1054.14453 | 1123.16797 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 1054.14844 | 1123.17188 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 1054.14844 | 1123.17188 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 255 | 14.26418 | 53.96156 | 56.97602 | 15.50781 | 775.42188 |

Cancel-to-settled 48.28462 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### profile-current

Diagnostic True; requests 1283; native databases 128; schema [94].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.36719 | 48.12500 | 0/0/0 | 0 |
| metadata_only | 30.60547 | 74.23828 | 0/0/0 | 0 |
| empty_attached_1 | 103.01172 | 154.48828 | 1/0/0 | 0 |
| empty_attached_8 | 119.19531 | 171.01562 | 8/0/0 | 0 |
| empty_attached_32 | 161.20703 | 213.77734 | 32/0/0 | 0 |
| empty_attached_64 | 218.00781 | 271.57031 | 64/0/0 | 0 |
| empty_attached_128 | 329.17969 | 384.73438 | 128/0/0 | 0 |
| subscribed_1 | 329.25000 | 385.92969 | 128/0/0 | 0 |
| subscribed_8 | 329.32031 | 386.00000 | 128/0/0 | 0 |
| subscribed_32 | 329.32031 | 386.00000 | 128/0/0 | 0 |
| subscribed_64 | 329.38672 | 386.06641 | 128/0/0 | 0 |
| subscribed_128 | 329.38672 | 386.06641 | 128/0/0 | 0 |
| all_subscribed_empty | 329.38672 | 386.06641 | 128/0/0 | 0 |
| seeded_history_1 | 484.31250 | 548.75781 | 128/0/0 | 0 |
| seeded_history_8 | 782.80078 | 847.24609 | 128/0/0 | 0 |
| active_held_1 | 790.80859 | 857.06250 | 128/0/0 | 1 |
| active_finished_1 | 865.17188 | 936.17578 | 128/0/0 | 0 |
| extra_controller_connections | 865.17188 | 936.67578 | 128/0/0 | 0 |
| history_browsed | 865.54688 | 937.17578 | 128/0/0 | 0 |
| slow_consumer_held | 866.54297 | 938.23438 | 128/0/0 | 1 |
| slow_consumer_settled | 866.54297 | 938.23438 | 128/0/0 | 0 |
| all_unloaded | 867.73438 | 936.30078 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 867.74609 | 936.50000 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 867.75391 | 936.50781 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 867.77734 | 936.53125 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 255 | 16.16303 | 55.30161 | 60.64026 | 15.27344 | 523.62500 |

Cancel-to-settled 8.42808 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### profile-clustered

Diagnostic True; requests 1283; native databases 128; schema [93].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.37109 | 47.63672 | 0/0/0 | 0 |
| metadata_only | 30.51953 | 74.59766 | 0/0/0 | 0 |
| empty_attached_1 | 102.83984 | 154.38672 | 1/0/0 | 0 |
| empty_attached_8 | 119.50391 | 171.45703 | 8/0/0 | 0 |
| empty_attached_32 | 161.15234 | 213.85547 | 32/0/0 | 0 |
| empty_attached_64 | 217.68750 | 271.39062 | 64/0/0 | 0 |
| empty_attached_128 | 328.64453 | 384.33984 | 128/0/0 | 0 |
| subscribed_1 | 328.64844 | 385.65625 | 128/0/0 | 0 |
| subscribed_8 | 328.71875 | 385.72656 | 128/0/0 | 0 |
| subscribed_32 | 328.80078 | 385.80859 | 128/0/0 | 0 |
| subscribed_64 | 328.80859 | 385.81641 | 128/0/0 | 0 |
| subscribed_128 | 328.82031 | 385.82812 | 128/0/0 | 0 |
| all_subscribed_empty | 328.82031 | 385.82812 | 128/0/0 | 0 |
| seeded_history_1 | 488.59766 | 553.48828 | 128/0/0 | 0 |
| seeded_history_8 | 781.82812 | 846.71875 | 128/0/0 | 0 |
| active_held_1 | 791.46094 | 858.30859 | 128/0/0 | 1 |
| active_finished_1 | 867.48828 | 939.21094 | 128/0/0 | 0 |
| extra_controller_connections | 867.49219 | 939.71484 | 128/0/0 | 0 |
| history_browsed | 867.93359 | 940.15625 | 128/0/0 | 0 |
| slow_consumer_held | 868.38281 | 940.73047 | 128/0/0 | 1 |
| slow_consumer_settled | 868.38281 | 940.73047 | 128/0/0 | 0 |
| all_unloaded | 869.24609 | 938.15625 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 869.25000 | 938.28516 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 869.25781 | 938.29297 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 869.25781 | 938.29297 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 255 | 17.14022 | 55.27619 | 59.24897 | 15.85938 | 539.45312 |

Cancel-to-settled 10.20700 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### profile-fixed

Diagnostic True; requests 1283; native databases 128; schema [94].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.34766 | 47.79688 | 0/0/0 | 0 |
| metadata_only | 25.61328 | 69.68750 | 0/0/0 | 0 |
| empty_attached_1 | 98.91016 | 150.51562 | 1/0/0 | 0 |
| empty_attached_8 | 113.65234 | 165.66406 | 8/0/0 | 0 |
| empty_attached_32 | 155.87891 | 208.64062 | 32/0/0 | 0 |
| empty_attached_64 | 211.53125 | 265.27734 | 64/0/0 | 0 |
| empty_attached_128 | 323.39453 | 379.12500 | 128/0/0 | 0 |
| subscribed_1 | 323.57031 | 380.48828 | 128/0/0 | 0 |
| subscribed_8 | 323.76953 | 380.68750 | 128/0/0 | 0 |
| subscribed_32 | 323.83594 | 380.75391 | 128/0/0 | 0 |
| subscribed_64 | 323.84766 | 380.76562 | 128/0/0 | 0 |
| subscribed_128 | 323.91406 | 380.83203 | 128/0/0 | 0 |
| all_subscribed_empty | 323.91406 | 380.83203 | 128/0/0 | 0 |
| seeded_history_1 | 479.75000 | 544.57422 | 128/0/0 | 0 |
| seeded_history_8 | 766.04297 | 830.86719 | 128/0/0 | 0 |
| active_held_1 | 774.36328 | 841.03906 | 128/0/0 | 1 |
| active_finished_1 | 843.98438 | 915.64062 | 128/0/0 | 0 |
| extra_controller_connections | 848.39453 | 920.55078 | 128/0/0 | 0 |
| history_browsed | 851.67188 | 923.89062 | 128/0/0 | 0 |
| slow_consumer_held | 852.01172 | 924.29297 | 128/0/0 | 1 |
| slow_consumer_settled | 852.19531 | 924.47656 | 128/0/0 | 0 |
| all_unloaded | 854.89453 | 923.98828 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 854.94141 | 924.22266 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 854.94922 | 924.23047 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 854.94922 | 924.23047 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 255 | 9.73973 | 14.19522 | 17.04817 | 11.83594 | 479.68750 |

Cancel-to-settled 6.53790 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### current-plain

Diagnostic True; requests 1059; native databases 128; schema [94].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.20312 | 46.33203 | 0/0/0 | 0 |
| metadata_only | 30.04688 | 72.17578 | 0/0/0 | 0 |
| empty_attached_1 | 103.63281 | 153.60547 | 1/0/0 | 0 |
| empty_attached_8 | 119.01562 | 169.33203 | 8/0/0 | 0 |
| empty_attached_32 | 160.30469 | 211.36719 | 32/0/0 | 0 |
| empty_attached_64 | 217.00000 | 269.06250 | 64/0/0 | 0 |
| empty_attached_128 | 328.34766 | 382.40625 | 128/0/0 | 0 |
| subscribed_1 | 328.41797 | 383.60156 | 128/0/0 | 0 |
| subscribed_8 | 328.53516 | 383.71875 | 128/0/0 | 0 |
| subscribed_32 | 328.53516 | 383.71875 | 128/0/0 | 0 |
| subscribed_64 | 328.53516 | 383.71875 | 128/0/0 | 0 |
| subscribed_128 | 328.53516 | 383.71875 | 128/0/0 | 0 |
| all_subscribed_empty | 328.53516 | 383.71875 | 128/0/0 | 0 |
| seeded_history_1 | 485.99609 | 548.93750 | 128/0/0 | 0 |
| seeded_history_8 | 783.79688 | 846.73828 | 128/0/0 | 0 |
| active_held_1 | 784.11719 | 847.05859 | 128/0/0 | 1 |
| active_finished_1 | 794.57031 | 857.51172 | 128/0/0 | 0 |
| extra_controller_connections | 794.57812 | 858.01953 | 128/0/0 | 0 |
| history_browsed | 794.59375 | 858.16016 | 128/0/0 | 0 |
| slow_consumer_held | 794.59375 | 858.22266 | 128/0/0 | 1 |
| slow_consumer_settled | 794.59375 | 858.22266 | 128/0/0 | 0 |
| all_unloaded | 795.39453 | 855.89844 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 794.26562 | 854.95703 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 794.27344 | 854.96484 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 794.27344 | 854.96484 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 31 | 20.25283 | 62.21335 | 62.31897 | 18.43750 | 736.87500 |

Cancel-to-settled 8.45376 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### current-nodelay

Diagnostic True; requests 1059; native databases 128; schema [94].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.21875 | 46.33203 | 0/0/0 | 0 |
| metadata_only | 30.34766 | 72.52344 | 0/0/0 | 0 |
| empty_attached_1 | 104.20312 | 153.97266 | 1/0/0 | 0 |
| empty_attached_8 | 117.48828 | 167.60156 | 8/0/0 | 0 |
| empty_attached_32 | 159.96094 | 210.82422 | 32/0/0 | 0 |
| empty_attached_64 | 216.32031 | 268.16406 | 64/0/0 | 0 |
| empty_attached_128 | 327.59766 | 381.44141 | 128/0/0 | 0 |
| subscribed_1 | 327.66797 | 382.51172 | 128/0/0 | 0 |
| subscribed_8 | 327.73438 | 382.57812 | 128/0/0 | 0 |
| subscribed_32 | 327.73438 | 382.57812 | 128/0/0 | 0 |
| subscribed_64 | 327.73438 | 382.57812 | 128/0/0 | 0 |
| subscribed_128 | 327.73438 | 382.57812 | 128/0/0 | 0 |
| all_subscribed_empty | 327.73438 | 382.57812 | 128/0/0 | 0 |
| seeded_history_1 | 485.22266 | 548.08594 | 128/0/0 | 0 |
| seeded_history_8 | 778.88281 | 841.74609 | 128/0/0 | 0 |
| active_held_1 | 779.06641 | 841.92969 | 128/0/0 | 1 |
| active_finished_1 | 793.43359 | 856.29688 | 128/0/0 | 0 |
| extra_controller_connections | 794.29688 | 857.53516 | 128/0/0 | 0 |
| history_browsed | 795.39062 | 858.75391 | 128/0/0 | 0 |
| slow_consumer_held | 797.14453 | 860.57031 | 128/0/0 | 1 |
| slow_consumer_settled | 797.19531 | 860.62109 | 128/0/0 | 0 |
| all_unloaded | 797.00000 | 857.30078 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 797.00000 | 857.48828 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 797.00000 | 857.48828 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 797.00781 | 857.49609 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 31 | 10.88212 | 14.00991 | 15.09766 | 13.75000 | 737.75000 |

Cancel-to-settled 7.65625 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### baseline-nodelay

Diagnostic True; requests 1059; native databases 128; schema [93].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.21484 | 46.54688 | 0/0/0 | 0 |
| metadata_only | 29.76562 | 72.34766 | 0/0/0 | 0 |
| empty_attached_1 | 103.49609 | 153.79688 | 1/0/0 | 0 |
| empty_attached_8 | 117.77344 | 168.48047 | 8/0/0 | 0 |
| empty_attached_32 | 163.58984 | 215.04688 | 32/0/0 | 0 |
| empty_attached_64 | 221.65625 | 274.09766 | 64/0/0 | 0 |
| empty_attached_128 | 338.00391 | 392.42969 | 128/0/0 | 0 |
| subscribed_1 | 338.14062 | 393.69141 | 128/0/0 | 0 |
| subscribed_8 | 338.14062 | 393.69141 | 128/0/0 | 0 |
| subscribed_32 | 338.20703 | 393.75781 | 128/0/0 | 0 |
| subscribed_64 | 338.20703 | 393.75781 | 128/0/0 | 0 |
| subscribed_128 | 338.20703 | 393.75781 | 128/0/0 | 0 |
| all_subscribed_empty | 338.20703 | 393.75781 | 128/0/0 | 0 |
| seeded_history_1 | 528.91406 | 591.99609 | 128/0/0 | 0 |
| seeded_history_8 | 925.72266 | 988.80469 | 128/0/0 | 0 |
| active_held_1 | 926.15625 | 989.23828 | 128/0/0 | 1 |
| active_finished_1 | 945.53516 | 1008.61719 | 128/0/0 | 0 |
| extra_controller_connections | 946.06641 | 1009.46094 | 128/0/0 | 0 |
| history_browsed | 946.67578 | 1010.07031 | 128/0/0 | 0 |
| slow_consumer_held | 947.62891 | 1011.02344 | 128/0/0 | 1 |
| slow_consumer_settled | 947.69531 | 1011.08984 | 128/0/0 | 0 |
| all_unloaded | 948.69531 | 1008.96484 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 948.69922 | 1009.15625 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 948.70703 | 1009.16406 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 948.70703 | 1009.16406 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 31 | 11.41558 | 14.18821 | 14.92096 | 13.43750 | 970.75000 |

Cancel-to-settled 8.07537 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

### baseline-plain

Diagnostic True; requests 1059; native databases 128; schema [93].

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active roots |
| --- | --- | --- | --- | --- |
| empty_server | 14.19922 | 46.30078 | 0/0/0 | 0 |
| metadata_only | 29.84766 | 71.88672 | 0/0/0 | 0 |
| empty_attached_1 | 103.65234 | 153.47266 | 1/0/0 | 0 |
| empty_attached_8 | 119.17188 | 169.39844 | 8/0/0 | 0 |
| empty_attached_32 | 162.51953 | 213.49609 | 32/0/0 | 0 |
| empty_attached_64 | 219.67188 | 271.64453 | 64/0/0 | 0 |
| empty_attached_128 | 336.97266 | 390.94141 | 128/0/0 | 0 |
| subscribed_1 | 337.14453 | 392.30078 | 128/0/0 | 0 |
| subscribed_8 | 337.17969 | 392.33594 | 128/0/0 | 0 |
| subscribed_32 | 337.28906 | 392.44531 | 128/0/0 | 0 |
| subscribed_64 | 337.29297 | 392.44922 | 128/0/0 | 0 |
| subscribed_128 | 337.29297 | 392.44922 | 128/0/0 | 0 |
| all_subscribed_empty | 337.29297 | 392.44922 | 128/0/0 | 0 |
| seeded_history_1 | 523.88672 | 586.80078 | 128/0/0 | 0 |
| seeded_history_8 | 923.95312 | 986.92969 | 128/0/0 | 0 |
| active_held_1 | 925.33594 | 988.31250 | 128/0/0 | 1 |
| active_finished_1 | 943.12500 | 1006.10156 | 128/0/0 | 0 |
| extra_controller_connections | 943.12500 | 1006.47656 | 128/0/0 | 0 |
| history_browsed | 943.78516 | 1007.13672 | 128/0/0 | 0 |
| slow_consumer_held | 944.05859 | 1007.47266 | 128/0/0 | 1 |
| slow_consumer_settled | 944.31250 | 1007.72656 | 128/0/0 | 0 |
| all_unloaded | 945.32812 | 1005.55469 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 945.32812 | 1005.61719 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 944.90234 | 1005.19141 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 944.90234 | 1005.19141 | 0/0/0 | 0 |

| Width | Nonheld n | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 31 | 13.59427 | 60.22642 | 61.28667 | 16.56250 | 972.87500 |

Cancel-to-settled 8.84270 ms. Native/API contents, cancelled partial absence, full provider history, integrity/FKs and image hashes independently rechecked.

All original per-turn values, counter fields, request bodies, closed images, source/check identities, failed attempts and frozen sources remain in the archive. Native counter windows include held waves; latency quantiles exclude them. Small shared-host cohorts do not establish production capacity or P99 confidence.
