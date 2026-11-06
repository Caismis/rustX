# Observation ownership: complete measurements

Four balanced pairs each for fixed/rotating, two separate pairs for deeper history. Before: 57c7082ea; final head in source-audit.json. Pi and DeepSeek were not run. MiB = 2^20 bytes, KiB = 2^10 bytes. Changes are medians of paired percentages, not ratios of displayed medians.

## Ordinary process memory

Sampled endpoints, not exact peaks. Anonymous PSS is not live heap or SQLite/replay allocation ownership.

### anon_pss_mib

| Endpoint | Before MiB | After MiB | Paired change |
| --- | --- | --- | --- |
| empty_server | 14.266 | 14.262 | -0.027% |
| metadata_only | 25.359 | 24.646 | -3.609% |
| empty_attached_1 | 28.061 | 26.457 | -5.853% |
| empty_attached_8 | 38.912 | 37.906 | -2.329% |
| empty_attached_32 | 64.268 | 63.811 | -1.122% |
| empty_attached_64 | 99.234 | 98.578 | -0.777% |
| empty_attached_128 | 169.088 | 168.127 | -0.619% |
| all_subscribed_empty | 169.510 | 168.406 | -0.654% |
| seeded_history_8 | 584.893 | 480.746 | -17.752% |
| all_unloaded | 651.826 | 523.934 | -19.590% |
| reload_unload_cycle_2 | 651.852 | 523.965 | -19.535% |

### rss_mib

| Endpoint | Before MiB | After MiB | Paired change |
| --- | --- | --- | --- |
| empty_server | 45.863 | 45.344 | -1.124% |
| metadata_only | 67.787 | 67.221 | -1.090% |
| empty_attached_1 | 78.117 | 77.188 | -1.037% |
| empty_attached_8 | 89.316 | 89.176 | -0.340% |
| empty_attached_32 | 115.430 | 115.795 | -0.048% |
| empty_attached_64 | 151.441 | 151.639 | -0.014% |
| empty_attached_128 | 223.271 | 223.092 | -0.137% |
| all_subscribed_empty | 224.902 | 224.596 | -0.136% |
| seeded_history_8 | 648.695 | 544.938 | -15.926% |
| all_unloaded | 712.879 | 585.602 | -17.814% |
| reload_unload_cycle_2 | 712.967 | 585.771 | -17.793% |

### Anonymous-PSS growth within each product

| Increment | Before MiB | After MiB | Paired change |
| --- | --- | --- | --- |
| first_session | 2.773 | 2.293 | -32.383% |
| remaining127_per_session | 1.111 | 1.114 | +0.442% |
| eight_turns_per_session | 3.245 | 2.443 | -24.727% |

### All memory pairs

