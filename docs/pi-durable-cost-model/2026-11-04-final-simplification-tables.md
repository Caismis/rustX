# Final simplification: complete measurements

Before is saved production e7289ac18; Statistics is35ae68b5f; final is3d5055779. No Pi/DeepSeek builds, products or performance runs. MiB=2^20 bytes, KiB=2^10 bytes. Changes are medians of same-pair percentages, not ratios of displayed medians. Hot, native, history, Source and WAL products have distinct denominators.

## statistics independent hot comparison

32 hot turns after8/64 history; four alternating pairs per cell. CPU includes driver,10ms process ticks. live-subscriber excludes AppServer/WebUI transport.

| Mode | History case | Metric | Before | After | Paired % |
| --- | --- | --- | --- | --- | --- |
| no-live-subscriber | short-large | mean_turn_ms | 7.424 | 7.437 | 0.552 |
| no-live-subscriber | short-large | turn_cpu_ms_per_turn | 7.500 | 7.500 | 0.000 |
| no-live-subscriber | short-large | turn_write_mib_per_turn | 0.704 | 0.704 | 0.009 |
| no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 2560.000 | 2555.000 | -0.391 |
| no-live-subscriber | short-large | through_cleanup_process_write_mib | 61.314 | 61.449 | 0.169 |
| no-live-subscriber | short-small | mean_turn_ms | 6.520 | 6.576 | 1.021 |
| no-live-subscriber | short-small | turn_cpu_ms_per_turn | 6.719 | 6.875 | 0.000 |
| no-live-subscriber | short-small | turn_write_mib_per_turn | 0.577 | 0.577 | 0.064 |
| no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 710.000 | 710.000 | 0.714 |
| no-live-subscriber | short-small | through_cleanup_process_write_mib | 23.264 | 23.287 | 0.092 |
| live-subscriber | short-large | mean_turn_ms | 9.070 | 8.209 | -9.991 |
| live-subscriber | short-large | turn_cpu_ms_per_turn | 10.312 | 9.531 | -7.442 |
| live-subscriber | short-large | turn_write_mib_per_turn | 0.705 | 0.704 | -0.009 |
| live-subscriber | short-large | through_cleanup_self_cpu_ms | 2790.000 | 2750.000 | -1.429 |
| live-subscriber | short-large | through_cleanup_process_write_mib | 61.457 | 61.443 | 0.029 |
| live-subscriber | short-small | mean_turn_ms | 7.536 | 7.554 | -1.329 |
| live-subscriber | short-small | turn_cpu_ms_per_turn | 8.750 | 8.750 | 0.000 |
| live-subscriber | short-small | turn_write_mib_per_turn | 0.576 | 0.578 | 0.223 |
| live-subscriber | short-small | through_cleanup_self_cpu_ms | 780.000 | 785.000 | 1.290 |
| live-subscriber | short-small | through_cleanup_process_write_mib | 23.264 | 23.309 | 0.193 |

| Mode | Case | Metric | Before pairs | After pairs | Paired % values |
| --- | --- | --- | --- | --- | --- |
| no-live-subscriber | short-large | mean_turn_ms | 7.410, 7.338, 7.737, 7.438 | 7.826, 7.390, 7.407, 7.467 | 5.610, 0.703, -4.270, 0.401 |
| no-live-subscriber | short-large | turn_cpu_ms_per_turn | 7.812, 7.500, 7.500, 7.500 | 8.125, 7.188, 7.500, 7.500 | 4.000, -4.167, 0.000, 0.000 |
| no-live-subscriber | short-large | turn_write_mib_per_turn | 0.704, 0.706, 0.704, 0.702 | 0.704, 0.704, 0.705, 0.703 | -0.104, -0.311, 0.156, 0.122 |
| no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 2550.000, 2560.000, 2560.000, 2600.000 | 2640.000, 2550.000, 2550.000, 2560.000 | 3.529, -0.391, -0.391, -1.538 |
| no-live-subscriber | short-large | through_cleanup_process_write_mib | 61.273, 61.344, 61.406, 61.285 | 61.453, 61.445, 61.477, 61.391 | 0.293, 0.166, 0.115, 0.172 |
| no-live-subscriber | short-small | mean_turn_ms | 6.523, 6.522, 6.518, 6.496 | 6.576, 6.540, 6.926, 6.575 | 0.820, 0.274, 6.253, 1.222 |
| no-live-subscriber | short-small | turn_cpu_ms_per_turn | 6.875, 6.562, 6.562, 6.875 | 6.875, 6.562, 7.500, 6.875 | 0.000, 0.000, 14.286, 0.000 |
| no-live-subscriber | short-small | turn_write_mib_per_turn | 0.577, 0.576, 0.577, 0.577 | 0.577, 0.577, 0.577, 0.577 | 0.064, 0.148, 0.042, 0.064 |
| no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 710.000, 710.000, 710.000, 700.000 | 710.000, 700.000, 730.000, 710.000 | 0.000, -1.408, 2.817, 1.429 |
| no-live-subscriber | short-small | through_cleanup_process_write_mib | 23.266, 23.262, 23.270, 23.246 | 23.281, 23.285, 23.289, 23.289 | 0.067, 0.101, 0.084, 0.185 |
| live-subscriber | short-large | mean_turn_ms | 8.780, 9.730, 8.814, 9.325 | 8.132, 8.302, 8.268, 8.150 | -7.379, -14.675, -6.199, -12.603 |
| live-subscriber | short-large | turn_cpu_ms_per_turn | 9.688, 10.312, 10.312, 10.625 | 9.375, 9.688, 9.375, 9.688 | -3.226, -6.061, -9.091, -8.824 |
| live-subscriber | short-large | turn_write_mib_per_turn | 0.703, 0.705, 0.704, 0.707 | 0.703, 0.704, 0.704, 0.708 | -0.069, -0.242, 0.052, 0.173 |
| live-subscriber | short-large | through_cleanup_self_cpu_ms | 2780.000, 2810.000, 2790.000, 2790.000 | 2750.000, 2770.000, 2750.000, 2750.000 | -1.079, -1.423, -1.434, -1.434 |
| live-subscriber | short-large | through_cleanup_process_write_mib | 61.332, 61.457, 61.457, 61.523 | 61.375, 61.438, 61.449, 61.637 | 0.070, -0.032, -0.013, 0.184 |
| live-subscriber | short-small | mean_turn_ms | 7.731, 7.580, 7.492, 7.397 | 7.616, 7.491, 7.207, 7.817 | -1.486, -1.172, -3.792, 5.668 |
| live-subscriber | short-small | turn_cpu_ms_per_turn | 8.750, 8.750, 8.750, 8.750 | 8.750, 8.750, 8.438, 9.375 | 0.000, 0.000, -3.571, 7.143 |
| live-subscriber | short-small | turn_write_mib_per_turn | 0.577, 0.576, 0.576, 0.577 | 0.577, 0.577, 0.578, 0.579 | 0.042, 0.085, 0.360, 0.402 |
| live-subscriber | short-small | through_cleanup_self_cpu_ms | 770.000, 780.000, 790.000, 780.000 | 780.000, 790.000, 780.000, 840.000 | 1.299, 1.282, -1.266, 7.692 |
| live-subscriber | short-small | through_cleanup_process_write_mib | 23.281, 23.258, 23.234, 23.270 | 23.301, 23.273, 23.316, 23.340 | 0.084, 0.067, 0.353, 0.302 |

