# Loaded-session foundations: complete measurements

Four balanced pairs per primary family. Before: d50125842; after: 57c7082ea. Pi and DeepSeek were not run. MiB = 2^20 bytes, KiB = 2^10 bytes. Changes are medians of paired percentages, not ratios of displayed medians.

## Ordinary process memory

Sampled endpoints, not exact peaks. Anonymous PSS is not live heap or SQLite/replay allocation ownership.

### anon_pss_mib

| Endpoint | Before MiB | After MiB | Paired change |
| --- | --- | --- | --- |
| empty_server | 14.207 | 14.260 | +0.357% |
| metadata_only | 25.273 | 24.670 | -1.687% |
| empty_attached_1 | 99.652 | 28.203 | -71.651% |
| empty_attached_8 | 113.410 | 37.957 | -66.650% |
| empty_attached_32 | 155.873 | 64.061 | -58.841% |
| empty_attached_64 | 211.053 | 98.965 | -53.126% |
| empty_attached_128 | 323.201 | 168.680 | -47.797% |
| all_subscribed_empty | 323.607 | 169.027 | -47.750% |
| seeded_history_8 | 743.289 | 585.506 | -21.175% |
| all_unloaded | 812.445 | 653.584 | -19.563% |
| reload_unload_cycle_2 | 812.494 | 653.646 | -19.565% |

### rss_mib

| Endpoint | Before MiB | After MiB | Paired change |
| --- | --- | --- | --- |
| empty_server | 46.328 | 45.842 | -1.131% |
| metadata_only | 67.963 | 66.816 | -1.643% |
| empty_attached_1 | 149.883 | 77.990 | -47.817% |
| empty_attached_8 | 163.979 | 88.162 | -46.378% |
| empty_attached_32 | 207.141 | 115.252 | -44.360% |
| empty_attached_64 | 263.512 | 150.971 | -42.652% |
| empty_attached_128 | 377.441 | 222.889 | -40.974% |
| all_subscribed_empty | 379.066 | 224.500 | -40.811% |
| seeded_history_8 | 806.785 | 649.127 | -19.516% |
| all_unloaded | 873.254 | 714.775 | -18.211% |
| reload_unload_cycle_2 | 873.521 | 714.994 | -18.223% |

### Anonymous-PSS growth within each product

| Increment | Before MiB | After MiB | Paired change |
| --- | --- | --- | --- |
| first_session | 74.875 | 3.463 | -95.408% |
| remaining127_per_session | 1.760 | 1.106 | -37.090% |
| eight_turns_per_session | 3.277 | 3.253 | -0.675% |

### All memory pairs