| Endpoint / measure | Before pairs | After pairs | Paired changes % |
| --- | --- | --- | --- |
| empty_server / anon_pss_mib | 14.270, 14.359, 14.262, 14.262 | 14.262, 14.262, 14.262, 14.262 | -0.055, -0.680, 0.000, 0.000 |
| empty_server / rss_mib | 46.160, 45.875, 45.797, 45.852 | 44.910, 45.262, 45.426, 45.434 | -2.708, -1.337, -0.810, -0.912 |
| metadata_only / anon_pss_mib | 25.328, 25.812, 24.762, 25.391 | 24.516, 24.777, 24.910, 23.664 | -3.208, -4.010, 0.599, -6.800 |
| metadata_only / rss_mib | 67.906, 68.016, 66.984, 67.668 | 66.727, 67.715, 67.824, 66.398 | -1.737, -0.442, 1.254, -1.876 |
| empty_attached_1 / anon_pss_mib | 29.348, 27.938, 28.184, 27.488 | 26.750, 26.164, 27.305, 26.016 | -8.851, -6.348, -3.119, -5.357 |
| empty_attached_1 / rss_mib | 79.520, 78.172, 78.062, 77.422 | 77.367, 77.008, 78.312, 76.969 | -2.707, -1.489, 0.320, -0.585 |
| empty_attached_8 / anon_pss_mib | 38.402, 38.926, 39.109, 38.898 | 37.613, 37.992, 37.820, 38.020 | -2.055, -2.398, -3.296, -2.259 |
| empty_attached_8 / rss_mib | 88.980, 89.566, 89.395, 89.238 | 88.574, 89.180, 89.172, 89.316 | -0.457, -0.432, -0.249, 0.088 |
| empty_attached_32 / anon_pss_mib | 64.105, 64.902, 64.199, 64.336 | 63.344, 63.965, 64.102, 63.656 | -1.188, -1.444, -0.152, -1.056 |
| empty_attached_32 / rss_mib | 115.434, 116.277, 115.234, 115.426 | 115.055, 115.902, 116.191, 115.688 | -0.328, -0.323, 0.831, 0.227 |
| empty_attached_64 / anon_pss_mib | 99.223, 99.707, 99.156, 99.246 | 98.410, 98.922, 98.672, 98.484 | -0.819, -0.787, -0.488, -0.768 |
| empty_attached_64 / rss_mib | 151.547, 152.082, 151.191, 151.336 | 151.102, 151.859, 151.762, 151.516 | -0.294, -0.146, 0.377, 0.119 |
| empty_attached_128 / anon_pss_mib | 169.109, 169.324, 169.066, 169.023 | 167.719, 168.465, 168.523, 167.789 | -0.822, -0.508, -0.321, -0.730 |
| empty_attached_128 / rss_mib | 223.430, 223.684, 223.102, 223.113 | 222.391, 223.379, 223.602, 222.805 | -0.465, -0.136, 0.224, -0.138 |
| all_subscribed_empty / anon_pss_mib | 169.508, 169.531, 169.508, 169.512 | 167.938, 168.816, 168.805, 168.008 | -0.926, -0.422, -0.415, -0.887 |
| all_subscribed_empty / rss_mib | 225.078, 224.891, 224.855, 224.914 | 223.859, 224.793, 224.945, 224.398 | -0.541, -0.043, 0.040, -0.229 |
| seeded_history_8 / anon_pss_mib | 584.691, 583.168, 585.289, 585.094 | 480.555, 479.977, 481.680, 480.938 | -17.811, -17.695, -17.702, -17.802 |
| seeded_history_8 / rss_mib | 648.645, 647.246, 648.887, 648.746 | 544.578, 544.289, 546.156, 545.297 | -16.044, -15.907, -15.832, -15.946 |
| all_unloaded / anon_pss_mib | 651.488, 650.637, 653.078, 652.164 | 524.984, 523.660, 523.945, 523.922 | -19.418, -19.516, -19.773, -19.664 |
| all_unloaded / rss_mib | 712.691, 711.840, 713.926, 713.066 | 586.320, 585.473, 585.609, 585.594 | -17.732, -17.752, -17.973, -17.877 |
| reload_unload_cycle_2 / anon_pss_mib | 651.508, 649.602, 653.113, 652.195 | 525.062, 523.719, 523.973, 523.957 | -19.408, -19.378, -19.773, -19.663 |
| reload_unload_cycle_2 / rss_mib | 712.773, 710.992, 714.023, 713.160 | 586.523, 585.719, 585.824, 585.691 | -17.713, -17.620, -17.954, -17.874 |

## ordinary requests

Fixed active sessions, 16 rounds per width; 128×8 seeding precedes active work.

Latency excludes the deliberately held first round; native PID CPU/write counters include it. P95/P99 are finite sample summaries, not production-tail guarantees.