## navigation independent hot comparison

32 hot turns after8/64 history; four alternating pairs per cell. CPU includes driver,10ms process ticks. live-subscriber excludes AppServer/WebUI transport.

| Mode | History case | Metric | Before | After | Paired % |
| --- | --- | --- | --- | --- | --- |
| no-live-subscriber | short-large | mean_turn_ms | 12.505 | 39.108 | 216.263 |
| no-live-subscriber | short-large | turn_cpu_ms_per_turn | 9.219 | 9.375 | 3.391 |
| no-live-subscriber | short-large | turn_write_mib_per_turn | 0.706 | 0.705 | -0.320 |
| no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 2800.000 | 2835.000 | 1.250 |
| no-live-subscriber | short-large | through_cleanup_process_write_mib | 61.477 | 61.467 | 0.006 |
| no-live-subscriber | short-small | mean_turn_ms | 15.826 | 14.999 | -4.767 |
| no-live-subscriber | short-small | turn_cpu_ms_per_turn | 8.438 | 8.125 | -3.704 |
| no-live-subscriber | short-small | turn_write_mib_per_turn | 0.577 | 0.576 | -0.074 |
| no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 770.000 | 780.000 | 1.316 |
| no-live-subscriber | short-small | through_cleanup_process_write_mib | 23.283 | 23.371 | 0.386 |
| live-subscriber | short-large | mean_turn_ms | 15.502 | 16.486 | 16.085 |
| live-subscriber | short-large | turn_cpu_ms_per_turn | 10.938 | 11.562 | 8.571 |
| live-subscriber | short-large | turn_write_mib_per_turn | 0.704 | 0.704 | 0.043 |
| live-subscriber | short-large | through_cleanup_self_cpu_ms | 2925.000 | 2990.000 | 2.585 |
| live-subscriber | short-large | through_cleanup_process_write_mib | 61.422 | 61.473 | 0.134 |
| live-subscriber | short-small | mean_turn_ms | 12.129 | 14.665 | 22.910 |
| live-subscriber | short-small | turn_cpu_ms_per_turn | 10.000 | 10.625 | 6.674 |
| live-subscriber | short-small | turn_write_mib_per_turn | 0.577 | 0.576 | -0.159 |
| live-subscriber | short-small | through_cleanup_self_cpu_ms | 855.000 | 890.000 | 1.800 |
| live-subscriber | short-small | through_cleanup_process_write_mib | 23.291 | 23.373 | 0.302 |