| Endpoint / measure | Before pair 0..3 | After pair 0..3 | Paired changes 0..3 % |
| --- | --- | --- | --- |
| empty_server / anon_pss_mib | 14.207, 14.211, 14.207, 14.207 | 14.258, 14.262, 14.352, 14.258 | 0.357, 0.357, 1.017, 0.357 |
| empty_server / rss_mib | 46.062, 46.609, 46.125, 46.531 | 45.840, 45.922, 45.762, 45.844 | -0.483, -1.475, -0.788, -1.478 |
| metadata_only / anon_pss_mib | 25.129, 25.418, 24.426, 25.430 | 24.602, 25.094, 24.484, 24.738 | -2.099, -1.276, 0.240, -2.719 |
| metadata_only / rss_mib | 67.797, 68.379, 67.531, 68.129 | 66.809, 67.129, 66.582, 66.824 | -1.458, -1.828, -1.406, -1.915 |
| empty_attached_1 / anon_pss_mib | 100.332, 97.934, 98.973, 101.059 | 28.102, 28.242, 28.395, 28.164 | -71.991, -71.162, -71.311, -72.131 |
| empty_attached_1 / rss_mib | 150.531, 148.238, 149.234, 150.914 | 77.965, 77.934, 78.898, 78.016 | -48.207, -47.427, -47.131, -48.305 |
| empty_attached_8 / anon_pss_mib | 113.027, 113.793, 112.914, 114.000 | 37.262, 37.996, 37.941, 37.973 | -67.033, -66.609, -66.398, -66.691 |
| empty_attached_8 / rss_mib | 163.695, 164.566, 163.520, 164.262 | 87.531, 88.094, 88.727, 88.230 | -46.528, -46.469, -45.739, -46.287 |
| empty_attached_32 / anon_pss_mib | 156.223, 155.531, 154.910, 156.215 | 64.051, 64.578, 63.984, 64.070 | -59.000, -58.479, -58.696, -58.986 |
| empty_attached_32 / rss_mib | 207.641, 207.055, 206.258, 207.227 | 115.055, 115.426, 115.520, 115.078 | -44.590, -44.253, -43.993, -44.467 |
| empty_attached_64 / anon_pss_mib | 211.855, 211.012, 211.094, 210.914 | 98.957, 99.094, 98.840, 98.973 | -53.290, -53.039, -53.177, -53.074 |
| empty_attached_64 / rss_mib | 264.254, 263.520, 263.504, 262.926 | 150.961, 150.926, 151.375, 150.980 | -42.873, -42.727, -42.553, -42.577 |
| empty_attached_128 / anon_pss_mib | 323.492, 323.043, 322.883, 323.359 | 169.094, 168.484, 168.652, 168.707 | -47.729, -47.845, -47.767, -47.827 |
| empty_attached_128 / rss_mib | 377.871, 377.535, 377.285, 377.348 | 223.082, 222.301, 223.188, 222.695 | -40.963, -41.118, -40.844, -40.984 |
| all_subscribed_empty / anon_pss_mib | 323.867, 323.430, 323.023, 323.785 | 169.324, 168.691, 168.980, 169.074 | -47.718, -47.843, -47.688, -47.782 |
| all_subscribed_empty / rss_mib | 379.684, 379.234, 378.426, 378.898 | 224.625, 223.695, 224.641, 224.375 | -40.839, -41.014, -40.638, -40.782 |
| seeded_history_8 / anon_pss_mib | 743.578, 743.836, 741.090, 743.000 | 585.629, 586.832, 585.383, 575.332 | -21.242, -21.107, -21.011, -22.566 |
| seeded_history_8 / rss_mib | 807.309, 807.547, 804.070, 806.262 | 649.211, 650.492, 649.043, 639.289 | -19.583, -19.448, -19.280, -20.709 |
| all_unloaded / anon_pss_mib | 813.059, 813.934, 810.605, 811.832 | 654.750, 653.953, 653.215, 640.461 | -19.471, -19.655, -19.416, -21.109 |
| all_unloaded / rss_mib | 874.227, 875.020, 870.836, 872.281 | 715.582, 715.113, 714.438, 701.793 | -18.147, -18.275, -17.960, -19.545 |
| reload_unload_cycle_2 / anon_pss_mib | 813.117, 814.004, 809.477, 811.871 | 654.766, 654.004, 653.289, 640.520 | -19.475, -19.656, -19.295, -21.106 |
| reload_unload_cycle_2 / rss_mib | 874.535, 875.277, 869.832, 872.508 | 715.660, 715.289, 714.699, 701.914 | -18.167, -18.279, -17.835, -19.552 |

## ordinary requests

Fixed active sessions, 16 rounds per width; 128×8 seeding precedes active work.

Latency excludes the deliberately held first round; native PID CPU/write counters include it. P95/P99 are finite sample summaries, not production-tail guarantees.