| Width / metric | Before | After | Paired change |
| --- | --- | --- | --- |
| 1 / settled_ms | 9.839 | 10.341 | +1.892% |
| 1 / accepted_ms | 0.955 | 1.382 | +44.684% |
| 1 / p95_ms | 13.296 | 15.218 | +8.660% |
| 1 / p99_ms | 13.296 | 15.218 | +8.660% |
| 1 / cpu_ms_per_turn | 10.938 | 11.250 | +2.941% |
| 1 / write_kib_per_turn | 640.625 | 640.875 | +0.059% |
| 4 / settled_ms | 28.173 | 28.231 | -1.718% |
| 4 / accepted_ms | 4.805 | 4.319 | -16.139% |
| 4 / p95_ms | 36.718 | 35.512 | -2.129% |
| 4 / p99_ms | 39.267 | 40.356 | -4.564% |
| 4 / cpu_ms_per_turn | 10.859 | 10.781 | -2.888% |
| 4 / write_kib_per_turn | 667.375 | 667.344 | +0.023% |
| 8 / settled_ms | 52.216 | 53.550 | +2.338% |
| 8 / accepted_ms | 10.098 | 10.530 | +0.021% |
| 8 / p95_ms | 62.541 | 63.173 | +1.473% |
| 8 / p99_ms | 64.386 | 70.964 | +10.204% |
| 8 / cpu_ms_per_turn | 10.898 | 10.859 | +0.362% |
| 8 / write_kib_per_turn | 693.625 | 693.125 | -0.090% |

| Width / metric | Before pairs | After pairs | Paired changes % |
| --- | --- | --- | --- |
| 1 / settled_ms | 9.540, 10.160, 10.138, 8.782 | 9.252, 10.785, 9.897, 12.130 | -3.013, 6.156, -2.373, 38.117 |
| 1 / accepted_ms | 0.884, 1.088, 0.950, 0.960 | 0.909, 1.691, 1.366, 1.398 | 2.834, 55.355, 43.792, 45.576 |
| 1 / p95_ms | 13.247, 14.637, 13.344, 11.885 | 11.918, 17.636, 12.922, 17.515 | -10.031, 20.486, -3.165, 47.372 |
| 1 / p99_ms | 13.247, 14.637, 13.344, 11.885 | 11.918, 17.636, 12.922, 17.515 | -10.031, 20.486, -3.165, 47.372 |
| 1 / cpu_ms_per_turn | 10.625, 11.875, 10.625, 11.250 | 11.250, 11.250, 11.250, 11.250 | 5.882, -5.263, 5.882, 0.000 |
| 1 / write_kib_per_turn | 640.750, 640.500, 641.500, 639.500 | 641.000, 641.000, 640.000, 640.750 | 0.039, 0.078, -0.234, 0.195 |
| 4 / settled_ms | 28.509, 30.806, 27.838, 27.306 | 26.330, 27.709, 29.010, 28.753 | -7.643, -10.054, 4.208, 5.298 |
| 4 / accepted_ms | 3.808, 6.845, 5.456, 4.154 | 3.581, 4.617, 4.020, 5.593 | -5.961, -32.546, -26.317, 34.641 |
| 4 / p95_ms | 34.622, 40.796, 38.815, 32.439 | 30.843, 34.037, 41.399, 36.988 | -10.915, -16.567, 6.658, 14.022 |
| 4 / p99_ms | 35.203, 43.330, 48.399, 34.955 | 35.161, 36.739, 44.039, 43.973 | -0.119, -15.211, -9.008, 25.800 |
| 4 / cpu_ms_per_turn | 10.781, 11.406, 10.938, 10.781 | 10.312, 10.781, 10.781, 11.250 | -4.348, -5.479, -1.429, 4.348 |
| 4 / write_kib_per_turn | 667.250, 669.188, 667.500, 666.625 | 667.562, 666.688, 667.500, 667.188 | 0.047, -0.374, 0.000, 0.084 |
| 8 / settled_ms | 51.246, 55.880, 52.523, 51.909 | 51.645, 53.167, 56.164, 53.933 | 0.778, -4.855, 6.931, 3.898 |
| 8 / accepted_ms | 9.770, 10.426, 14.333, 8.478 | 11.318, 9.742, 14.442, 8.418 | 15.840, -6.565, 0.757, -0.715 |
| 8 / p95_ms | 62.354, 64.313, 62.728, 62.124 | 62.638, 62.675, 71.886, 63.671 | 0.455, -2.548, 14.599, 2.491 |
| 8 / p99_ms | 64.595, 69.029, 64.073, 64.177 | 73.747, 65.597, 77.354, 68.182 | 14.169, -4.972, 20.728, 6.239 |
| 8 / cpu_ms_per_turn | 10.859, 11.172, 10.938, 10.781 | 10.859, 10.703, 11.250, 10.859 | 0.000, -4.196, 2.857, 0.725 |
| 8 / write_kib_per_turn | 693.156, 694.438, 693.438, 693.812 | 693.656, 693.062, 693.188, 692.812 | 0.072, -0.198, -0.036, -0.144 |

