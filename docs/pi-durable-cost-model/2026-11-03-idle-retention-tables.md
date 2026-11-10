# Loaded-session retention: complete measurement tables

Before: `e1d4a40dc` (prior report `e1a984771`). Trial: `f2e1273d2`. Final: `d50125842`. All new products are rustX; Pi/DeepSeek were not run. MiB = 2^20 bytes. Changes are medians of paired ratios, not ratios of the displayed medians. All pairs and adverse cells are retained.

## Final ordinary processes: four balanced pairs

128 independent SQLite conversations, eight seed turns each, all subscribed. These immediate endpoints precede the explicit cache-idle experiment. RSS and anonymous PSS are distinct process measurements.

| Phase / metric | Before median | Final median | Paired change |
| --- | --- | --- | --- |
| empty_server / anon_pss_mib | 14.191 | 14.207 | +0.110% |
| empty_server / rss_mib | 46.096 | 46.438 | +0.593% |
| empty_attached_128 / anon_pss_mib | 323.457 | 323.977 | +0.086% |
| empty_attached_128 / rss_mib | 377.482 | 378.334 | +0.183% |
| all_subscribed_empty / anon_pss_mib | 323.605 | 324.342 | +0.089% |
| all_subscribed_empty / rss_mib | 378.980 | 379.824 | +0.203% |
| seeded_history_8 / anon_pss_mib | 765.875 | 742.838 | -3.034% |
| seeded_history_8 / rss_mib | 828.768 | 806.287 | -2.752% |
| all_unloaded / anon_pss_mib | 838.379 | 811.934 | -3.141% |
| all_unloaded / rss_mib | 898.727 | 872.764 | -2.889% |
| reload_unload_cycle_2 / anon_pss_mib | 838.402 | 811.973 | -3.058% |
| reload_unload_cycle_2 / rss_mib | 898.893 | 873.037 | -2.805% |

Each primary product performs 1,235 requests. Latencies omit the deliberately held first burst; CPU/write windows include all 16 bursts (16×width requests). Native server PID only. `/proc` CPU granularity is 10ms. P95/P99 are descriptive finite samples, not service-level guarantees.

| Width / metric | Before median | Final median | Paired change |
| --- | --- | --- | --- |
| 1 / settled_ms | 9.555 | 9.713 | +0.327% |
| 1 / accepted_ms | 0.971 | 0.999 | +0.227% |
| 1 / p95_ms | 14.073 | 14.962 | +0.404% |
| 1 / p99_ms | 14.073 | 14.962 | +0.404% |
| 1 / cpu_ms_per_turn | 11.875 | 11.875 | +0.000% |
| 1 / write_kib_per_turn | 645.500 | 645.250 | +0.000% |
| 4 / settled_ms | 28.005 | 28.794 | +4.347% |
| 4 / accepted_ms | 4.927 | 4.523 | -14.914% |
| 4 / p95_ms | 33.463 | 35.285 | +5.305% |
| 4 / p99_ms | 36.521 | 37.587 | +5.813% |
| 4 / cpu_ms_per_turn | 11.328 | 11.797 | +4.129% |
| 4 / write_kib_per_turn | 671.375 | 670.938 | -0.033% |
| 8 / settled_ms | 55.974 | 55.025 | -1.750% |
| 8 / accepted_ms | 10.054 | 9.281 | -9.736% |
| 8 / p95_ms | 65.858 | 66.836 | -0.310% |
| 8 / p99_ms | 72.414 | 69.975 | -4.482% |
| 8 / cpu_ms_per_turn | 11.602 | 11.641 | +0.336% |
| 8 / write_kib_per_turn | 697.453 | 697.438 | +0.002% |