| Width / metric | Before | After | Paired change |
| --- | --- | --- | --- |
| 1 / settled_ms | 9.700 | 9.545 | +2.810% |
| 1 / accepted_ms | 1.005 | 1.055 | +5.049% |
| 1 / p95_ms | 14.833 | 14.040 | -3.326% |
| 1 / p99_ms | 14.833 | 14.040 | -3.326% |
| 1 / cpu_ms_per_turn | 11.875 | 11.250 | -5.263% |
| 1 / write_kib_per_turn | 640.000 | 641.125 | +0.176% |
| 4 / settled_ms | 29.711 | 28.344 | -3.721% |
| 4 / accepted_ms | 5.723 | 4.479 | -28.619% |
| 4 / p95_ms | 40.466 | 33.110 | -17.515% |
| 4 / p99_ms | 45.948 | 35.654 | -22.819% |
| 4 / cpu_ms_per_turn | 11.719 | 11.094 | -6.029% |
| 4 / write_kib_per_turn | 667.281 | 667.281 | -0.042% |
| 8 / settled_ms | 56.572 | 55.954 | -2.766% |
| 8 / accepted_ms | 8.385 | 11.780 | +31.689% |
| 8 / p95_ms | 72.379 | 76.253 | +3.100% |
| 8 / p99_ms | 74.606 | 81.191 | +5.210% |
| 8 / cpu_ms_per_turn | 11.953 | 11.562 | -3.533% |
| 8 / write_kib_per_turn | 693.047 | 693.062 | -0.005% |

| Width / metric | Before pair 0..3 | After pair 0..3 | Paired changes 0..3 % |
| --- | --- | --- | --- |
| 1 / settled_ms | 9.247, 11.926, 10.079, 9.322 | 9.660, 8.876, 11.098, 9.429 | 4.477, -25.572, 10.115, 1.143 |
| 1 / accepted_ms | 1.002, 1.231, 0.999, 1.007 | 1.024, 0.975, 1.435, 1.087 | 2.211, -20.768, 43.677, 7.887 |
| 1 / p95_ms | 16.374, 19.368, 12.181, 13.292 | 12.643, 12.361, 17.757, 15.437 | -22.789, -36.177, 45.771, 16.136 |
| 1 / p99_ms | 16.374, 19.368, 12.181, 13.292 | 12.643, 12.361, 17.757, 15.437 | -22.789, -36.177, 45.771, 16.136 |
| 1 / cpu_ms_per_turn | 11.875, 13.125, 11.875, 11.875 | 11.875, 11.250, 11.250, 11.250 | 0.000, -14.286, -5.263, -5.263 |
| 1 / write_kib_per_turn | 640.750, 640.000, 639.500, 640.000 | 641.750, 641.250, 641.000, 641.000 | 0.156, 0.195, 0.235, 0.156 |
| 4 / settled_ms | 30.607, 29.067, 30.355, 27.553 | 29.119, 28.317, 28.372, 27.071 | -4.860, -2.582, -6.533, -1.750 |
| 4 / accepted_ms | 7.288, 4.775, 6.671, 4.742 | 4.980, 3.554, 4.454, 4.505 | -31.667, -25.570, -33.233, -4.998 |
| 4 / p95_ms | 44.631, 36.301, 45.991, 33.586 | 33.940, 32.280, 34.801, 31.672 | -23.955, -11.076, -24.331, -5.699 |
| 4 / p99_ms | 50.752, 43.849, 48.047, 34.066 | 38.067, 34.183, 36.710, 34.598 | -24.994, -22.043, -23.595, 1.562 |
| 4 / cpu_ms_per_turn | 11.562, 11.875, 12.188, 11.406 | 11.406, 11.094, 11.094, 10.781 | -1.351, -6.579, -8.974, -5.479 |
| 4 / write_kib_per_turn | 667.875, 667.125, 666.688, 667.438 | 667.188, 667.500, 666.188, 667.375 | -0.103, 0.056, -0.075, -0.009 |
| 8 / settled_ms | 58.785, 55.937, 57.207, 55.218 | 58.349, 57.440, 54.467, 52.250 | -0.742, 2.688, -4.790, -5.375 |
| 8 / accepted_ms | 11.130, 8.120, 8.650, 7.320 | 11.112, 13.109, 12.447, 8.746 | -0.161, 61.433, 43.886, 19.493 |
| 8 / p95_ms | 76.026, 69.382, 75.377, 64.587 | 73.362, 79.304, 79.145, 65.363 | -3.504, 14.300, 4.999, 1.200 |
| 8 / p99_ms | 84.210, 69.884, 78.278, 70.933 | 75.457, 100.322, 86.924, 70.489 | -10.394, 43.555, 11.045, -0.626 |
| 8 / cpu_ms_per_turn | 12.422, 11.953, 11.953, 11.562 | 12.031, 11.641, 11.484, 11.016 | -3.145, -2.614, -3.922, -4.730 |
| 8 / write_kib_per_turn | 693.469, 692.500, 693.156, 692.938 | 693.125, 693.000, 692.844, 693.188 | -0.050, 0.072, -0.045, 0.036 |