| Mode | Case | Metric | Before pairs | After pairs | Paired % values |
| --- | --- | --- | --- | --- | --- |
| no-live-subscriber | short-large | mean_turn_ms | 8.252, 11.018, 18.467, 13.993 | 218.519, 38.124, 19.562, 40.091 | 2548.038, 246.018, 5.926, 186.508 |
| no-live-subscriber | short-large | turn_cpu_ms_per_turn | 8.438, 9.375, 9.062, 9.375 | 9.375, 9.688, 9.375, 9.375 | 11.111, 3.333, 3.448, 0.000 |
| no-live-subscriber | short-large | turn_write_mib_per_turn | 0.704, 0.709, 0.705, 0.707 | 0.706, 0.705, 0.703, 0.704 | 0.277, -0.551, -0.225, -0.414 |
| no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 2650.000, 2800.000, 2800.000, 2800.000 | 2850.000, 2780.000, 2820.000, 2850.000 | 7.547, -0.714, 0.714, 1.786 |
| no-live-subscriber | short-large | through_cleanup_process_write_mib | 61.484, 61.250, 61.477, 61.477 | 61.484, 61.430, 61.484, 61.449 | 0.000, 0.293, 0.013, -0.044 |
| no-live-subscriber | short-small | mean_turn_ms | 7.260, 17.745, 15.913, 15.739 | 6.791, 14.574, 15.424, 23.460 | -6.461, -17.874, -3.074, 49.054 |
| no-live-subscriber | short-small | turn_cpu_ms_per_turn | 7.500, 8.438, 8.438, 8.438 | 7.188, 8.125, 8.125, 8.750 | -4.167, -3.704, -3.704, 3.704 |
| no-live-subscriber | short-small | turn_write_mib_per_turn | 0.577, 0.577, 0.577, 0.577 | 0.577, 0.577, 0.576, 0.576 | -0.063, 0.021, -0.085, -0.169 |
| no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 730.000, 760.000, 780.000, 780.000 | 730.000, 780.000, 780.000, 810.000 | 0.000, 2.632, 0.000, 3.846 |
| no-live-subscriber | short-small | through_cleanup_process_write_mib | 23.289, 23.270, 23.277, 23.289 | 23.383, 23.379, 23.363, 23.363 | 0.403, 0.470, 0.369, 0.319 |
| live-subscriber | short-large | mean_turn_ms | 15.802, 36.682, 15.202, 8.777 | 17.477, 15.496, 75.482, 10.671 | 10.597, -57.755, 396.519, 21.572 |
| live-subscriber | short-large | turn_cpu_ms_per_turn | 10.938, 11.250, 10.938, 10.000 | 11.875, 10.938, 11.875, 11.250 | 8.571, -2.778, 8.571, 12.500 |
| live-subscriber | short-large | turn_write_mib_per_turn | 0.703, 0.704, 0.704, 0.704 | 0.706, 0.705, 0.703, 0.703 | 0.382, 0.139, -0.052, -0.225 |
| live-subscriber | short-large | through_cleanup_self_cpu_ms | 2920.000, 2930.000, 2930.000, 2860.000 | 3230.000, 2960.000, 3020.000, 2920.000 | 10.616, 1.024, 3.072, 2.098 |
| live-subscriber | short-large | through_cleanup_process_write_mib | 61.418, 61.332, 61.426, 61.449 | 61.629, 61.457, 61.371, 61.488 | 0.343, 0.204, -0.089, 0.064 |
| live-subscriber | short-small | mean_turn_ms | 11.059, 13.554, 13.198, 7.801 | 14.686, 15.319, 14.645, 10.718 | 32.794, 13.026, 10.958, 37.388 |
| live-subscriber | short-small | turn_cpu_ms_per_turn | 10.312, 10.312, 9.688, 9.062 | 10.938, 11.250, 10.312, 9.688 | 6.061, 9.091, 6.452, 6.897 |
| live-subscriber | short-small | turn_write_mib_per_turn | 0.577, 0.577, 0.578, 0.578 | 0.576, 0.576, 0.577, 0.576 | -0.148, -0.106, -0.169, -0.232 |
| live-subscriber | short-small | through_cleanup_self_cpu_ms | 870.000, 910.000, 840.000, 820.000 | 980.000, 920.000, 860.000, 830.000 | 12.644, 1.099, 2.381, 1.220 |
| live-subscriber | short-small | through_cleanup_process_write_mib | 23.293, 23.289, 23.316, 23.289 | 23.379, 23.352, 23.379, 23.367 | 0.369, 0.268, 0.268, 0.335 |

## Navigation retake after severe external I/O stress

One bounded four-pair retake per hot cell; original matrix above is retained. No exclusivity claim: instantaneous device busy <5% at preflight, global PSI remained high. Short-history adverse values are not dropped.

| Mode | Case | Metric | Before | After | Paired % |
| --- | --- | --- | --- | --- | --- |
| no-live-subscriber | short-large | mean_turn_ms | 8.508 | 7.948 | -6.519 |
| no-live-subscriber | short-large | turn_cpu_ms_per_turn | 8.438 | 8.281 | -1.852 |
| no-live-subscriber | short-large | turn_write_mib_per_turn | 0.704 | 0.703 | -0.026 |
| no-live-subscriber | short-small | mean_turn_ms | 6.787 | 7.259 | 6.918 |
| no-live-subscriber | short-small | turn_cpu_ms_per_turn | 7.031 | 7.656 | 8.893 |
| no-live-subscriber | short-small | turn_write_mib_per_turn | 0.577 | 0.576 | -0.211 |
| live-subscriber | short-large | mean_turn_ms | 9.118 | 8.911 | -0.706 |
| live-subscriber | short-large | turn_cpu_ms_per_turn | 10.156 | 9.844 | -1.562 |
| live-subscriber | short-large | turn_write_mib_per_turn | 0.704 | 0.704 | 0.009 |
| live-subscriber | short-small | mean_turn_ms | 7.882 | 8.159 | 3.497 |
| live-subscriber | short-small | turn_cpu_ms_per_turn | 9.219 | 9.375 | 3.448 |
| live-subscriber | short-small | turn_write_mib_per_turn | 0.577 | 0.576 | -0.169 |

## History statistics

Public API plus probe JSON conversion. Two alternating pairs; first is process-first check after appends, not disk-cold. Reopen means same process/new connection. All cut values agree.