## rotating requests

Four active sessions rotate through all128,128rounds/512calls,histories8→12.

Latency excludes the deliberately held first round; native PID CPU/write counters include it. P95/P99 are finite sample summaries, not production-tail guarantees.

| Width / metric | Before | After | Paired change |
| --- | --- | --- | --- |
| 4 / settled_ms | 28.479 | 27.575 | -0.374% |
| 4 / accepted_ms | 5.353 | 5.292 | +3.377% |
| 4 / p95_ms | 38.564 | 38.671 | +0.148% |
| 4 / p99_ms | 47.677 | 46.315 | -1.410% |
| 4 / cpu_ms_per_turn | 11.123 | 11.074 | -0.959% |
| 4 / write_kib_per_turn | 628.441 | 628.523 | +0.019% |

| Width / metric | Before pairs | After pairs | Paired changes % |
| --- | --- | --- | --- |
| 4 / settled_ms | 27.564, 27.425, 29.394, 29.636 | 27.469, 27.315, 27.681, 30.485 | -0.346, -0.403, -5.826, 2.864 |
| 4 / accepted_ms | 5.506, 4.711, 5.416, 5.290 | 5.210, 5.165, 5.697, 5.373 | -5.362, 9.651, 5.176, 1.578 |
| 4 / p95_ms | 36.889, 35.863, 40.238, 41.301 | 34.497, 35.490, 495.077, 41.853 | -6.485, -1.040, 1130.364, 1.336 |
| 4 / p99_ms | 48.291, 45.284, 47.062, 48.677 | 39.194, 44.721, 893.582, 47.910 | -18.838, -1.244, 1798.713, -1.575 |
| 4 / cpu_ms_per_turn | 10.879, 10.938, 11.484, 11.309 | 10.781, 10.684, 11.367, 11.680 | -0.898, -2.321, -1.020, 3.282 |
| 4 / write_kib_per_turn | 628.383, 628.500, 628.422, 628.461 | 628.492, 628.234, 628.555, 628.805 | 0.017, -0.042, 0.021, 0.055 |

## deep requests

Eight sessions64turns,two separate pairs; fixed width4/32rounds; memory points8/16/32/64.

Latency excludes the deliberately held first round; native PID CPU/write counters include it. P95/P99 are finite sample summaries, not production-tail guarantees.

| Width / metric | Before | After | Paired change |
| --- | --- | --- | --- |
| 4 / settled_ms | 45.762 | 28.624 | -26.674% |
| 4 / accepted_ms | 7.212 | 4.175 | -31.999% |
| 4 / p95_ms | 332.123 | 39.909 | -46.540% |
| 4 / p99_ms | 396.583 | 48.826 | -34.888% |
| 4 / cpu_ms_per_turn | 12.422 | 11.211 | -8.829% |
| 4 / write_kib_per_turn | 728.672 | 729.234 | +0.077% |

| Width / metric | Before pairs | After pairs | Paired changes % |
| --- | --- | --- | --- |
| 4 / settled_ms | 63.715, 27.810 | 29.217, 28.031 | -54.145, 0.796 |
| 4 / accepted_ms | 10.014, 4.410 | 4.202, 4.148 | -58.039, -5.959 |
| 4 / p95_ms | 626.848, 37.398 | 42.360, 37.459 | -93.242, 0.162 |
| 4 / p99_ms | 752.172, 40.995 | 46.817, 50.834 | -93.776, 24.001 |
| 4 / cpu_ms_per_turn | 13.828, 11.016 | 11.484, 10.938 | -16.949, -0.709 |
| 4 / write_kib_per_turn | 727.625, 729.719 | 728.469, 730.000 | 0.116, 0.039 |