## rotating requests

Four active sessions rotate through all 128, 128 rounds /512 calls per product; histories grow 8→12.

Latency excludes the deliberately held first round; native PID CPU/write counters include it. P95/P99 are finite sample summaries, not production-tail guarantees.

| Width / metric | Before | After | Paired change |
| --- | --- | --- | --- |
| 4 / settled_ms | 34.985 | 33.041 | -6.543% |
| 4 / accepted_ms | 5.870 | 7.471 | +4.326% |
| 4 / p95_ms | 120.355 | 375.551 | +30.142% |
| 4 / p99_ms | 654.109 | 683.683 | -4.694% |
| 4 / cpu_ms_per_turn | 13.096 | 12.549 | -6.240% |
| 4 / write_kib_per_turn | 628.902 | 628.508 | -0.037% |

| Width / metric | Before pair 0..3 | After pair 0..3 | Paired changes 0..3 % |
| --- | --- | --- | --- |
| 4 / settled_ms | 49.971, 39.097, 28.658, 30.874 | 45.134, 30.159, 27.682, 35.923 | -9.680, -22.860, -3.405, 16.355 |
| 4 / accepted_ms | 8.902, 6.246, 5.015, 5.494 | 9.014, 6.707, 4.827, 8.235 | 1.262, 7.390, -3.762, 49.881 |
| 4 / p95_ms | 199.814, 454.691, 37.294, 40.896 | 306.328, 444.773, 39.896, 486.235 | 53.306, -2.181, 6.977, 1088.955 |
| 4 / p99_ms | 2036.761, 1261.865, 43.193, 46.353 | 1817.839, 615.781, 43.781, 751.585 | -10.749, -51.201, 1.360, 1521.455 |
| 4 / cpu_ms_per_turn | 17.637, 14.180, 11.465, 12.012 | 16.367, 11.328, 10.859, 13.770 | -7.198, -20.110, -5.281, 14.634 |
| 4 / write_kib_per_turn | 632.602, 628.820, 628.984, 628.664 | 637.883, 628.711, 628.062, 628.305 | 0.835, -0.017, -0.147, -0.057 |

## Extended ordinary cache pair

One before/after product per side: diagnostic and functional coverage, not a timing confidence cohort. 32 s is all128×8; 130 s is four14-turn +1248-turn sessions. Cold/warm latency uses122 predefined eligible sessions; CPU/write windows include all128.

| Endpoint / measure | Before MiB | After MiB | Change |
| --- | --- | --- | --- |
| all_subscribed_empty / anon_pss_mib | 324.000 | 168.133 | -48.107% |
| all_subscribed_empty / rss_mib | 379.820 | 223.434 | -41.174% |
| seeded_history_8 / anon_pss_mib | 743.547 | 585.035 | -21.318% |
| seeded_history_8 / rss_mib | 806.953 | 648.625 | -19.620% |
| cache_pages_idle / anon_pss_mib | 730.164 | 571.652 | -21.709% |
| cache_pages_idle / rss_mib | 793.691 | 635.426 | -19.940% |
| cache_statements_idle / anon_pss_mib | 698.535 | 538.191 | -22.954% |
| cache_statements_idle / rss_mib | 762.062 | 601.965 | -21.008% |
| cache_rotation_cold / anon_pss_mib | 801.066 | 636.051 | -20.599% |
| cache_rotation_cold / rss_mib | 865.156 | 700.387 | -19.045% |
| cache_rotation_warm / anon_pss_mib | 841.578 | 669.164 | -20.487% |
| cache_rotation_warm / rss_mib | 905.668 | 733.500 | -19.010% |
| all_unloaded / anon_pss_mib | 897.258 | 724.094 | -19.299% |
| all_unloaded / rss_mib | 958.035 | 785.180 | -18.043% |