| Step | Events | Mode | Read / µs | Before pairs | After pairs | Paired % |
| --- | --- | --- | --- | --- | --- | --- |
| statistics | 64 | plain | cut63_first_ns | 126.570, 114.968 | 34.067, 29.616 | -73.084, -74.240 |
| statistics | 64 | plain | cut63_repeated_ns | 28.805, 28.236 | 11.000, 10.977 | -61.812, -61.124 |
| statistics | 64 | plain | cut64_first_ns | 13.373, 13.422 | 13.079, 12.614 | -2.198, -6.020 |
| statistics | 64 | plain | cut64_repeated_ns | 11.636, 11.617 | 10.851, 10.842 | -6.751, -6.671 |
| statistics | 64 | plain | reopened63_first_ns | 175.312, 168.774 | 110.368, 97.805 | -37.045, -42.050 |
| statistics | 64 | plain | reopened63_repeated_ns | 31.212, 30.669 | 11.636, 11.528 | -62.720, -62.410 |
| statistics | 512 | plain | cut63_first_ns | 142.267, 142.701 | 40.322, 39.414 | -71.658, -72.380 |
| statistics | 512 | plain | cut63_repeated_ns | 28.930, 29.096 | 11.261, 11.112 | -61.073, -61.809 |
| statistics | 512 | plain | cut64_first_ns | 17.251, 17.163 | 15.120, 15.022 | -12.353, -12.475 |
| statistics | 512 | plain | cut64_repeated_ns | 11.899, 11.926 | 11.236, 11.031 | -5.576, -7.505 |
| statistics | 512 | plain | reopened63_first_ns | 175.146, 173.411 | 105.123, 111.893 | -39.980, -35.475 |
| statistics | 512 | plain | reopened63_repeated_ns | 30.794, 31.128 | 11.682, 11.764 | -62.062, -62.206 |
| statistics | 512 | concurrent | cut63_first_ns | 145.258, 148.473 | 47.301, 53.837 | -67.437, -63.740 |
| statistics | 512 | concurrent | cut63_repeated_ns | 28.326, 29.184 | 10.797, 11.048 | -61.881, -62.144 |
| statistics | 512 | concurrent | cut64_first_ns | 17.194, 17.587 | 14.340, 14.691 | -16.599, -16.467 |
| statistics | 512 | concurrent | cut64_repeated_ns | 11.586, 11.948 | 10.764, 10.892 | -7.103, -8.838 |
| statistics | 512 | concurrent | reopened63_first_ns | 178.219, 189.114 | 124.905, 105.843 | -29.915, -44.032 |
| statistics | 512 | concurrent | reopened63_repeated_ns | 31.787, 33.103 | 14.258, 12.177 | -55.146, -63.215 |
| navigation | 64 | plain | cut63_first_ns | 40.865, 48.911 | 42.206, 39.305 | 3.282, -19.640 |
| navigation | 64 | plain | cut63_repeated_ns | 10.921, 11.040 | 10.934, 10.839 | 0.119, -1.812 |
| navigation | 64 | plain | cut64_first_ns | 11.114, 11.296 | 12.637, 12.394 | 13.703, 9.720 |
| navigation | 64 | plain | cut64_repeated_ns | 10.829, 10.970 | 10.883, 10.842 | 0.494, -1.171 |
| navigation | 64 | plain | reopened63_first_ns | 118.104, 116.169 | 120.294, 109.489 | 1.854, -5.750 |
| navigation | 64 | plain | reopened63_repeated_ns | 11.662, 11.691 | 11.706, 11.693 | 0.373, 0.013 |
| navigation | 512 | plain | cut63_first_ns | 44.688, 50.743 | 45.263, 79.861 | 1.287, 57.383 |
| navigation | 512 | plain | cut63_repeated_ns | 11.095, 11.073 | 11.118, 17.512 | 0.203, 58.150 |
| navigation | 512 | plain | cut64_first_ns | 15.534, 15.845 | 15.709, 26.063 | 1.127, 64.487 |
| navigation | 512 | plain | cut64_repeated_ns | 10.970, 11.040 | 11.153, 16.985 | 1.673, 53.843 |
| navigation | 512 | plain | reopened63_first_ns | 116.635, 115.703 | 113.732, 204.150 | -2.489, 76.443 |
| navigation | 512 | plain | reopened63_repeated_ns | 11.973, 11.837 | 11.953, 17.905 | -0.171, 51.267 |
| navigation | 512 | concurrent | cut63_first_ns | 50.078, 59.098 | 47.611, 48.721 | -4.926, -17.559 |
| navigation | 512 | concurrent | cut63_repeated_ns | 11.717, 11.252 | 10.993, 11.031 | -6.179, -1.960 |
| navigation | 512 | concurrent | cut64_first_ns | 17.486, 16.551 | 15.292, 14.967 | -12.547, -9.570 |
| navigation | 512 | concurrent | cut64_repeated_ns | 11.633, 11.107 | 10.882, 10.963 | -6.452, -1.288 |
| navigation | 512 | concurrent | reopened63_first_ns | 119.711, 117.233 | 116.663, 121.844 | -2.546, 3.933 |
| navigation | 512 | concurrent | reopened63_repeated_ns | 12.685, 12.382 | 12.115, 12.360 | -4.493, -0.178 |

## Storage diagnostics

One synchronous strace/SQL product per history/build, separate from timing.2KiB pages. WAL frames are page writes, not device writes; db growth and overflow deltas are endpoint net counts. Shared endpoint page-owner labels are not exact function attribution.