## Deeper history:8sessions,64turns,two balanced pairs

Not pooled with the128×8cohort. Immediate seeding endpoints; actual process counters retained.

| Endpoint / measure | Before MiB | After MiB | Paired change |
| --- | --- | --- | --- |
| all_subscribed_empty / anon_pss_mib | 28.975 | 28.133 | -2.722% |
| all_subscribed_empty / rss_mib | 80.432 | 80.025 | -0.471% |
| seeded_history_8 / anon_pss_mib | 57.791 | 51.967 | -10.077% |
| seeded_history_8 / rss_mib | 117.561 | 112.422 | -4.371% |
| seeded_history_16 / anon_pss_mib | 77.688 | 64.812 | -16.575% |
| seeded_history_16 / rss_mib | 137.457 | 125.268 | -8.868% |
| seeded_history_32 / anon_pss_mib | 114.826 | 88.125 | -23.252% |
| seeded_history_32 / rss_mib | 174.596 | 148.580 | -14.900% |
| seeded_history_64 / anon_pss_mib | 177.672 | 122.516 | -31.045% |
| seeded_history_64 / rss_mib | 237.441 | 182.971 | -22.941% |

## Concrete native owner census

Private intrusive rootO0/default diagnostics;126exact8turns per side. Counts address identities in current/replay and current prepared head. Inline sizes and serialized representations are not a live-heap/PSS partition or an exhaustive store-cache inventory.

| Representation | Before refs/unique median | After refs/unique median | Before inline total MiB | After inline total MiB |
| --- | --- | --- | --- | --- |
| binding_rows | 1444.0/373.5 | 1439.0/368.5 | 8.555 | 8.472 |
| descriptor_rows | 172.0/64.0 | 172.0/16.0 | 7.813 | 1.953 |
| frontier_rows | 3615.0/800.0 | 3613.0/797.5 | 6.143 | 6.134 |
| normalized_transcript_rows | 8.0/8.0 | 8.0/8.0 | 0.761 | 0.777 |
| snapshots | 8.0/8.0 | 8.0/8.0 | 2.607 | 2.607 |
| transcript_arrays | 38.0/38.0 | 38.0/38.0 | 0.109 | 0.109 |
| transcript_rows | 239.5/231.5 | 236.0/42.0 | 33.378 | 6.166 |

### Retained vector capacity

Before capacity diagnostic has6eligible8turn sessions; candidate census126. This describes actual buffers, not a paired PSS cohort; backing capacity can overlap the inline rows above.

| Side / vector | Median length | Median capacity | Requested bytes | Unused bytes |
| --- | --- | --- | --- | --- |
| before-capacity / descriptors | 48.0 | 48.0 | 48768.0 | 0.0 |
| before-capacity / events | 193.0 | 428.0 | 239680.0 | 131600.0 |
| before-capacity / metadata_bindings | 1222.5 | 1244.5 | 9956.0 | 176.0 |
| before-capacity / metadata_frontiers | 3469.0 | 52736.0 | 421888.0 | 394136.0 |
| before-capacity / transcript_rows | 226.5 | 244.5 | 295356.0 | 19932.0 |
| after / descriptors | 48.0 | 48.0 | 384.0 | 0.0 |
| after / events | 193.0 | 193.0 | 108080.0 | 0.0 |
| after / metadata_bindings | 1223.0 | 1223.0 | 9784.0 | 0.0 |
| after / metadata_frontiers | 3468.0 | 3468.0 | 27744.0 | 0.0 |
| after / transcript_rows | 228.0 | 228.0 | 1824.0 | 0.0 |

## Extended ordinary cache pair

One before/after product per side: diagnostic and functional coverage, not a timing confidence cohort. 32 s is all128×8; 130 s is four14-turn +1248-turn sessions. Cold/warm latency uses122 predefined eligible sessions; CPU/write windows include all128.