| Rotation / metric | Before | After | Change |
| --- | --- | --- | --- |
| cold / settled_median_ms | 12.842 | 12.731 | -0.862% |
| cold / accepted_median_ms | 1.956 | 1.991 | +1.787% |
| cold / settled_p95_ms | 15.849 | 14.723 | -7.108% |
| cold / cpu_ms_per_turn | 14.922 | 14.219 | -4.712% |
| cold / write_kib_per_turn | 483.906 | 483.281 | -0.129% |
| warm / settled_median_ms | 9.579 | 8.906 | -7.029% |
| warm / accepted_median_ms | 1.553 | 1.441 | -7.194% |
| warm / settled_p95_ms | 14.724 | 10.957 | -25.584% |
| warm / cpu_ms_per_turn | 11.641 | 10.781 | -7.383% |
| warm / write_kib_per_turn | 502.688 | 503.781 | +0.218% |

| Side | Kept-warm max gap s | Warm-rotation max gap s | Warm gaps ≥30 s | Inspector/history/2 Source reads ms | Mixed98 s CPU ms |
| --- | --- | --- | --- | --- | --- |
| before | 15.990 | 1.701 | 0 | 15.421 | 620 |
| after | 15.994 | 1.652 | 0 | 14.347 | 600 |

## CPU sample audit

Three separate profiler products. Inclusive groups overlap; counts cannot be added. These rotating512-call diagnostics do not reproduce the earlier fixed64-call +4.13% cell.

| Version | Samples | SQLite | Observation | Serde | Public native Budget | Checkpoint | Schema compile |
| --- | --- | --- | --- | --- | --- | --- | --- |
| previous | 1466 | 362 | 732 | 513 | 47 | 128 | 0 |
| before | 1454 | 364 | 724 | 530 | 49 | 123 | 0 |
| after | 1377 | 382 | 655 | 448 | 0 | 133 | 0 |

## Complete sampled heap diagnostics

One fresh native product per side,12 phases each. Replacement tcmalloc,64KiB Poisson samples. These are allocator-class live totals and estimated requested bytes, not ordinary-glibc PSS. Priority groups assign each weighted sampled allocation once by first matching stack; the grouping is hierarchy-dependent and does not partition PSS. Inclusive groups remain in heap-audit.json.

| Side / phase | Class-charged live MiB | Estimated requested MiB | Heap profiles |
| --- | --- | --- | --- |
| before / empty_server | 8.898 | 8.019 | 1 |
| before / metadata_only | 18.148 | 16.269 | 2 |
| before / empty_attached_1 | 95.624 | 85.720 | 3 |
| before / empty_attached_8 | 108.639 | 98.445 | 4 |
| before / empty_attached_32 | 152.641 | 137.912 | 5 |
| before / empty_attached_64 | 211.346 | 190.459 | 6 |
| before / empty_attached_128 | 328.385 | 293.264 | 7 |
| before / all_subscribed_empty | 328.574 | 293.171 | 8 |
| before / seeded_history_8 | 776.612 | 709.276 | 9 |
| before / cache_pages_idle | 665.259 | 608.189 | 10 |
| before / cache_statements_idle | 559.338 | 510.813 | 11 |
| before / all_unloaded | 96.431 | 87.465 | 12 |
| after / empty_server | 8.986 | 8.180 | 1 |
| after / metadata_only | 18.431 | 17.457 | 2 |
| after / empty_attached_1 | 22.471 | 20.844 | 3 |
| after / empty_attached_8 | 30.428 | 27.206 | 4 |
| after / empty_attached_32 | 58.172 | 53.815 | 5 |
| after / empty_attached_64 | 94.831 | 86.034 | 6 |
| after / empty_attached_128 | 168.468 | 153.167 | 7 |
| after / all_subscribed_empty | 168.652 | 152.996 | 8 |
| after / seeded_history_8 | 617.862 | 568.262 | 9 |
| after / cache_pages_idle | 506.293 | 464.629 | 10 |
| after / cache_statements_idle | 400.905 | 369.949 | 11 |
| after / all_unloaded | 23.843 | 22.203 | 12 |