| Build | History | WAL frames | Commits | WAL bytes | Used DB growth bytes | Statistics bytes | Statistics records / bytes | Navigation page / byte delta | Source overflow net pages |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| baseline | short-small-rust | 7000 | 160 | 14504000 | 1564672 | 18823 | {'observation_statistics': [192, 10620], 'observation_statistics_checkpoint': [32, 8203]} | {'rows': 165, 'bytes': 76849} | 0 |
| baseline | short-large-rust | 7772 | 160 | 16103584 | 1570816 | 19292 | {'observation_statistics': [192, 11004], 'observation_statistics_checkpoint': [32, 8288]} | {'rows': 160, 'bytes': 71424} | 0 |
| statistics | short-small-rust | 6995 | 160 | 14493640 | 1562624 | 73306 | {'observation_statistics': [192, 73306]} | {'rows': 165, 'bytes': 76849} | 0 |
| statistics | short-large-rust | 7786 | 160 | 16132592 | 1574912 | 74047 | {'observation_statistics': [192, 74047]} | {'rows': 160, 'bytes': 71424} | 0 |
| candidate | short-small-rust | 6997 | 160 | 14497784 | 1568768 | 73241 | {'observation_statistics': [192, 73241]} | {'rows': 165, 'bytes': 78896} | 0 |
| candidate | short-large-rust | 7800 | 160 | 16161600 | 1579008 | 74048 | {'observation_statistics': [192, 74048]} | {'rows': 160, 'bytes': 71104} | 0 |

## Native memory

Fresh fixed cohort: four balanced pairs; sampled ordinary GNU-allocator process endpoints, not peaks or live heap. All128sessions have8turns at the seeding endpoint.

| Endpoint | Metric | Before | After | Paired % | Before values | After values |
| --- | --- | --- | --- | --- | --- | --- |
| all_subscribed_empty | anon_pss_mib | 168.090 | 168.236 | -0.106% | 168.086, 168.863, 168.094, 167.859 | 168.316, 168.441, 168.156, 167.324 |
| all_subscribed_empty | rss_mib | 224.344 | 224.420 | -0.063% | 224.355, 224.840, 223.883, 224.332 | 224.402, 224.512, 224.438, 223.387 |
| seeded_history_8 | anon_pss_mib | 480.975 | 482.930 | 0.439% | 481.352, 481.043, 480.285, 480.906 | 483.145, 483.844, 482.715, 482.355 |
| seeded_history_8 | rss_mib | 545.234 | 547.238 | 0.425% | 545.387, 545.082, 544.246, 545.406 | 547.230, 547.875, 547.246, 546.895 |
| all_unloaded | anon_pss_mib | 524.191 | 525.164 | 0.172% | 524.883, 524.098, 523.637, 524.285 | 525.855, 527.609, 524.473, 523.973 |
| all_unloaded | rss_mib | 585.773 | 586.879 | 0.218% | 586.105, 585.512, 585.098, 586.035 | 587.379, 588.953, 586.379, 585.762 |

| Eight-turn growth /session MiB | Before | After | Paired % | Before values | After values |
| --- | --- | --- | --- | --- | --- |
| Anonymous PSS | 2.442 | 2.460 | 0.696% | 2.447, 2.439, 2.439, 2.446 | 2.460, 2.464, 2.457, 2.461 |

## Native activity

Server PID only. Original fixed four pairs retained as shared-load diagnostics; fresh fixed has four balanced pairs. Fixed1/4/8:128×8seed,16rounds per width; rotating4:128rounds/512calls. Deep:8×64,32rounds width4, two balanced pairs. Latency excludes held first round; CPU/write includes it. P95/P99 are finite sample summaries.