| Endpoint / measure | Before MiB | After MiB | Change |
| --- | --- | --- | --- |
| all_subscribed_empty / anon_pss_mib | 169.266 | 168.875 | -0.231% |
| all_subscribed_empty / rss_mib | 224.578 | 224.777 | +0.089% |
| seeded_history_8 / anon_pss_mib | 585.035 | 480.203 | -17.919% |
| seeded_history_8 / rss_mib | 648.754 | 544.418 | -16.083% |
| cache_pages_idle / anon_pss_mib | 571.770 | 466.172 | -18.469% |
| cache_pages_idle / rss_mib | 635.609 | 530.508 | -16.536% |
| cache_statements_idle / anon_pss_mib | 539.766 | 431.879 | -19.988% |
| cache_statements_idle / rss_mib | 603.605 | 496.215 | -17.792% |
| cache_rotation_cold / anon_pss_mib | 644.270 | 529.230 | -17.856% |
| cache_rotation_cold / rss_mib | 708.609 | 594.066 | -16.164% |
| cache_rotation_warm / anon_pss_mib | 683.914 | 554.027 | -18.992% |
| cache_rotation_warm / rss_mib | 748.254 | 618.863 | -17.292% |
| all_unloaded / anon_pss_mib | 740.836 | 588.438 | -20.571% |
| all_unloaded / rss_mib | 802.051 | 650.273 | -18.924% |

| Rotation / metric | Before | After | Change |
| --- | --- | --- | --- |
| cold / settled_median_ms | 12.624 | 12.827 | +1.607% |
| cold / accepted_median_ms | 1.875 | 2.017 | +7.582% |
| cold / settled_p95_ms | 14.873 | 15.607 | +4.939% |
| cold / cpu_ms_per_turn | 14.219 | 14.688 | +3.297% |
| cold / write_kib_per_turn | 483.375 | 483.219 | -0.032% |
| warm / settled_median_ms | 9.014 | 10.039 | +11.367% |
| warm / accepted_median_ms | 1.503 | 1.773 | +17.938% |
| warm / settled_p95_ms | 10.885 | 15.708 | +44.312% |
| warm / cpu_ms_per_turn | 10.703 | 11.250 | +5.109% |
| warm / write_kib_per_turn | 503.844 | 503.094 | -0.149% |

| Side | Kept-warm max gap s | Warm-rotation max gap s | Warm gaps ≥30 s | Inspector/history/2 Source reads ms | Mixed98 s CPU ms |
| --- | --- | --- | --- | --- | --- |
| before | 15.988 | 1.635 | 0 | 15.704 | 680 |
| after | 15.991 | 1.690 | 0 | 14.434 | 580 |

## Complete sampled heap diagnostics

One fresh native product per side,12 phases each. Replacement tcmalloc,64KiB Poisson samples. These are allocator-class live totals and estimated requested bytes, not ordinary-glibc PSS. Priority groups assign each weighted sampled allocation once by first matching stack; the grouping is hierarchy-dependent and does not partition PSS. Inclusive groups remain in heap-audit.json.

| Side / phase | Class-charged live MiB | Estimated requested MiB | Heap profiles |
| --- | --- | --- | --- |
| before / empty_server | 8.901 | 7.621 | 1 |
| before / metadata_only | 18.315 | 15.923 | 2 |
| before / empty_attached_1 | 22.695 | 19.471 | 3 |
| before / empty_attached_8 | 30.597 | 28.080 | 4 |
| before / empty_attached_32 | 58.239 | 53.252 | 5 |
| before / empty_attached_64 | 95.102 | 86.535 | 6 |
| before / empty_attached_128 | 168.761 | 152.490 | 7 |
| before / all_subscribed_empty | 168.982 | 152.713 | 8 |
| before / seeded_history_8 | 616.936 | 558.715 | 9 |
| before / cache_pages_idle | 505.804 | 460.364 | 10 |
| before / cache_statements_idle | 400.517 | 368.394 | 11 |
| before / all_unloaded | 23.811 | 21.477 | 12 |
| after / empty_server | 9.129 | 9.211 | 1 |
| after / metadata_only | 18.432 | 16.302 | 2 |
| after / empty_attached_1 | 21.702 | 19.198 | 3 |
| after / empty_attached_8 | 30.545 | 27.346 | 4 |
| after / empty_attached_32 | 57.962 | 51.040 | 5 |
| after / empty_attached_64 | 94.853 | 85.588 | 6 |
| after / empty_attached_128 | 168.577 | 154.808 | 7 |
| after / all_subscribed_empty | 168.771 | 154.618 | 8 |
| after / seeded_history_8 | 507.308 | 460.998 | 9 |
| after / cache_pages_idle | 395.770 | 357.811 | 10 |
| after / cache_statements_idle | 287.511 | 261.267 | 11 |
| after / all_unloaded | 23.918 | 21.318 | 12 |