| Side / phase | schema_validation | sqlite | http_tls | capability_preparation | tool_catalog | checkpoint_capture | publication | other_observation | model_binding | other_session | other |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| before / empty_attached_1 | 65.887 | 0.402 | 0.258 | 0.063 | 0.441 | 0.125 | 1.403 | 6.623 | 1.324 | 7.050 | 2.144 |
| before / all_subscribed_empty | 65.887 | 70.008 | 23.481 | 55.205 | 30.048 | 2.635 | 2.364 | 15.144 | 3.666 | 21.517 | 3.215 |
| before / seeded_history_8 | 65.887 | 231.080 | 23.606 | 55.205 | 30.048 | 108.817 | 6.694 | 131.847 | 6.734 | 21.608 | 27.749 |
| before / cache_pages_idle | 65.887 | 129.993 | 23.606 | 55.205 | 30.048 | 108.817 | 6.694 | 131.847 | 6.734 | 21.608 | 27.749 |
| before / cache_statements_idle | 65.887 | 52.820 | 23.606 | 55.205 | 30.048 | 111.445 | 6.950 | 130.229 | 6.734 | 20.785 | 7.102 |
| after / empty_attached_1 | 0.000 | 0.441 | 0.199 | 0.063 | 0.125 | 0.189 | 1.965 | 6.971 | 1.141 | 8.000 | 1.750 |
| after / all_subscribed_empty | 0.000 | 71.258 | 0.199 | 29.286 | 3.770 | 2.507 | 2.281 | 15.046 | 3.162 | 22.997 | 2.489 |
| after / seeded_history_8 | 0.000 | 231.880 | 0.199 | 29.286 | 3.770 | 108.866 | 5.656 | 132.834 | 5.540 | 22.556 | 27.674 |
| after / cache_pages_idle | 0.000 | 128.185 | 0.199 | 29.286 | 3.770 | 108.866 | 5.656 | 132.834 | 5.540 | 22.556 | 27.737 |
| after / cache_statements_idle | 0.000 | 54.020 | 0.199 | 29.286 | 3.770 | 111.536 | 5.719 | 130.846 | 5.540 | 21.483 | 7.550 |

## Unchanged prior Pi memory reference

One previously authorized Pi128 sample; no execution this round. Different service/tool/inspection surface, so the residual gap is not an estimate of removable work.

| Phase / measure | Prior Pi MiB | Before / Pi | After / Pi |
| --- | --- | --- | --- |
| all_subscribed_empty / anon_pss_mib | 105.477 | 3.068× | 1.603× |
| all_subscribed_empty / rss_mib | 165.141 | 2.295× | 1.359× |
| seeded_history_8 / anon_pss_mib | 310.465 | 2.394× | 1.886× |
| seeded_history_8 / rss_mib | 373.746 | 2.159× | 1.737× |

## Acceptance counts

| Check | Result |
| --- | --- |
| Source / helpers / command groups | 974 /2 /22 |
| Unique tests passed / existing ignored | 4379 /9 |
| Native products / consistent SQLite images | 23 /2944 |
| Complete matching HTTP body pairs / requests per side | 9 /12611 |
| Separate stream/large/tool/cancel products | 8 |
| New Pi / DeepSeek products | 0 /0 |