| Family | Width | Metric | Before | After | Paired % | Before values | After values | Paired % values |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ordinary | 1 | wall_ms | 26.346 | 10.232 | -58.914% | 30.991, 18.882, 21.701, 308.389 | 9.646, 9.638, 17.278, 10.818 | -68.874, -48.954, -20.380, -96.492 |
| ordinary | 1 | cpu_ms | 14.688 | 11.875 | -13.636% | 13.750, 13.750, 17.500, 15.625 | 11.875, 11.875, 16.250, 11.250 | -13.636, -13.636, -7.143, -28.000 |
| ordinary | 1 | write_kib | 640.875 | 644.500 | 0.527% | 641.750, 640.000, 640.000, 642.500 | 644.000, 644.500, 644.750, 644.500 | 0.351, 0.703, 0.742, 0.311 |
| ordinary | 1 | p95_ms | 170.516 | 13.579 | -67.583% | 313.436, 22.522, 27.596, 1991.454 | 12.798, 13.682, 22.795, 13.476 | -95.917, -39.249, -17.397, -99.323 |
| ordinary | 1 | p99_ms | 170.516 | 13.579 | -67.583% | 313.436, 22.522, 27.596, 1991.454 | 12.798, 13.682, 22.795, 13.476 | -95.917, -39.249, -17.397, -99.323 |
| ordinary | 4 | wall_ms | 49.479 | 34.088 | -30.060% | 175.243, 37.624, 47.647, 51.311 | 36.588, 31.587, 41.614, 28.695 | -79.122, -16.045, -12.662, -44.076 |
| ordinary | 4 | cpu_ms | 15.000 | 12.188 | -8.677% | 13.438, 12.656, 16.562, 16.719 | 12.500, 11.875, 14.844, 11.094 | -6.977, -6.173, -10.377, -33.645 |
| ordinary | 4 | write_kib | 667.281 | 670.656 | 0.506% | 667.500, 667.062, 666.500, 668.938 | 670.562, 670.750, 670.312, 670.812 | 0.459, 0.553, 0.572, 0.280 |
| ordinary | 4 | p95_ms | 64.866 | 48.599 | -12.996% | 432.321, 54.143, 56.947, 72.785 | 754.227, 36.597, 60.601, 35.204 | 74.460, -32.408, 6.417, -51.633 |
| ordinary | 4 | p99_ms | 74.284 | 52.424 | -21.169% | 692.728, 70.959, 64.139, 77.608 | 1125.096, 38.755, 66.093, 37.593 | 62.415, -45.384, 3.046, -51.560 |
| ordinary | 8 | wall_ms | 92.886 | 72.913 | -10.838% | 96.613, 67.378, 96.305, 89.467 | 150.043, 60.346, 85.480, 51.906 | 55.304, -10.437, -11.240, -41.983 |
| ordinary | 8 | cpu_ms | 14.531 | 12.578 | -7.835% | 12.812, 12.734, 17.422, 16.250 | 13.047, 12.109, 15.547, 11.094 | 1.829, -4.908, -10.762, -31.731 |
| ordinary | 8 | write_kib | 693.016 | 696.453 | 0.496% | 693.188, 692.844, 692.406, 694.562 | 696.500, 696.406, 696.656, 696.031 | 0.478, 0.514, 0.614, 0.211 |
| ordinary | 8 | p95_ms | 110.483 | 88.493 | -15.236% | 800.966, 91.754, 115.827, 105.139 | 1046.413, 73.840, 103.146, 64.609 | 30.644, -19.524, -10.948, -38.549 |
| ordinary | 8 | p99_ms | 129.432 | 93.137 | -22.087% | 1209.132, 140.661, 118.204, 109.890 | 4449.788, 77.410, 108.864, 70.030 | 268.015, -44.967, -7.902, -36.273 |
| ordinary-fresh | 1 | wall_ms | 10.488 | 11.814 | 12.633% | 10.002, 12.318, 10.975, 9.382 | 82.425, 10.585, 9.464, 13.044 | 724.067, -14.065, -13.765, 39.031 |
| ordinary-fresh | 1 | cpu_ms | 12.188 | 11.875 | -4.868% | 10.625, 13.750, 12.500, 11.875 | 22.500, 11.250, 10.625, 12.500 | 111.765, -18.182, -15.000, 5.263 |
| ordinary-fresh | 1 | write_kib | 645.250 | 648.375 | 0.465% | 645.250, 645.750, 644.250, 645.250 | 648.000, 648.750, 648.500, 648.250 | 0.426, 0.465, 0.660, 0.465 |
| ordinary-fresh | 1 | p95_ms | 14.356 | 16.625 | 16.775% | 13.485, 21.107, 15.228, 13.414 | 310.960, 15.706, 16.138, 17.113 | 2206.038, -25.587, 5.975, 27.575 |
| ordinary-fresh | 1 | p99_ms | 14.356 | 16.625 | 16.775% | 13.485, 21.107, 15.228, 13.414 | 310.960, 15.706, 16.138, 17.113 | 2206.038, -25.587, 5.975, 27.575 |
| ordinary-fresh | 4 | wall_ms | 27.853 | 29.111 | 5.030% | 27.531, 29.299, 28.175, 26.333 | 172.086, 28.708, 27.141, 29.514 | 525.061, -2.019, -3.673, 12.080 |
| ordinary-fresh | 4 | cpu_ms | 11.172 | 11.250 | 2.143% | 11.250, 11.094, 11.250, 10.938 | 14.844, 11.094, 11.094, 11.406 | 31.944, 0.000, -1.389, 4.286 |
| ordinary-fresh | 4 | write_kib | 671.031 | 674.406 | 0.498% | 671.062, 671.000, 670.875, 671.438 | 674.500, 674.375, 674.188, 674.438 | 0.512, 0.503, 0.494, 0.447 |
| ordinary-fresh | 4 | p95_ms | 37.679 | 37.046 | 0.021% | 36.621, 38.738, 39.327, 35.254 | 455.621, 33.734, 34.467, 39.626 | 1144.155, -12.916, -12.357, 12.400 |
| ordinary-fresh | 4 | p99_ms | 40.309 | 41.963 | 6.292% | 39.226, 41.392, 43.994, 36.078 | 657.916, 34.795, 40.185, 43.742 | 1577.253, -15.938, -8.657, 21.242 |
| ordinary-fresh | 8 | wall_ms | 57.103 | 56.951 | -0.741% | 54.520, 57.834, 59.600, 56.372 | 59.844, 58.243, 55.660, 55.139 | 9.765, 0.706, -6.612, -2.187 |
| ordinary-fresh | 8 | cpu_ms | 11.562 | 11.406 | -1.037% | 11.250, 11.719, 11.797, 11.406 | 11.641, 11.797, 11.172, 11.094 | 3.472, 0.667, -5.298, -2.740 |
| ordinary-fresh | 8 | write_kib | 697.281 | 700.250 | 0.428% | 697.281, 696.938, 697.812, 697.281 | 700.188, 700.062, 700.312, 700.344 | 0.417, 0.448, 0.358, 0.439 |
| ordinary-fresh | 8 | p95_ms | 70.308 | 72.160 | 2.468% | 65.198, 75.155, 76.126, 65.461 | 94.422, 75.141, 69.179, 68.705 | 44.823, -0.019, -9.125, 4.955 |
| ordinary-fresh | 8 | p99_ms | 73.434 | 81.396 | 3.139% | 68.037, 77.685, 89.435, 69.183 | 181.315, 81.433, 81.359, 70.189 | 166.493, 4.824, -9.030, 1.453 |
| rotating | 4 | wall_ms | 29.195 | 29.513 | 0.870% | 28.574, 29.193, 29.198, 29.945 | 28.893, 28.624, 30.156, 30.133 | 1.115, -1.950, 3.283, 0.625 |
| rotating | 4 | cpu_ms | 11.270 | 11.367 | -0.955% | 11.230, 11.309, 11.230, 11.836 | 11.074, 11.250, 11.641, 11.484 | -1.391, -0.518, 3.652, -2.970 |
| rotating | 4 | write_kib | 628.570 | 632.242 | 0.594% | 628.508, 628.633, 628.469, 629.016 | 632.297, 632.383, 632.188, 632.172 | 0.603, 0.597, 0.592, 0.502 |
| rotating | 4 | p95_ms | 41.888 | 40.968 | -2.079% | 42.964, 40.812, 37.541, 311.349 | 40.131, 41.805, 47.736, 39.757 | -6.593, 2.435, 27.157, -87.231 |
| rotating | 4 | p99_ms | 47.359 | 48.051 | 1.673% | 48.940, 45.777, 41.849, 550.790 | 46.680, 49.422, 57.888, 45.002 | -4.616, 7.962, 38.326, -91.830 |
| deep | 4 | wall_ms | 31.708 | 102.469 | 224.564% | 31.513, 31.903 | 173.818, 31.121 | 451.579, -2.452 |
| deep | 4 | cpu_ms | 12.070 | 12.734 | 5.398% | 12.188, 11.953 | 14.141, 11.328 | 16.026, -5.229 |
| deep | 4 | write_kib | 728.922 | 729.344 | 0.058% | 729.375, 728.469 | 728.094, 730.594 | -0.176, 0.292 |
| deep | 4 | p95_ms | 43.347 | 306.879 | 655.348% | 40.307, 46.386 | 576.829, 36.929 | 1331.084, -20.387 |
| deep | 4 | p99_ms | 52.614 | 523.834 | 871.530% | 54.086, 51.141 | 991.009, 56.660 | 1732.269, 10.791 |