| Pair | Side | Width | Seed anon MiB | Seed RSS MiB | Settled ms | P95 ms | CPU ms/turn | Write KiB/turn |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 0 | before | 1 | 766.859 | 830.008 | 9.572 | 13.079 | 11.875 | 645.500 |
| 0 | before | 4 | 766.859 | 830.008 | 28.000 | 31.711 | 11.250 | 671.688 |
| 0 | before | 8 | 766.859 | 830.008 | 55.403 | 64.716 | 11.562 | 697.219 |
| 0 | after | 1 | 743.664 | 806.945 | 9.458 | 12.839 | 11.875 | 645.250 |
| 0 | after | 4 | 743.664 | 806.945 | 29.134 | 33.525 | 11.250 | 671.000 |
| 0 | after | 8 | 743.664 | 806.945 | 53.782 | 64.007 | 11.562 | 698.094 |
| 1 | after | 1 | 743.195 | 806.676 | 9.831 | 13.852 | 11.250 | 645.000 |
| 1 | after | 4 | 743.195 | 806.676 | 28.006 | 35.201 | 11.562 | 671.438 |
| 1 | after | 8 | 743.195 | 806.676 | 57.255 | 69.055 | 11.953 | 697.031 |
| 1 | before | 1 | 764.543 | 827.582 | 9.825 | 13.495 | 11.875 | 644.750 |
| 1 | before | 4 | 764.543 | 827.582 | 28.097 | 33.204 | 11.250 | 671.250 |
| 1 | before | 8 | 764.543 | 827.582 | 55.014 | 64.768 | 11.562 | 697.156 |
| 2 | after | 1 | 742.480 | 805.898 | 10.113 | 16.758 | 11.875 | 645.250 |
| 2 | after | 4 | 742.480 | 805.898 | 28.453 | 35.420 | 12.031 | 670.875 |
| 2 | after | 8 | 742.480 | 805.898 | 56.222 | 67.265 | 11.719 | 697.844 |
| 2 | before | 1 | 766.270 | 829.262 | 9.504 | 14.652 | 11.875 | 645.500 |
| 2 | before | 4 | 766.270 | 829.262 | 27.152 | 34.066 | 11.406 | 670.688 |
| 2 | before | 8 | 766.270 | 829.262 | 56.546 | 66.947 | 11.641 | 697.688 |
| 3 | before | 1 | 765.480 | 828.273 | 9.538 | 17.076 | 11.875 | 645.500 |
| 3 | before | 4 | 765.480 | 828.273 | 28.009 | 33.721 | 11.406 | 671.500 |
| 3 | before | 8 | 765.480 | 828.273 | 57.354 | 68.772 | 11.953 | 697.688 |
| 3 | after | 1 | 742.180 | 805.703 | 9.594 | 16.073 | 11.875 | 645.750 |
| 3 | after | 4 | 742.180 | 805.703 | 29.311 | 35.370 | 12.031 | 670.875 |
| 3 | after | 8 | 742.180 | 805.703 | 53.829 | 66.407 | 11.484 | 696.719 |

## Final idle-cache processes: two separate balanced pairs

Each extended product performs 1,515 requests. At 32s all 128 sessions still have exactly eight turns. At 130s, four kept-warm sessions have 14 turns and 124 idle sessions have eight. Later rotation points have additional turns and cannot be presented as 128×8 memory.

| Phase / metric | Before median | Final median | Paired change |
| --- | --- | --- | --- |
| all_subscribed_empty / anon_pss_mib | 323.943 | 324.336 | +0.121% |
| all_subscribed_empty / rss_mib | 379.631 | 380.178 | +0.144% |
| seeded_history_8 / anon_pss_mib | 766.941 | 742.865 | -3.139% |
| seeded_history_8 / rss_mib | 830.242 | 806.596 | -2.848% |
| cache_pages_idle / anon_pss_mib | 765.012 | 729.473 | -4.646% |
| cache_pages_idle / rss_mib | 828.434 | 793.324 | -4.238% |
| cache_statements_idle / anon_pss_mib | 772.570 | 698.402 | -9.600% |
| cache_statements_idle / rss_mib | 835.992 | 762.254 | -8.820% |
| cache_rotation_cold / anon_pss_mib | 815.178 | 799.938 | -1.871% |
| cache_rotation_cold / rss_mib | 879.162 | 864.414 | -1.679% |
| cache_rotation_warm / anon_pss_mib | 856.090 | 839.611 | -1.926% |
| cache_rotation_warm / rss_mib | 920.074 | 904.088 | -1.739% |
| all_unloaded / anon_pss_mib | 923.521 | 898.350 | -2.727% |
| all_unloaded / rss_mib | 984.256 | 959.639 | -2.502% |