| Side / phase | schema_validation | sqlite | http_tls | capability_preparation | tool_catalog | checkpoint_capture | publication | other_observation | model_binding | other_session | other |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| before / empty_attached_1 | 0.000 | 0.467 | 0.132 | 0.000 | 0.000 | 0.063 | 2.365 | 6.098 | 1.966 | 6.267 | 2.114 |
| before / all_subscribed_empty | 0.000 | 70.128 | 0.132 | 31.858 | 4.266 | 3.136 | 3.115 | 12.523 | 3.673 | 20.880 | 3.001 |
| before / seeded_history_8 | 0.000 | 219.959 | 0.132 | 31.858 | 4.266 | 110.342 | 6.809 | 129.499 | 5.739 | 20.815 | 29.295 |
| before / cache_pages_idle | 0.000 | 121.607 | 0.132 | 31.858 | 4.266 | 110.342 | 6.809 | 129.499 | 5.739 | 20.815 | 29.295 |
| before / cache_statements_idle | 0.000 | 52.376 | 0.132 | 31.858 | 4.266 | 113.023 | 6.809 | 127.436 | 5.801 | 19.676 | 7.015 |
| after / empty_attached_1 | 0.000 | 0.755 | 0.063 | 0.063 | 0.063 | 0.313 | 1.015 | 7.399 | 0.762 | 6.267 | 2.499 |
| after / all_subscribed_empty | 0.000 | 73.950 | 0.063 | 32.550 | 3.640 | 2.255 | 2.055 | 15.311 | 2.281 | 19.625 | 2.888 |
| after / seeded_history_8 | 0.000 | 229.452 | 0.063 | 32.550 | 3.640 | 45.176 | 5.313 | 90.874 | 4.597 | 19.435 | 29.896 |
| after / cache_pages_idle | 0.000 | 126.266 | 0.063 | 32.550 | 3.640 | 45.176 | 5.313 | 90.874 | 4.597 | 19.435 | 29.896 |
| after / cache_statements_idle | 0.000 | 55.503 | 0.063 | 32.550 | 3.640 | 46.743 | 5.633 | 85.552 | 4.660 | 18.483 | 8.440 |

## Unchanged prior Pi memory reference

One previously authorized Pi128 sample; no execution this round. Different service/tool/inspection surface, so the residual gap is not an estimate of removable work.

| Phase / measure | Prior Pi MiB | Before / Pi | After / Pi |
| --- | --- | --- | --- |
| all_subscribed_empty / anon_pss_mib | 105.477 | 1.607× | 1.597× |
| all_subscribed_empty / rss_mib | 165.141 | 1.362× | 1.360× |
| seeded_history_8 / anon_pss_mib | 310.465 | 1.884× | 1.548× |
| seeded_history_8 / rss_mib | 373.746 | 1.736× | 1.458× |

## Acceptance counts

| Check | Result |
| --- | --- |
| Source / helpers / command groups | 974 /2 /22 |
| Unique tests passed / existing ignored | 4384 /9 |
| Native products / consistent SQLite images | 28 /2984 |
| Complete matching HTTP body pairs / requests per side | 11 /13897 |
| Separate stream/large/tool/cancel products | 8 |
| New Pi / DeepSeek products | 0 /0 |
