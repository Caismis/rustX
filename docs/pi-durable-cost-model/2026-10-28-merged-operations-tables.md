# Merged operations performance evidence

Fresh Rust comparison: five interleaved pairs per cell. Historical Pi/DeepSeek rows are fixed, separate batches; their ratios are ratios of medians, not paired results.

| Mode | Case | Paired median wall % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -36.944 [-45.108,24.770] | -15.385 | -0.019 | -0.970 | 0.152 |
| no-live-subscriber | short-large | 7.351 [-37.238,60.784] | -0.488 | 0.018 | -0.284 | 0.402 |
| no-live-subscriber | stream | -0.552 [-1.645,0.374] | -3.704 | 0.000 | -0.370 | 0.565 |
| no-live-subscriber | large | 0.054 [-2.040,3.743] | -6.250 | 0.000 | -0.142 | 1.281 |
| no-live-subscriber | tool | 8.527 [4.421,23.003] | 6.250 | 0.000 | -0.360 | 0.764 |
| no-live-subscriber | cancel | 10.215 [-1.284,28.728] | 0.000 | 0.000 | 0.736 | 1.275 |
| live-subscriber | short-small | -16.261 [-49.232,13.739] | -13.587 | 0.019 | -0.034 | 0.946 |
| live-subscriber | short-large | -13.333 [-48.908,56.384] | -15.091 | 0.025 | 0.300 | 0.814 |
| live-subscriber | stream | -0.961 [-2.451,1.723] | 0.000 | 0.000 | 0.370 | 1.374 |
| live-subscriber | large | -12.252 [-27.250,2.436] | 0.000 | 0.000 | 0.248 | 1.028 |
| live-subscriber | tool | -3.785 [-10.159,46.354] | 5.000 | 0.000 | 0.472 | 1.315 |
| live-subscriber | cancel | -4.774 [-27.722,32.586] | 0.000 | 0.000 | -0.281 | 0.905 |

| Mode | Case | Mean wall / old Pi | Median wall / old Pi | Full CPU / old Pi | Full writes / old Pi |
|---|---|---:|---:|---:|---:|
| no-live-subscriber | short-small | 3.275 | 3.363 | 1.170 | 4.744 |
| no-live-subscriber | short-large | 4.719 | 5.366 | 2.759 | 5.228 |
| no-live-subscriber | stream | 0.908 | 0.908 | 0.236 | 4.009 |
| no-live-subscriber | large | 0.428 | 0.428 | 0.254 | 4.541 |
| no-live-subscriber | tool | 0.456 | 0.456 | 0.286 | 4.171 |
| no-live-subscriber | cancel | 0.341 | 0.341 | 0.230 | 4.767 |
| live-subscriber | short-small | 4.277 | 5.187 | 1.639 | 4.744 |
| live-subscriber | short-large | 4.652 | 5.477 | 2.813 | 5.230 |
| live-subscriber | stream | 0.917 | 0.917 | 0.261 | 4.009 |
| live-subscriber | large | 0.390 | 0.390 | 0.271 | 4.541 |
| live-subscriber | tool | 0.516 | 0.516 | 0.333 | 4.171 |
| live-subscriber | cancel | 0.373 | 0.373 | 0.283 | 4.767 |

Warm measured windows (per-product statistic, then median [minimum,maximum] across five Rust/Pi products; each has 32 measured turns):

| Mode | History | Engine/batch | Mean ms/turn | Median ms/turn | CPU ms/turn | Write bytes/turn |
|---|---:|---|---:|---:|---:|---:|
| no-live-subscriber | 8 | current Rust | 11.875 [10.087,14.526] | 10.543 [9.413,15.373] | 14.062 [11.562,16.250] | 1084928.000 [1084544.000,1085184.000] |
| no-live-subscriber | 8 | fixed old Pi | 3.625 [3.524,4.546] | 3.135 [3.014,3.328] | 7.188 [6.875,9.688] | 224512.000 [224512.000,224512.000] |
| no-live-subscriber | 64 | current Rust | 17.444 [13.855,18.704] | 17.701 [11.642,18.439] | 19.688 [15.625,20.938] | 1242752.000 [1242240.000,1243520.000] |
| no-live-subscriber | 64 | fixed old Pi | 3.697 [3.667,3.911] | 3.299 [3.159,3.524] | 6.250 [5.938,6.875] | 224384.000 [224384.000,224384.000] |
| live-subscriber | 8 | current Rust | 16.825 [10.867,21.438] | 17.741 [10.162,20.064] | 24.688 [16.875,31.875] | 1085056.000 [1084928.000,1085312.000] |
| live-subscriber | 8 | fixed old Pi | 3.934 [3.784,4.165] | 3.420 [3.309,3.699] | 7.500 [7.188,8.438] | 224512.000 [224512.000,224512.000] |
| live-subscriber | 64 | current Rust | 18.558 [13.957,20.889] | 19.393 [11.729,19.882] | 26.875 [20.625,31.250] | 1242880.000 [1241984.000,1243392.000] |
| live-subscriber | 64 | fixed old Pi | 3.989 [3.827,4.233] | 3.541 [3.482,3.679] | 6.875 [6.250,7.188] | 224384.000 [224384.000,224384.000] |