Latency below uses the 122 predeclared cold-eligible sessions: indices 4…124 and 127. Sessions 0…3 were kept warm; 125/126 were primed by Inspector. CPU and writes use the entire 128-request rotation. The warm rotation repeats the same order before a 30s idle gap.

| Rotation / metric | Before median | Final median | Paired change |
| --- | --- | --- | --- |
| cold / settled_median_ms | 10.076 | 13.142 | +30.524% |
| cold / accepted_median_ms | 1.596 | 1.899 | +18.953% |
| cold / settled_p95_ms | 13.811 | 15.689 | +14.308% |
| cold / cpu_ms_per_turn | 11.484 | 14.883 | +29.592% |
| cold / write_kib_per_turn | 483.672 | 483.672 | +0.000% |
| warm / settled_median_ms | 9.607 | 10.042 | +4.565% |
| warm / accepted_median_ms | 1.538 | 1.508 | -1.793% |
| warm / settled_p95_ms | 12.188 | 14.668 | +21.018% |
| warm / cpu_ms_per_turn | 11.367 | 11.445 | +0.694% |
| warm / write_kib_per_turn | 502.906 | 502.938 | +0.006% |

Kept-warm latency excludes the first four calls after page-cache reclamation. The CPU counter spans approximately 98s of calls, idle time and maintenance; it is not CPU/turn. Inspector includes checkpoint/history and two exact Source content reads.

| Metric | Before median | Final median | Paired change |
| --- | --- | --- | --- |
| settled_median_ms | 19.021 | 18.838 | +0.354% |
| accepted_median_ms | 3.188 | 3.293 | +3.282% |
| settled_p95_ms | 30.391 | 29.747 | -1.808% |
| mixed98s_cpu_ms | 680.000 | 640.000 | -5.758% |
| Inspector complete ms | 12.523 | 14.321 | +14.415% |

| Pair | Side | Rotation | 122 median ms | 122 P95 ms | 128 CPU ms/turn | 128 KiB/turn |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | before | cold | 10.322 | 12.562 | 11.484 | 483.719 |
| 0 | before | warm | 9.501 | 11.743 | 11.328 | 502.844 |
| 0 | after | cold | 13.040 | 15.343 | 14.922 | 483.719 |
| 0 | after | warm | 10.243 | 16.379 | 11.641 | 503.031 |
| 1 | after | cold | 13.244 | 16.035 | 14.844 | 483.625 |
| 1 | after | warm | 9.840 | 12.956 | 11.250 | 502.844 |
| 1 | before | cold | 9.831 | 15.060 | 11.484 | 483.625 |
| 1 | before | warm | 9.712 | 12.634 | 11.406 | 502.969 |

| Pair | Side | Phase | Anon MiB | RSS MiB |
| --- | --- | --- | --- | --- |
| 0 | before | all_subscribed_empty | 324.156 | 379.574 |
| 0 | before | seeded_history_8 | 766.301 | 829.352 |
| 0 | before | cache_pages_idle | 764.371 | 827.543 |
| 0 | before | cache_statements_idle | 771.754 | 834.926 |
| 0 | before | cache_rotation_cold | 814.285 | 878.020 |
| 0 | before | cache_rotation_warm | 855.262 | 918.996 |
| 0 | before | all_unloaded | 918.734 | 979.281 |
| 0 | after | all_subscribed_empty | 324.309 | 380.133 |
| 0 | after | seeded_history_8 | 741.402 | 805.129 |
| 0 | after | cache_pages_idle | 728.836 | 792.684 |
| 0 | after | cache_statements_idle | 697.855 | 761.703 |
| 0 | after | cache_rotation_cold | 788.473 | 852.945 |
| 0 | after | cache_rotation_warm | 828.766 | 893.238 |
| 0 | after | all_unloaded | 890.930 | 952.215 |
| 1 | after | all_subscribed_empty | 324.363 | 380.223 |
| 1 | after | seeded_history_8 | 744.328 | 808.062 |
| 1 | after | cache_pages_idle | 730.109 | 793.965 |
| 1 | after | cache_statements_idle | 698.949 | 762.805 |
| 1 | after | cache_rotation_cold | 811.402 | 875.883 |
| 1 | after | cache_rotation_warm | 850.457 | 914.938 |
| 1 | after | all_unloaded | 905.770 | 967.062 |
| 1 | before | all_subscribed_empty | 323.730 | 379.688 |
| 1 | before | seeded_history_8 | 767.582 | 831.133 |
| 1 | before | cache_pages_idle | 765.652 | 829.324 |
| 1 | before | cache_statements_idle | 773.387 | 837.059 |
| 1 | before | cache_rotation_cold | 816.070 | 880.305 |
| 1 | before | cache_rotation_warm | 856.918 | 921.152 |
| 1 | before | all_unloaded | 928.309 | 989.230 |