## Deep-history memory

Two separate pairs, eight sessions,8/16/32/64turn endpoints; not pooled into128×8.

| Endpoint | Metric | Before | After | Paired % | Before values | After values |
| --- | --- | --- | --- | --- | --- | --- |
| all_subscribed_empty | anon_pss_mib | 28.486 | 28.197 | -1.003% | 27.910, 29.062 | 27.793, 28.602 |
| all_subscribed_empty | rss_mib | 80.756 | 80.361 | -0.485% | 80.012, 81.500 | 79.953, 80.770 |
| seeded_history_8 | anon_pss_mib | 52.133 | 52.010 | -0.231% | 51.691, 52.574 | 51.883, 52.137 |
| seeded_history_8 | rss_mib | 112.465 | 112.455 | -0.009% | 112.105, 112.824 | 112.105, 112.805 |
| seeded_history_16 | anon_pss_mib | 65.072 | 65.250 | 0.272% | 64.980, 65.164 | 64.488, 66.012 |
| seeded_history_16 | rss_mib | 125.404 | 125.695 | 0.232% | 125.395, 125.414 | 124.711, 126.680 |
| seeded_history_32 | anon_pss_mib | 87.344 | 88.098 | 0.862% | 87.176, 87.512 | 87.234, 88.961 |
| seeded_history_32 | rss_mib | 147.676 | 148.543 | 0.587% | 147.590, 147.762 | 147.457, 149.629 |
| seeded_history_64 | anon_pss_mib | 121.639 | 121.781 | 0.117% | 121.137, 122.141 | 121.148, 122.414 |
| seeded_history_64 | rss_mib | 181.971 | 182.227 | 0.140% | 181.551, 182.391 | 181.371, 183.082 |

## Request Source inspection

One ordinary public-API probe per history/build on consistent closed images. Header/page/range first calls are sequential in one connection; repeated calls include intervening child-field correctness checks. Reopen is same process/new connection. Whole Source bytes and every top-level child agree; old request survives later requests. No cross-process Source cache is claimed.