| Live case | Current mean ms/turn | Fixed DS mean ms/turn | Wall ratio | CPU ratio | Measured write ratio |
|---|---:|---:|---:|---:|---:|
| short-small | 16.825 | 15.541 | 1.083 | 1.362 | 50.458 |
| short-large | 18.558 | 15.450 | 1.201 | 1.536 | 58.144 |
| stream | 1591.861 | 1583.218 | 1.005 | 0.500 | 50.714 |
| large | 35.091 | 165.613 | 0.212 | 0.200 | 35.143 |

DeepSeek exit/flush CPU remains separate from measured turns and from Rust full-lifecycle CPU; full DeepSeek process write bytes remain unavailable. The JSON retains the exact wait4 metrics and source-specific limits.

All adverse metrics (including individual fresh paired reversals):

| Comparison | Mode | Case | Metric | Median % or cross-batch ratio | Adverse pairs |
|---|---|---|---|---:|---|
| fresh_paired_rust | no-live-subscriber | short-small | mean_turn_ms | -30.048 | 2:+10.148% |
| fresh_paired_rust | no-live-subscriber | short-small | turn_cpu_ms_per_turn | -26.230 | 2:+8.333% |
| fresh_paired_rust | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | -15.385 | 2:+6.897% |
| fresh_paired_rust | no-live-subscriber | short-small | through_cleanup_process_write_mib | -0.019 | 3:+0.028% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_ms | -14.143 | 2:+6.271% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_anon_pss_mib | -0.970 | 4:+0.042% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_pss_mib | 0.174 | 1:+0.174%,2:+0.502%,4:+0.776% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_rss_mib | 0.152 | 1:+0.152%,2:+0.501%,4:+0.762% |
| fresh_paired_rust | no-live-subscriber | short-small | median_measured_turn_ms | -36.944 | 2:+24.770% |
| fresh_paired_rust | no-live-subscriber | short-small | measured_turn_write_bytes | -0.024 | 3:+0.035% |
| fresh_paired_rust | no-live-subscriber | short-small | turn_write_bytes_per_turn | -0.024 | 3:+0.035% |
| fresh_paired_rust | no-live-subscriber | short-small | through_cleanup_process_write_bytes | -0.019 | 3:+0.028% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_self_cpu_ms | -15.385 | 2:+6.897% |
| fresh_paired_rust | no-live-subscriber | short-small | measured_turn_cpu_ms | -26.230 | 2:+8.333% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_turn_rss_mib | -0.376 | 4:+0.415% |
| fresh_paired_rust | no-live-subscriber | short-small | through_product_exit_wall_ms | -13.254 | 2:+5.603% |
| fresh_paired_rust | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -12.640 | 2:+4.000% |
| fresh_paired_rust | no-live-subscriber | short-small | closed_logical_bytes | -0.055 | 1:+0.055% |
| fresh_paired_rust | no-live-subscriber | short-small | closed_allocated_bytes | -0.054 | 1:+0.054% |
| fresh_paired_rust | no-live-subscriber | short-large | mean_turn_ms | 13.271 | 0:+40.643%,2:+13.271%,3:+15.399%,4:+10.700% |
| fresh_paired_rust | no-live-subscriber | short-large | turn_cpu_ms_per_turn | 12.727 | 0:+43.182%,2:+12.727%,3:+17.544%,4:+8.333% |
| fresh_paired_rust | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | -0.488 | 0:+8.357%,2:+4.156% |
| fresh_paired_rust | no-live-subscriber | short-large | through_cleanup_process_write_mib | 0.018 | 0:+0.018%,1:+0.043%,3:+0.004%,4:+0.036% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_ms | -0.691 | 0:+7.128%,2:+4.026% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_anon_pss_mib | -0.284 | 3:+0.299% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_pss_mib | 0.406 | 0:+0.406%,3:+0.749%,4:+0.608% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_rss_mib | 0.402 | 0:+0.402%,3:+0.777%,4:+0.601% |
| fresh_paired_rust | no-live-subscriber | short-large | median_measured_turn_ms | 7.351 | 0:+60.784%,2:+12.608%,3:+7.351%,4:+1.047% |
| fresh_paired_rust | no-live-subscriber | short-large | measured_turn_write_bytes | 0.072 | 0:+0.093%,1:+0.124%,2:+0.062%,3:+0.021%,4:+0.072% |
| fresh_paired_rust | no-live-subscriber | short-large | turn_write_bytes_per_turn | 0.072 | 0:+0.093%,1:+0.124%,2:+0.062%,3:+0.021%,4:+0.072% |
| fresh_paired_rust | no-live-subscriber | short-large | through_cleanup_process_write_bytes | 0.018 | 0:+0.018%,1:+0.043%,3:+0.004%,4:+0.036% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_self_cpu_ms | -0.488 | 0:+8.357%,2:+4.156% |
| fresh_paired_rust | no-live-subscriber | short-large | measured_turn_cpu_ms | 12.727 | 0:+43.182%,2:+12.727%,3:+17.544%,4:+8.333% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_turn_rss_mib | 0.398 | 0:+0.398%,3:+1.073%,4:+0.713% |
| fresh_paired_rust | no-live-subscriber | short-large | through_product_exit_wall_ms | -0.778 | 0:+6.696%,2:+3.943% |
| fresh_paired_rust | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | -1.204 | 0:+6.498%,2:+3.879% |
| fresh_paired_rust | no-live-subscriber | short-large | closed_logical_bytes | 0.114 | 1:+0.114%,2:+1.757%,3:+1.103% |
| fresh_paired_rust | no-live-subscriber | short-large | closed_allocated_bytes | 0.113 | 1:+0.113%,2:+1.746%,3:+1.057% |
| fresh_paired_rust | no-live-subscriber | stream | mean_turn_ms | -0.552 | 1:+0.374% |
| fresh_paired_rust | no-live-subscriber | stream | through_cleanup_self_cpu_ms | -3.704 | 2:+3.704% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_ms | -0.744 | 2:+0.980%,3:+0.122% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_anon_pss_mib | -0.370 | 4:+0.125% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_pss_mib | 0.609 | 0:+0.609%,1:+1.068%,3:+0.181%,4:+1.816% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_rss_mib | 0.565 | 0:+0.565%,1:+1.038%,3:+0.176%,4:+1.800% |
| fresh_paired_rust | no-live-subscriber | stream | median_measured_turn_ms | -0.552 | 1:+0.374% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_self_cpu_ms | -3.704 | 2:+3.704% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_turn_rss_mib | 0.442 | 0:+0.621%,1:+0.442%,2:+0.059%,3:+0.312%,4:+1.445% |
| fresh_paired_rust | no-live-subscriber | stream | through_product_exit_wall_ms | -0.411 | 2:+1.298% |
| fresh_paired_rust | no-live-subscriber | stream | complete_harness_lifecycle_wall_ms | -0.389 | 2:+1.221%,3:+0.310% |
| fresh_paired_rust | no-live-subscriber | large | mean_turn_ms | 0.054 | 0:+2.636%,1:+0.054%,3:+3.743% |
| fresh_paired_rust | no-live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 0:+33.333%,3:+25.000% |
| fresh_paired_rust | no-live-subscriber | large | through_cleanup_self_cpu_ms | -6.250 | 1:+6.250% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_ms | -1.628 | 2:+2.312% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_anon_pss_mib | -0.142 | 2:+0.719%,3:+0.467% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_pss_mib | 1.379 | 0:+0.167%,1:+1.060%,2:+1.379%,3:+1.402%,4:+1.421% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_rss_mib | 1.281 | 0:+0.191%,1:+1.111%,2:+1.281%,3:+1.336%,4:+1.405% |
| fresh_paired_rust | no-live-subscriber | large | median_measured_turn_ms | 0.054 | 0:+2.636%,1:+0.054%,3:+3.743% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_self_cpu_ms | -6.250 | 1:+6.250% |
| fresh_paired_rust | no-live-subscriber | large | measured_turn_cpu_ms | 0.000 | 0:+33.333%,3:+25.000% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_turn_rss_mib | 0.919 | 0:+0.149%,1:+0.919%,2:+0.875%,3:+1.332%,4:+1.539% |
| fresh_paired_rust | no-live-subscriber | large | through_product_exit_wall_ms | -2.781 | 2:+2.426% |
| fresh_paired_rust | no-live-subscriber | large | complete_harness_lifecycle_wall_ms | -1.132 | 2:+2.529%,3:+0.421% |
| fresh_paired_rust | no-live-subscriber | tool | mean_turn_ms | 8.527 | 0:+8.527%,1:+23.003%,2:+13.419%,3:+4.421%,4:+8.068% |
| fresh_paired_rust | no-live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 1:+66.667%,3:+20.000% |
| fresh_paired_rust | no-live-subscriber | tool | through_cleanup_self_cpu_ms | 6.250 | 0:+6.250%,1:+6.250%,2:+12.500% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_ms | 5.384 | 0:+2.233%,1:+7.279%,2:+7.500%,3:+2.535%,4:+5.384% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_anon_pss_mib | -0.360 | 1:+2.081%,2:+1.875% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_pss_mib | 0.866 | 0:+0.556%,1:+2.890%,2:+2.756%,3:+0.866% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_rss_mib | 0.764 | 0:+0.540%,1:+2.874%,2:+2.669%,3:+0.764% |
| fresh_paired_rust | no-live-subscriber | tool | median_measured_turn_ms | 8.527 | 0:+8.527%,1:+23.003%,2:+13.419%,3:+4.421%,4:+8.068% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_self_cpu_ms | 6.250 | 0:+6.250%,1:+6.250%,2:+12.500% |
| fresh_paired_rust | no-live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 1:+66.667%,3:+20.000% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_turn_rss_mib | 0.620 | 0:+0.620%,1:+2.473%,2:+2.462%,3:+0.506% |
| fresh_paired_rust | no-live-subscriber | tool | through_product_exit_wall_ms | 5.743 | 0:+1.691%,1:+7.022%,2:+7.418%,3:+0.911%,4:+5.743% |
| fresh_paired_rust | no-live-subscriber | tool | complete_harness_lifecycle_wall_ms | 5.063 | 0:+2.453%,1:+6.284%,2:+5.671%,3:+1.317%,4:+5.063% |
| fresh_paired_rust | no-live-subscriber | cancel | mean_turn_ms | 10.215 | 0:+15.961%,2:+28.728%,3:+10.215% |
| fresh_paired_rust | no-live-subscriber | cancel | turn_cpu_ms_per_turn | 0.000 | 2:+33.333% |
| fresh_paired_rust | no-live-subscriber | cancel | lifecycle_ms | -0.924 | 0:+8.249%,2:+1.969% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_anon_pss_mib | 0.736 | 0:+0.736%,1:+1.063%,2:+1.040%,3:+0.606%,4:+0.076% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_pss_mib | 1.295 | 0:+1.216%,1:+1.508%,2:+1.404%,3:+1.180%,4:+1.295% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_rss_mib | 1.275 | 0:+1.158%,1:+1.539%,2:+1.396%,3:+1.162%,4:+1.275% |
| fresh_paired_rust | no-live-subscriber | cancel | median_measured_turn_ms | 10.215 | 0:+15.961%,2:+28.728%,3:+10.215% |
| fresh_paired_rust | no-live-subscriber | cancel | measured_turn_cpu_ms | 0.000 | 2:+33.333% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_turn_rss_mib | 1.439 | 0:+1.168%,1:+1.624%,2:+1.523%,3:+1.439%,4:+1.090% |
| fresh_paired_rust | no-live-subscriber | cancel | through_product_exit_wall_ms | -2.083 | 0:+8.519%,2:+2.893% |
| fresh_paired_rust | no-live-subscriber | cancel | complete_harness_lifecycle_wall_ms | -2.443 | 0:+7.092%,2:+3.717% |
| fresh_paired_rust | live-subscriber | short-small | mean_turn_ms | -24.434 | 1:+2.096%,3:+21.614% |
| fresh_paired_rust | live-subscriber | short-small | turn_cpu_ms_per_turn | -22.549 | 1:+6.579%,3:+24.390% |
| fresh_paired_rust | live-subscriber | short-small | through_cleanup_self_cpu_ms | -13.587 | 1:+3.896%,3:+10.366% |
| fresh_paired_rust | live-subscriber | short-small | through_cleanup_process_write_mib | 0.019 | 0:+0.009%,1:+0.037%,2:+0.019%,3:+0.028%,4:+0.009% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_ms | -13.361 | 1:+0.955%,3:+6.066% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_anon_pss_mib | -0.034 | 0:+0.814%,3:+0.603% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_pss_mib | 0.942 | 0:+1.611%,1:+0.789%,3:+1.598%,4:+0.942% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_rss_mib | 0.946 | 0:+1.564%,1:+0.773%,3:+1.602%,4:+0.946% |
| fresh_paired_rust | live-subscriber | short-small | median_measured_turn_ms | -16.261 | 1:+8.677%,3:+13.739% |
| fresh_paired_rust | live-subscriber | short-small | measured_turn_write_bytes | 0.024 | 0:+0.012%,1:+0.047%,2:+0.024%,3:+0.035%,4:+0.012% |
| fresh_paired_rust | live-subscriber | short-small | turn_write_bytes_per_turn | 0.024 | 0:+0.012%,1:+0.047%,2:+0.024%,3:+0.035%,4:+0.012% |
| fresh_paired_rust | live-subscriber | short-small | through_cleanup_process_write_bytes | 0.019 | 0:+0.009%,1:+0.037%,2:+0.019%,3:+0.028%,4:+0.009% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_self_cpu_ms | -13.587 | 1:+3.896%,3:+10.366% |
| fresh_paired_rust | live-subscriber | short-small | measured_turn_cpu_ms | -22.549 | 1:+6.579%,3:+24.390% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_turn_rss_mib | 0.718 | 0:+0.759%,1:+0.718%,3:+1.248%,4:+0.628% |
| fresh_paired_rust | live-subscriber | short-small | through_product_exit_wall_ms | -13.043 | 1:+0.783%,3:+6.077% |
| fresh_paired_rust | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -12.573 | 1:+0.935%,3:+6.303% |
| fresh_paired_rust | live-subscriber | short-small | closed_logical_bytes | 0.055 | 0:+0.055%,2:+0.055%,3:+0.110% |
| fresh_paired_rust | live-subscriber | short-small | closed_allocated_bytes | 0.054 | 0:+0.054%,2:+0.054%,3:+0.109% |
| fresh_paired_rust | live-subscriber | short-large | mean_turn_ms | -20.627 | 1:+28.292%,3:+26.471% |
| fresh_paired_rust | live-subscriber | short-large | turn_cpu_ms_per_turn | -17.308 | 1:+33.333%,3:+30.556% |
| fresh_paired_rust | live-subscriber | short-large | through_cleanup_self_cpu_ms | -15.091 | 1:+21.040%,3:+15.566% |
| fresh_paired_rust | live-subscriber | short-large | through_cleanup_process_write_mib | 0.025 | 1:+0.058%,2:+0.054%,3:+0.025%,4:+0.014% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_ms | -14.881 | 1:+14.299%,3:+10.246% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_anon_pss_mib | 0.300 | 1:+0.300%,2:+0.453%,4:+0.490% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_pss_mib | 0.791 | 0:+0.791%,1:+0.860%,2:+0.769%,3:+0.260%,4:+0.796% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_rss_mib | 0.814 | 0:+0.814%,1:+0.865%,2:+0.825%,3:+0.264%,4:+0.775% |
| fresh_paired_rust | live-subscriber | short-large | median_measured_turn_ms | -13.333 | 1:+56.384%,3:+51.442% |
| fresh_paired_rust | live-subscriber | short-large | measured_turn_write_bytes | 0.031 | 0:+0.031%,3:+0.062%,4:+0.113% |
| fresh_paired_rust | live-subscriber | short-large | turn_write_bytes_per_turn | 0.031 | 0:+0.031%,3:+0.062%,4:+0.113% |
| fresh_paired_rust | live-subscriber | short-large | through_cleanup_process_write_bytes | 0.025 | 1:+0.058%,2:+0.054%,3:+0.025%,4:+0.014% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_self_cpu_ms | -15.091 | 1:+21.040%,3:+15.566% |
| fresh_paired_rust | live-subscriber | short-large | measured_turn_cpu_ms | -17.308 | 1:+33.333%,3:+30.556% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_turn_rss_mib | 0.759 | 0:+0.717%,1:+0.759%,2:+0.964%,3:+0.385%,4:+0.911% |
| fresh_paired_rust | live-subscriber | short-large | through_product_exit_wall_ms | -13.719 | 1:+13.761%,3:+10.246% |
| fresh_paired_rust | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | -13.298 | 1:+13.011%,3:+9.770% |
| fresh_paired_rust | live-subscriber | short-large | closed_logical_bytes | 0.113 | 0:+0.457%,2:+0.076%,3:+0.113%,4:+1.489% |
| fresh_paired_rust | live-subscriber | short-large | closed_allocated_bytes | 0.112 | 0:+0.494%,2:+0.076%,3:+0.112%,4:+1.479% |
| fresh_paired_rust | live-subscriber | stream | mean_turn_ms | -0.961 | 1:+0.554%,3:+1.723% |
| fresh_paired_rust | live-subscriber | stream | turn_cpu_ms_per_turn | -11.765 | 1:+7.143%,3:+21.429% |
| fresh_paired_rust | live-subscriber | stream | through_cleanup_self_cpu_ms | 0.000 | 1:+7.407%,3:+19.231% |
| fresh_paired_rust | live-subscriber | stream | through_cleanup_process_write_mib | 0.000 | 2:+0.227% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_ms | 1.354 | 1:+1.992%,2:+1.354%,3:+1.976% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_anon_pss_mib | 0.370 | 1:+0.370%,3:+0.428%,4:+2.011% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_pss_mib | 1.337 | 0:+0.221%,1:+1.337%,2:+0.875%,3:+1.464%,4:+1.911% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_rss_mib | 1.374 | 0:+0.226%,1:+1.374%,2:+0.868%,3:+1.436%,4:+1.919% |
| fresh_paired_rust | live-subscriber | stream | median_measured_turn_ms | -0.961 | 1:+0.554%,3:+1.723% |
| fresh_paired_rust | live-subscriber | stream | measured_turn_write_bytes | 0.000 | 2:+0.282% |
| fresh_paired_rust | live-subscriber | stream | turn_write_bytes_per_turn | 0.000 | 2:+0.282% |
| fresh_paired_rust | live-subscriber | stream | through_cleanup_process_write_bytes | 0.000 | 2:+0.227% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_self_cpu_ms | 0.000 | 1:+7.407%,3:+19.231% |
| fresh_paired_rust | live-subscriber | stream | measured_turn_cpu_ms | -11.765 | 1:+7.143%,3:+21.429% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_turn_rss_mib | 0.976 | 0:+0.017%,1:+1.003%,2:+0.942%,3:+0.976%,4:+1.772% |
| fresh_paired_rust | live-subscriber | stream | through_product_exit_wall_ms | 1.412 | 1:+1.949%,2:+1.412%,3:+1.598% |
| fresh_paired_rust | live-subscriber | stream | complete_harness_lifecycle_wall_ms | 1.436 | 1:+2.066%,2:+1.436%,3:+1.892% |
| fresh_paired_rust | live-subscriber | stream | closed_logical_bytes | 0.000 | 2:+0.189% |
| fresh_paired_rust | live-subscriber | stream | closed_allocated_bytes | 0.000 | 2:+0.187% |
| fresh_paired_rust | live-subscriber | large | mean_turn_ms | -12.252 | 0:+2.436% |
| fresh_paired_rust | live-subscriber | large | through_cleanup_self_cpu_ms | 0.000 | 0:+6.250% |
| fresh_paired_rust | live-subscriber | large | lifecycle_ms | -2.302 | 0:+4.984%,4:+7.040% |
| fresh_paired_rust | live-subscriber | large | max_sampled_anon_pss_mib | 0.248 | 0:+0.261%,1:+0.248%,3:+0.301% |
| fresh_paired_rust | live-subscriber | large | max_sampled_pss_mib | 1.038 | 0:+1.219%,1:+1.038%,3:+1.115%,4:+0.116% |
| fresh_paired_rust | live-subscriber | large | max_sampled_rss_mib | 1.028 | 0:+1.233%,1:+1.028%,3:+1.045%,4:+0.038% |
| fresh_paired_rust | live-subscriber | large | median_measured_turn_ms | -12.252 | 0:+2.436% |
| fresh_paired_rust | live-subscriber | large | lifecycle_self_cpu_ms | 0.000 | 0:+6.250% |
| fresh_paired_rust | live-subscriber | large | max_sampled_turn_rss_mib | 0.780 | 0:+0.993%,1:+0.780%,3:+0.917% |
| fresh_paired_rust | live-subscriber | large | through_product_exit_wall_ms | -2.294 | 0:+5.296%,4:+7.783% |
| fresh_paired_rust | live-subscriber | large | complete_harness_lifecycle_wall_ms | -0.800 | 0:+7.220%,4:+7.728% |
| fresh_paired_rust | live-subscriber | tool | mean_turn_ms | -3.785 | 0:+9.173%,2:+46.354% |
| fresh_paired_rust | live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 2:+40.000% |
| fresh_paired_rust | live-subscriber | tool | through_cleanup_self_cpu_ms | 5.000 | 1:+5.000%,2:+23.529%,3:+10.526% |
| fresh_paired_rust | live-subscriber | tool | lifecycle_ms | 6.146 | 0:+6.146%,1:+3.364%,2:+10.476%,3:+7.933% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_anon_pss_mib | 0.472 | 0:+0.472%,2:+0.848%,4:+1.501% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_pss_mib | 1.386 | 0:+1.386%,1:+0.240%,2:+1.715%,3:+0.609%,4:+1.722% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_rss_mib | 1.315 | 0:+1.315%,1:+0.277%,2:+1.634%,3:+0.651%,4:+1.685% |
| fresh_paired_rust | live-subscriber | tool | median_measured_turn_ms | -3.785 | 0:+9.173%,2:+46.354% |
| fresh_paired_rust | live-subscriber | tool | lifecycle_self_cpu_ms | 5.000 | 1:+5.000%,2:+23.529%,3:+10.526% |
| fresh_paired_rust | live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 2:+40.000% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_turn_rss_mib | 1.167 | 0:+1.167%,2:+1.518%,3:+0.190%,4:+1.500% |
| fresh_paired_rust | live-subscriber | tool | through_product_exit_wall_ms | 5.038 | 0:+5.038%,1:+0.871%,2:+9.376%,3:+6.012% |
| fresh_paired_rust | live-subscriber | tool | complete_harness_lifecycle_wall_ms | 2.214 | 0:+2.214%,2:+10.160%,3:+5.079% |
| fresh_paired_rust | live-subscriber | cancel | mean_turn_ms | -4.774 | 1:+32.586%,4:+32.469% |
| fresh_paired_rust | live-subscriber | cancel | turn_cpu_ms_per_turn | 25.000 | 1:+25.000%,2:+25.000%,4:+66.667% |
| fresh_paired_rust | live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 1:+13.333%,4:+6.250% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_ms | -7.193 | 1:+9.603%,4:+0.309% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_anon_pss_mib | -0.281 | 3:+1.235%,4:+1.832% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_pss_mib | 0.933 | 0:+0.083%,1:+0.933%,2:+0.730%,3:+1.567%,4:+2.162% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_rss_mib | 0.905 | 0:+0.049%,1:+0.905%,2:+0.708%,3:+1.535%,4:+2.143% |
| fresh_paired_rust | live-subscriber | cancel | median_measured_turn_ms | -4.774 | 1:+32.586%,4:+32.469% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 1:+13.333%,4:+6.250% |
| fresh_paired_rust | live-subscriber | cancel | measured_turn_cpu_ms | 25.000 | 1:+25.000%,2:+25.000%,4:+66.667% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_turn_rss_mib | 0.901 | 1:+0.901%,2:+0.487%,3:+1.277%,4:+2.106% |
| fresh_paired_rust | live-subscriber | cancel | through_product_exit_wall_ms | -6.147 | 1:+7.171%,4:+0.945% |
| fresh_paired_rust | live-subscriber | cancel | complete_harness_lifecycle_wall_ms | -4.332 | 1:+6.414% |
| prior_pi | no-live-subscriber | short-small | closed_allocated_bytes | 43.714 | cross-batch |
| prior_pi | no-live-subscriber | short-small | closed_logical_bytes | 43.621 | cross-batch |
| prior_pi | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 1.699 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_ms | 2.571 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_self_cpu_ms | 2.157 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_anon_pss_mib | 1.112 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_pss_mib | 1.459 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_rss_mib | 1.066 | cross-batch |
| prior_pi | no-live-subscriber | short-small | mean_turn_ms | 3.275 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_cpu_ms | 1.957 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_write_bytes | 4.832 | cross-batch |
| prior_pi | no-live-subscriber | short-small | median_measured_turn_ms | 3.363 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_bytes | 4.744 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_mib | 4.744 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.170 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_product_exit_wall_ms | 1.692 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_cpu_ms_per_turn | 1.957 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_write_bytes_per_turn | 4.832 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_allocated_bytes | 44.200 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_logical_bytes | 44.119 | cross-batch |
| prior_pi | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.498 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_ms | 4.663 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_self_cpu_ms | 3.929 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_anon_pss_mib | 1.270 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_pss_mib | 1.504 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_rss_mib | 1.192 | cross-batch |
| prior_pi | no-live-subscriber | short-large | mean_turn_ms | 4.719 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_cpu_ms | 3.150 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_write_bytes | 5.539 | cross-batch |
| prior_pi | no-live-subscriber | short-large | median_measured_turn_ms | 5.366 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_bytes | 5.228 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_mib | 5.228 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 2.759 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_product_exit_wall_ms | 3.594 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_cpu_ms_per_turn | 3.150 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_write_bytes_per_turn | 5.539 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_allocated_bytes | 16.750 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_logical_bytes | 16.624 | cross-batch |
| prior_pi | no-live-subscriber | stream | max_sampled_pss_mib | 1.228 | cross-batch |
| prior_pi | no-live-subscriber | stream | measured_turn_write_bytes | 2.817 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_bytes | 4.009 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_mib | 4.009 | cross-batch |
| prior_pi | no-live-subscriber | stream | turn_write_bytes_per_turn | 2.817 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_allocated_bytes | 11.237 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_logical_bytes | 11.140 | cross-batch |
| prior_pi | no-live-subscriber | large | lifecycle_ms | 1.217 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_anon_pss_mib | 1.073 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_pss_mib | 1.529 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_rss_mib | 1.034 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_turn_rss_mib | 1.031 | cross-batch |
| prior_pi | no-live-subscriber | large | measured_turn_write_bytes | 3.417 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_bytes | 4.541 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_mib | 4.541 | cross-batch |
| prior_pi | no-live-subscriber | large | turn_write_bytes_per_turn | 3.417 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_allocated_bytes | 19.844 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_logical_bytes | 19.736 | cross-batch |
| prior_pi | no-live-subscriber | tool | lifecycle_ms | 1.224 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_pss_mib | 1.358 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_turn_rss_mib | 1.043 | cross-batch |
| prior_pi | no-live-subscriber | tool | measured_turn_write_bytes | 3.110 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_bytes | 4.171 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_mib | 4.171 | cross-batch |
| prior_pi | no-live-subscriber | tool | turn_write_bytes_per_turn | 3.110 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_allocated_bytes | 13.900 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_logical_bytes | 13.776 | cross-batch |
| prior_pi | no-live-subscriber | cancel | lifecycle_ms | 1.131 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_pss_mib | 1.380 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_turn_rss_mib | 1.026 | cross-batch |
| prior_pi | no-live-subscriber | cancel | measured_turn_write_bytes | 3.522 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_bytes | 4.767 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_mib | 4.767 | cross-batch |
| prior_pi | no-live-subscriber | cancel | turn_write_bytes_per_turn | 3.522 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_allocated_bytes | 43.738 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_logical_bytes | 43.645 | cross-batch |
| prior_pi | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 1.977 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_ms | 3.017 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_self_cpu_ms | 2.944 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_anon_pss_mib | 1.190 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_pss_mib | 1.536 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_rss_mib | 1.116 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_turn_rss_mib | 1.001 | cross-batch |
| prior_pi | live-subscriber | short-small | mean_turn_ms | 4.277 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_cpu_ms | 3.292 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_write_bytes | 4.833 | cross-batch |
| prior_pi | live-subscriber | short-small | median_measured_turn_ms | 5.187 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_bytes | 4.744 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_mib | 4.744 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.639 | cross-batch |
| prior_pi | live-subscriber | short-small | through_product_exit_wall_ms | 1.985 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_cpu_ms_per_turn | 3.292 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_write_bytes_per_turn | 4.833 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_allocated_bytes | 44.167 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_logical_bytes | 44.102 | cross-batch |
| prior_pi | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.280 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_ms | 4.296 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_self_cpu_ms | 3.981 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_anon_pss_mib | 1.277 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_pss_mib | 1.510 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_rss_mib | 1.201 | cross-batch |
| prior_pi | live-subscriber | short-large | mean_turn_ms | 4.652 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_cpu_ms | 3.909 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_write_bytes | 5.539 | cross-batch |
| prior_pi | live-subscriber | short-large | median_measured_turn_ms | 5.477 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_bytes | 5.230 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_mib | 5.230 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_self_cpu_ms | 2.813 | cross-batch |
| prior_pi | live-subscriber | short-large | through_product_exit_wall_ms | 3.350 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_cpu_ms_per_turn | 3.909 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_write_bytes_per_turn | 5.539 | cross-batch |
| prior_pi | live-subscriber | stream | closed_allocated_bytes | 16.750 | cross-batch |
| prior_pi | live-subscriber | stream | closed_logical_bytes | 16.624 | cross-batch |
| prior_pi | live-subscriber | stream | max_sampled_pss_mib | 1.245 | cross-batch |
| prior_pi | live-subscriber | stream | measured_turn_write_bytes | 2.817 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_bytes | 4.009 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_mib | 4.009 | cross-batch |
| prior_pi | live-subscriber | stream | turn_write_bytes_per_turn | 2.817 | cross-batch |
| prior_pi | live-subscriber | large | closed_allocated_bytes | 11.237 | cross-batch |
| prior_pi | live-subscriber | large | closed_logical_bytes | 11.140 | cross-batch |
| prior_pi | live-subscriber | large | lifecycle_ms | 1.210 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_anon_pss_mib | 1.083 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_pss_mib | 1.533 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_rss_mib | 1.036 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_turn_rss_mib | 1.035 | cross-batch |
| prior_pi | live-subscriber | large | measured_turn_write_bytes | 3.417 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_bytes | 4.541 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_mib | 4.541 | cross-batch |
| prior_pi | live-subscriber | large | turn_write_bytes_per_turn | 3.417 | cross-batch |
| prior_pi | live-subscriber | tool | closed_allocated_bytes | 19.844 | cross-batch |
| prior_pi | live-subscriber | tool | closed_logical_bytes | 19.736 | cross-batch |
| prior_pi | live-subscriber | tool | lifecycle_ms | 1.300 | cross-batch |
| prior_pi | live-subscriber | tool | lifecycle_self_cpu_ms | 1.105 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_pss_mib | 1.348 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_turn_rss_mib | 1.039 | cross-batch |
| prior_pi | live-subscriber | tool | measured_turn_write_bytes | 3.110 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_bytes | 4.171 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_mib | 4.171 | cross-batch |
| prior_pi | live-subscriber | tool | turn_write_bytes_per_turn | 3.110 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_allocated_bytes | 13.900 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_logical_bytes | 13.776 | cross-batch |
| prior_pi | live-subscriber | cancel | lifecycle_ms | 1.170 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_pss_mib | 1.372 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_turn_rss_mib | 1.023 | cross-batch |
| prior_pi | live-subscriber | cancel | measured_turn_write_bytes | 3.522 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_bytes | 4.767 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_mib | 4.767 | cross-batch |
| prior_pi | live-subscriber | cancel | turn_write_bytes_per_turn | 3.522 | cross-batch |
| prior_deepseek | live-subscriber | short-small | mean_turn_ms | 1.083 | cross-batch |
| prior_deepseek | live-subscriber | short-small | median_measured_turn_ms | 1.201 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_cpu_ms_per_turn | 1.362 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_write_bytes_per_turn | 50.458 | cross-batch |
| prior_deepseek | live-subscriber | short-large | mean_turn_ms | 1.201 | cross-batch |
| prior_deepseek | live-subscriber | short-large | median_measured_turn_ms | 1.265 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_cpu_ms_per_turn | 1.536 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_write_bytes_per_turn | 58.144 | cross-batch |
| prior_deepseek | live-subscriber | stream | mean_turn_ms | 1.005 | cross-batch |
| prior_deepseek | live-subscriber | stream | median_measured_turn_ms | 1.005 | cross-batch |
| prior_deepseek | live-subscriber | stream | turn_write_bytes_per_turn | 50.714 | cross-batch |
| prior_deepseek | live-subscriber | large | turn_write_bytes_per_turn | 35.143 | cross-batch |

- Only the fresh old/new Rust products are interleaved pairs. Pi/DeepSeek use fixed previous batches and separate descriptive ratios of medians; no causal parity or paired confidence claim.
- /proc CPU counters use 100 ticks/s; each cumulative endpoint has 10 ms granularity. This is not profiler sampling.
- A product measured-turn median and a product measured-turn mean are different estimands. Across products we report median and range of those statistics, not pooled turns.
- DeepSeek SDK idle does not await flush/fsync. Native wait4 CPU covers exit/flush and verification separately; full process write_bytes is unavailable, not zero. No equivalent lifecycle ratio is constructed.
- All recorded adverse pairs and all unavailable ratios are retained, including reversals hidden by a favorable group median.

Unavailable ratio records, all per-product values, and reference hashes are retained in report-data.json.