| Pair | Side | Kept20 median ms | Mixed98s CPU ms | Inspector ms | Warm max gap s | Warm gaps≥30s |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | before | 20.491 | 700.000 | 12.683 | 1.360 | 0 |
| 0 | after | 17.077 | 630.000 | 13.869 | 1.695 | 0 |
| 1 | after | 20.600 | 650.000 | 14.773 | 1.730 | 0 |
| 1 | before | 17.551 | 660.000 | 12.364 | 1.376 | 0 |

Raw `cache-rotation-{cold,warm}.json` and `cache-audit.json` retain every session time, actual idle gap and full counter boundary. No request or product was removed as an outlier.

## Trial before allocator return: separate products

These measurements belong to f2e1273d2 and are not pooled with the final d50125842 products. They showed that cache objects were freed without a corresponding ordinary-glibc PSS drop.

| Trial metric | Before median | Trial median | Paired change |
| --- | --- | --- | --- |
| seed8 anon MiB | 765.652 | 742.367 | -3.046% |
| width1 settled_ms | 10.219 | 9.561 | -6.635% |
| width1 accepted_ms | 1.001 | 0.966 | -2.437% |
| width1 p95_ms | 13.137 | 13.147 | -2.368% |
| width1 p99_ms | 13.137 | 13.147 | -2.368% |
| width1 cpu_ms_per_turn | 11.875 | 11.562 | -2.632% |
| width1 write_kib_per_turn | 641.750 | 641.500 | -0.039% |
| width4 settled_ms | 28.923 | 27.891 | -2.832% |
| width4 accepted_ms | 5.893 | 4.486 | -17.121% |
| width4 p95_ms | 37.672 | 32.852 | -11.674% |
| width4 p99_ms | 38.338 | 37.045 | -7.519% |
| width4 cpu_ms_per_turn | 11.641 | 11.406 | -3.205% |
| width4 write_kib_per_turn | 667.281 | 667.312 | +0.005% |
| width8 settled_ms | 112.147 | 54.728 | -33.582% |
| width8 accepted_ms | 23.772 | 10.819 | -45.921% |
| width8 p95_ms | 394.252 | 64.792 | -53.378% |
| width8 p99_ms | 564.483 | 71.104 | -51.105% |
| width8 cpu_ms_per_turn | 12.969 | 11.602 | -9.079% |
| width8 write_kib_per_turn | 693.125 | 693.688 | +0.095% |

## Heap diagnostic: one product per side, replacement tcmalloc

Same preserved O3/default native ELF, complete 64KiB sampled stacks. Allocator numeric properties are allocator counters; weighted requested bytes are a sampling estimate. Neither equals ordinary-glibc PSS. Stack-presence groups overlap and must not be added. This diagnostic uses the trial source, which already contains the complete cache/ownership changes; final only adds the allocator return hint.