| Build | History | Request | Operation | Encoded bytes | First µs | Repeated µs |
| --- | --- | --- | --- | --- | --- | --- |
| baseline | short-small | old | header_ns | 15030 | 287.996 | 174.846 |
| baseline | short-small | old | page_ns | 15030 | 2408.830 | 2054.311 |
| baseline | short-small | old | range_ns | 15030 | 262.041 | 245.523 |
| baseline | short-small | latest | header_ns | 94599 | 1751.114 | 1813.409 |
| baseline | short-small | latest | page_ns | 94599 | 4383.451 | 4415.935 |
| baseline | short-small | latest | range_ns | 94599 | 79540.945 | 79357.205 |
| baseline | short-small | reopened_old | header_ns | 15030 | 260.025 | 174.685 |
| baseline | short-small | reopened_old | page_ns | 15030 | 2154.685 | 2078.234 |
| baseline | short-small | reopened_old | range_ns | 15030 | 251.252 | 247.838 |
| baseline | short-large | old | header_ns | 15030 | 281.009 | 181.315 |
| baseline | short-large | old | page_ns | 15030 | 2432.787 | 2086.514 |
| baseline | short-large | old | range_ns | 15030 | 272.148 | 249.491 |
| baseline | short-large | latest | header_ns | 210405 | 2271.164 | 2262.762 |
| baseline | short-large | latest | page_ns | 210405 | 5966.373 | 5926.629 |
| baseline | short-large | latest | range_ns | 210405 | 332317.234 | 330986.114 |
| baseline | short-large | reopened_old | header_ns | 15030 | 258.099 | 178.028 |
| baseline | short-large | reopened_old | page_ns | 15030 | 2170.481 | 2093.649 |
| baseline | short-large | reopened_old | range_ns | 15030 | 257.300 | 248.186 |
| statistics | short-small | old | header_ns | 15030 | 295.465 | 175.917 |
| statistics | short-small | old | page_ns | 15030 | 2445.366 | 2082.818 |
| statistics | short-small | old | range_ns | 15030 | 268.823 | 248.198 |
| statistics | short-small | latest | header_ns | 94599 | 1834.602 | 1816.962 |
| statistics | short-small | latest | page_ns | 94599 | 4450.749 | 4469.785 |
| statistics | short-small | latest | range_ns | 94599 | 80787.206 | 80668.694 |
| statistics | short-small | reopened_old | header_ns | 15030 | 260.661 | 176.464 |
| statistics | short-small | reopened_old | page_ns | 15030 | 2133.432 | 2076.184 |
| statistics | short-small | reopened_old | range_ns | 15030 | 253.049 | 246.655 |
| statistics | short-large | old | header_ns | 15030 | 300.689 | 176.135 |
| statistics | short-large | old | page_ns | 15030 | 2533.281 | 2070.983 |
| statistics | short-large | old | range_ns | 15030 | 275.746 | 243.606 |
| statistics | short-large | latest | header_ns | 210405 | 2177.609 | 2257.983 |
| statistics | short-large | latest | page_ns | 210405 | 5738.810 | 5896.056 |
| statistics | short-large | latest | range_ns | 210405 | 326771.799 | 328299.510 |
| statistics | short-large | reopened_old | header_ns | 15030 | 254.400 | 177.534 |
| statistics | short-large | reopened_old | page_ns | 15030 | 2168.810 | 2099.793 |
| statistics | short-large | reopened_old | range_ns | 15030 | 256.530 | 250.670 |
| candidate | short-small | old | header_ns | 15030 | 309.386 | 188.316 |
| candidate | short-small | old | page_ns | 15030 | 2613.984 | 2188.867 |
| candidate | short-small | old | range_ns | 15030 | 274.740 | 248.959 |
| candidate | short-small | latest | header_ns | 94599 | 1826.687 | 1864.294 |
| candidate | short-small | latest | page_ns | 94599 | 4465.751 | 4555.409 |
| candidate | short-small | latest | range_ns | 94599 | 79474.042 | 81121.241 |
| candidate | short-small | reopened_old | header_ns | 15030 | 273.213 | 186.072 |
| candidate | short-small | reopened_old | page_ns | 15030 | 2252.649 | 2178.675 |
| candidate | short-small | reopened_old | range_ns | 15030 | 257.577 | 251.600 |
| candidate | short-large | old | header_ns | 15030 | 311.107 | 187.393 |
| candidate | short-large | old | page_ns | 15030 | 2651.715 | 2153.595 |
| candidate | short-large | old | range_ns | 15030 | 272.298 | 247.964 |
| candidate | short-large | latest | header_ns | 210405 | 2259.415 | 2307.928 |
| candidate | short-large | latest | page_ns | 210405 | 5862.696 | 6003.453 |
| candidate | short-large | latest | range_ns | 210405 | 328702.571 | 331145.163 |
| candidate | short-large | reopened_old | header_ns | 15030 | 278.815 | 188.650 |
| candidate | short-large | reopened_old | page_ns | 15030 | 2275.811 | 2203.510 |
| candidate | short-large | reopened_old | range_ns | 15030 | 261.103 | 253.214 |

## Cold/cache diagnostic

One before/after product per side, functional/cost diagnostic rather than a timing confidence cohort.32s endpoint is128×8;130s has4×14and124×8. Cold/warm latency excludes4kept-active and2Inspector-primed sessions (122eligible); CPU/write includes128.

| Endpoint | Metric | Before | After |
| --- | --- | --- | --- |
| seeded_history_8 | anon_pss_mib | 480.641 | 484.320 |
| seeded_history_8 | rss_mib | 545.055 | 548.488 |
| cache_pages_idle | anon_pss_mib | 466.363 | 466.867 |
| cache_pages_idle | rss_mib | 530.898 | 531.219 |
| cache_statements_idle | anon_pss_mib | 432.109 | 432.434 |
| cache_statements_idle | rss_mib | 496.645 | 496.785 |
| all_unloaded | anon_pss_mib | 574.328 | 567.969 |
| all_unloaded | rss_mib | 636.176 | 629.633 |

| Rotation | Metric | Before | After |
| --- | --- | --- | --- |
| cold | settled_median_ms | 13.618 | 15.385 |
| cold | settled_p95_ms | 20.165 | 21.892 |
| warm | settled_median_ms | 9.754 | 11.452 |
| warm | settled_p95_ms | 12.010 | 16.633 |

| Side | Inspector/history/2 Source reads ms | Max kept-warm gap s | Warm gaps≥30s |
| --- | --- | --- | --- |
| before | 14.698 | 15.993 | 0 |
| after | 13.561 | 15.991 | 0 |