| Side | Phase | Requested estimate MiB | Allocator allocated MiB | Heap MiB | Page free MiB | Page unmapped MiB |
| --- | --- | --- | --- | --- | --- | --- |
| before | all_subscribed_empty | 298.305 | 329.040 | 358.969 | 8.047 | 0.000 |
| before | seeded_history_8 | 734.413 | 799.946 | 863.820 | 15.781 | 0.000 |
| before | cache_pages_idle | 734.476 | 799.950 | 863.820 | 21.539 | 0.000 |
| before | cache_statements_idle | 742.219 | 808.070 | 870.203 | 21.438 | 0.000 |
| before | all_unloaded | 88.511 | 96.549 | 1,020.203 | 756.031 | 0.000 |
| after | all_subscribed_empty | 287.788 | 327.881 | 357.922 | 8.484 | 0.000 |
| after | seeded_history_8 | 697.884 | 776.163 | 840.648 | 15.555 | 0.000 |
| after | cache_pages_idle | 591.198 | 664.336 | 840.648 | 122.852 | 0.000 |
| after | cache_statements_idle | 495.510 | 558.681 | 840.648 | 170.484 | 0.000 |
| after | all_unloaded | 85.385 | 96.139 | 926.648 | 646.203 | 0.000 |

| Side | Phase | SQLite malloc MiB | Page-cache stack MiB | Prepare stack MiB | Schema stack MiB | Checkpoint stack MiB | Publish stack MiB |
| --- | --- | --- | --- | --- | --- | --- | --- |
| before | all_subscribed_empty | 72.434 | 22.519 | 55.449 | 66.911 | 15.494 | 17.713 |
| before | seeded_history_8 | 230.897 | 113.363 | 140.495 | 66.911 | 119.845 | 125.383 |
| before | cache_pages_idle | 230.897 | 113.363 | 140.495 | 66.911 | 119.845 | 125.383 |
| before | cache_statements_idle | 232.106 | 114.571 | 140.495 | 66.911 | 123.439 | 129.041 |
| before | all_unloaded | 0.000 | 0.000 | 0.000 | 66.911 | 0.063 | 2.028 |
| after | all_subscribed_empty | 68.592 | 20.864 | 54.901 | 63.666 | 15.180 | 17.423 |
| after | seeded_history_8 | 229.523 | 118.133 | 138.157 | 63.666 | 114.090 | 120.797 |
| after | cache_pages_idle | 122.838 | 11.448 | 129.632 | 63.666 | 114.090 | 120.797 |
| after | cache_statements_idle | 50.358 | 12.403 | 33.629 | 63.666 | 117.170 | 124.003 |
| after | all_unloaded | 0.000 | 0.000 | 0.000 | 63.666 | 0.063 | 2.115 |

## Controlled ordinary-glibc allocator return

One diagnostic product per side, one synchronous malloc_trim(0) at the 130s cut. The preload adds only a control thread; it does not replace glibc. Both native products still pass the full 1,515-request/native/reopen/cancel checks. This isolates a causal immediate PSS drop, not a statistical latency claim.

| Source | Before anon MiB | After anon MiB | Returned MiB | Control wall ms | Control thread CPU ms |
| --- | --- | --- | --- | --- | --- |
| before | 773.410 | 771.539 | 1.871 | 0.344 | 0.343 |
| after | 743.605 | 697.824 | 45.781 | 12.766 | 12.707 |

## Verification and reference boundaries

| Record | Value |
| --- | --- |
| Final unique tests | {'ok': 3256, 'ignored': 3} |
| Final source/test/Cargo inputs | 973 |
| Final command groups | 17 |
| Primary ordinary products | 8 |
| Extended ordinary products | 4 |
| Final stream/large/tool/cancel products | 8 |
| Trial native products including heap/control diagnostics | 16 |
| Heap profiles independently rechecked | 10 |
| New Pi/DeepSeek products | 0 |

Prior authorized Pi single memory sample only: anonymous PSS 310.465MiB, RSS 373.746MiB. Final immediate rustX seed8 / that reference = 2.393× anonymous PSS, 2.157× RSS. Different feature surfaces remain; no new Pi performance comparison. Idle rustX endpoints have no matched prior Pi idle endpoint.
