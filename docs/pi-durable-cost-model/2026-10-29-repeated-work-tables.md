# Repeated work removal performance evidence

Fresh Rust comparison: five interleaved pairs per cell. Historical Pi/DeepSeek rows are fixed, separate batches; their ratios are ratios of medians, not paired results.

| Mode | Case | Paired median wall % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -1.172 [-43.017,16.122] | -0.952 | -2.165 | -0.170 | -0.178 |
| no-live-subscriber | short-large | 6.200 [-16.786,54.280] | -0.787 | -1.677 | -0.017 | -0.162 |
| no-live-subscriber | stream | 0.185 [-3.175,2.369] | -12.000 | -0.732 | -0.027 | -0.219 |
| no-live-subscriber | large | -4.565 [-8.001,3.896] | -6.250 | -0.831 | 0.137 | 0.048 |
| no-live-subscriber | tool | 1.695 [-20.546,4.987] | 0.000 | -1.449 | -0.610 | -0.847 |
| no-live-subscriber | cancel | 1.413 [-0.878,45.368] | 0.000 | -0.857 | 0.062 | -0.456 |
| live-subscriber | short-small | -1.813 [-40.647,37.785] | 1.987 | -2.166 | 0.164 | -0.127 |
| live-subscriber | short-large | -12.690 [-43.427,96.579] | -3.310 | -1.734 | 0.587 | 0.439 |
| live-subscriber | stream | -1.690 [-2.389,1.444] | -6.897 | -0.732 | -0.062 | -0.282 |
| live-subscriber | large | 19.800 [-16.561,49.090] | 12.500 | -0.831 | -0.018 | -0.276 |
| live-subscriber | tool | 7.879 [-17.518,42.550] | 0.000 | -1.449 | 0.831 | 0.343 |
| live-subscriber | cancel | -3.208 [-5.896,12.742] | 0.000 | -0.857 | 0.119 | -0.075 |

| Mode | Case | Mean wall / old Pi | Median wall / old Pi | Full CPU / old Pi | Full writes / old Pi |
|---|---|---:|---:|---:|---:|
| no-live-subscriber | short-small | 2.789 | 2.796 | 1.106 | 4.056 |
| no-live-subscriber | short-large | 3.678 | 4.166 | 2.447 | 4.551 |
| no-live-subscriber | stream | 0.896 | 0.896 | 0.218 | 3.700 |
| no-live-subscriber | large | 0.381 | 0.381 | 0.254 | 4.163 |
| no-live-subscriber | tool | 0.418 | 0.418 | 0.254 | 3.683 |
| no-live-subscriber | cancel | 0.340 | 0.340 | 0.246 | 4.365 |
| live-subscriber | short-small | 4.479 | 4.774 | 1.577 | 4.056 |
| live-subscriber | short-large | 4.562 | 4.991 | 2.933 | 4.549 |
| live-subscriber | stream | 0.900 | 0.900 | 0.243 | 3.700 |
| live-subscriber | large | 0.476 | 0.476 | 0.305 | 4.163 |
| live-subscriber | tool | 0.445 | 0.445 | 0.286 | 3.683 |
| live-subscriber | cancel | 0.346 | 0.346 | 0.267 | 4.365 |

Warm measured windows (per-product statistic, then median [minimum,maximum] across five Rust/Pi products; each has 32 measured turns):

| Mode | History | Engine/batch | Mean ms/turn | Median ms/turn | CPU ms/turn | Write bytes/turn |
|---|---:|---|---:|---:|---:|---:|
| no-live-subscriber | 8 | current Rust | 10.112 [8.894,40.576] | 8.767 [8.011,14.420] | 11.250 [10.312,16.875] | 923392.000 [923136.000,923776.000] |
| no-live-subscriber | 8 | fixed old Pi | 3.625 [3.524,4.546] | 3.135 [3.014,3.328] | 7.188 [6.875,9.688] | 224512.000 [224512.000,224512.000] |
| no-live-subscriber | 64 | current Rust | 13.596 [9.960,18.177] | 13.744 [9.418,17.581] | 15.938 [11.875,20.312] | 1109504.000 [1109248.000,1110656.000] |
| no-live-subscriber | 64 | fixed old Pi | 3.697 [3.667,3.911] | 3.299 [3.159,3.524] | 6.250 [5.938,6.875] | 224384.000 [224384.000,224384.000] |
| live-subscriber | 8 | current Rust | 17.617 [12.690,63.315] | 16.327 [9.854,17.540] | 24.375 [17.500,26.562] | 923392.000 [923008.000,923648.000] |
| live-subscriber | 8 | fixed old Pi | 3.934 [3.784,4.165] | 3.420 [3.309,3.699] | 7.500 [7.188,8.438] | 224512.000 [224512.000,224512.000] |
| live-subscriber | 64 | current Rust | 18.199 [10.766,73.331] | 17.672 [10.173,19.506] | 22.812 [15.000,26.875] | 1108480.000 [1074816.000,1110784.000] |
| live-subscriber | 64 | fixed old Pi | 3.989 [3.827,4.233] | 3.541 [3.482,3.679] | 6.875 [6.250,7.188] | 224384.000 [224384.000,224384.000] |

| Live case | Current mean ms/turn | Fixed DS mean ms/turn | Wall ratio | CPU ratio | Measured write ratio |
|---|---:|---:|---:|---:|---:|
| short-small | 17.617 | 15.541 | 1.134 | 1.345 | 42.940 |
| short-large | 18.199 | 15.450 | 1.178 | 1.304 | 51.856 |
| stream | 1562.024 | 1583.218 | 0.987 | 0.467 | 45.000 |
| large | 42.814 | 165.613 | 0.259 | 0.200 | 29.714 |

DeepSeek exit/flush CPU remains separate from measured turns and from Rust full-lifecycle CPU; full DeepSeek process write bytes remain unavailable. The JSON retains the exact wait4 metrics and source-specific limits.

All adverse metrics (including individual fresh paired reversals):

| Comparison | Mode | Case | Metric | Median % or cross-batch ratio | Adverse pairs |
|---|---|---|---|---:|---|
| fresh_paired_rust | no-live-subscriber | short-small | mean_turn_ms | 3.043 | 1:+17.721%,3:+12.186%,4:+3.043% |
| fresh_paired_rust | no-live-subscriber | short-small | turn_cpu_ms_per_turn | 0.000 | 1:+14.894%,3:+14.706% |
| fresh_paired_rust | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | -0.952 | 1:+9.402%,3:+7.071% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_ms | 0.246 | 1:+7.711%,3:+6.435%,4:+0.246% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_anon_pss_mib | -0.170 | 2:+0.557%,4:+0.750% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_pss_mib | -0.234 | 2:+0.591%,4:+0.372% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_rss_mib | -0.178 | 2:+0.584%,4:+0.383% |
| fresh_paired_rust | no-live-subscriber | short-small | median_measured_turn_ms | -1.172 | 1:+16.122%,3:+9.851% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_self_cpu_ms | -0.952 | 1:+9.402%,3:+7.071% |
| fresh_paired_rust | no-live-subscriber | short-small | measured_turn_cpu_ms | 0.000 | 1:+14.894%,3:+14.706% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_turn_rss_mib | -0.677 | 2:+0.310%,4:+0.225% |
| fresh_paired_rust | no-live-subscriber | short-small | through_product_exit_wall_ms | 0.317 | 1:+7.220%,3:+6.284%,4:+0.317% |
| fresh_paired_rust | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -0.217 | 1:+7.452%,3:+6.646% |
| fresh_paired_rust | no-live-subscriber | short-small | closed_logical_bytes | 0.704 | 1:+0.822%,2:+0.763%,3:+0.704% |
| fresh_paired_rust | no-live-subscriber | short-small | closed_allocated_bytes | 0.699 | 1:+0.815%,2:+0.757%,3:+0.699% |
| fresh_paired_rust | no-live-subscriber | short-large | mean_turn_ms | -15.859 | 4:+25.237% |
| fresh_paired_rust | no-live-subscriber | short-large | turn_cpu_ms_per_turn | 5.263 | 1:+44.444%,2:+5.263%,4:+18.605% |
| fresh_paired_rust | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | -0.787 | 3:+0.606%,4:+0.284% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_ms | -12.849 | 3:+0.326%,4:+0.509% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_anon_pss_mib | -0.017 | 2:+0.384%,3:+0.422% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_pss_mib | -0.160 | 2:+0.035%,3:+0.282% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_rss_mib | -0.162 | 2:+0.030%,3:+0.270% |
| fresh_paired_rust | no-live-subscriber | short-large | median_measured_turn_ms | 6.200 | 1:+54.280%,2:+6.200%,4:+42.996% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_self_cpu_ms | -0.787 | 3:+0.606%,4:+0.284% |
| fresh_paired_rust | no-live-subscriber | short-large | measured_turn_cpu_ms | 5.263 | 1:+44.444%,2:+5.263%,4:+18.605% |
| fresh_paired_rust | no-live-subscriber | short-large | through_product_exit_wall_ms | -12.289 | 3:+0.395%,4:+0.871% |
| fresh_paired_rust | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | -11.948 | 3:+0.398%,4:+0.756% |
| fresh_paired_rust | no-live-subscriber | stream | mean_turn_ms | 0.185 | 1:+2.369%,2:+0.185%,3:+1.688% |
| fresh_paired_rust | no-live-subscriber | stream | turn_cpu_ms_per_turn | -8.333 | 3:+30.000% |
| fresh_paired_rust | no-live-subscriber | stream | through_cleanup_self_cpu_ms | -12.000 | 3:+13.636% |
| fresh_paired_rust | no-live-subscriber | stream | through_cleanup_process_write_mib | -0.732 | 2:+0.122% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_ms | -0.893 | 1:+6.967%,3:+1.960% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_anon_pss_mib | -0.027 | 4:+0.022% |
| fresh_paired_rust | no-live-subscriber | stream | median_measured_turn_ms | 0.185 | 1:+2.369%,2:+0.185%,3:+1.688% |
| fresh_paired_rust | no-live-subscriber | stream | measured_turn_write_bytes | -0.943 | 2:+0.943% |
| fresh_paired_rust | no-live-subscriber | stream | turn_write_bytes_per_turn | -0.943 | 2:+0.943% |
| fresh_paired_rust | no-live-subscriber | stream | through_cleanup_process_write_bytes | -0.732 | 2:+0.122% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_self_cpu_ms | -12.000 | 3:+13.636% |
| fresh_paired_rust | no-live-subscriber | stream | measured_turn_cpu_ms | -8.333 | 3:+30.000% |
| fresh_paired_rust | no-live-subscriber | stream | through_product_exit_wall_ms | -0.918 | 1:+6.993%,3:+1.945% |
| fresh_paired_rust | no-live-subscriber | stream | complete_harness_lifecycle_wall_ms | -0.848 | 1:+6.743%,3:+1.940% |
| fresh_paired_rust | no-live-subscriber | large | mean_turn_ms | -4.565 | 1:+3.546%,2:+3.896% |
| fresh_paired_rust | no-live-subscriber | large | through_cleanup_self_cpu_ms | -6.250 | 1:+20.000%,3:+7.143% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_ms | 0.235 | 0:+0.235%,1:+19.055%,3:+3.351% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_anon_pss_mib | 0.137 | 1:+0.587%,2:+0.932%,3:+0.137% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_pss_mib | -0.048 | 1:+0.227%,2:+0.700% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_rss_mib | 0.048 | 1:+0.224%,2:+0.703%,3:+0.048% |
| fresh_paired_rust | no-live-subscriber | large | median_measured_turn_ms | -4.565 | 1:+3.546%,2:+3.896% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_self_cpu_ms | -6.250 | 1:+20.000%,3:+7.143% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_turn_rss_mib | -0.391 | 2:+0.386% |
| fresh_paired_rust | no-live-subscriber | large | through_product_exit_wall_ms | 0.007 | 0:+0.007%,1:+17.953%,3:+3.461% |
| fresh_paired_rust | no-live-subscriber | large | complete_harness_lifecycle_wall_ms | 1.489 | 0:+1.489%,1:+17.911%,3:+3.305% |
| fresh_paired_rust | no-live-subscriber | tool | mean_turn_ms | 1.695 | 1:+1.695%,2:+4.987%,3:+4.200% |
| fresh_paired_rust | no-live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 2:+66.667%,3:+25.000% |
| fresh_paired_rust | no-live-subscriber | tool | through_cleanup_self_cpu_ms | 0.000 | 1:+10.526% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_ms | 3.977 | 0:+4.601%,1:+17.618%,2:+1.613%,3:+3.977% |
| fresh_paired_rust | no-live-subscriber | tool | median_measured_turn_ms | 1.695 | 1:+1.695%,2:+4.987%,3:+4.200% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_self_cpu_ms | 0.000 | 1:+10.526% |
| fresh_paired_rust | no-live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 2:+66.667%,3:+25.000% |
| fresh_paired_rust | no-live-subscriber | tool | through_product_exit_wall_ms | 5.090 | 0:+5.090%,1:+16.030%,2:+13.217%,3:+2.417% |
| fresh_paired_rust | no-live-subscriber | tool | complete_harness_lifecycle_wall_ms | 5.960 | 0:+5.960%,1:+17.923%,2:+28.226%,3:+1.875% |
| fresh_paired_rust | no-live-subscriber | cancel | mean_turn_ms | 1.413 | 0:+45.368%,1:+18.230%,3:+0.698%,4:+1.413% |
| fresh_paired_rust | no-live-subscriber | cancel | turn_cpu_ms_per_turn | 0.000 | 0:+33.333%,1:+33.333% |
| fresh_paired_rust | no-live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 0:+28.571%,1:+12.500% |
| fresh_paired_rust | no-live-subscriber | cancel | lifecycle_ms | -0.703 | 0:+16.832% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_anon_pss_mib | 0.062 | 0:+0.987%,1:+0.744%,4:+0.062% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_pss_mib | -0.544 | 1:+0.190% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_rss_mib | -0.456 | 1:+0.131% |
| fresh_paired_rust | no-live-subscriber | cancel | median_measured_turn_ms | 1.413 | 0:+45.368%,1:+18.230%,3:+0.698%,4:+1.413% |
| fresh_paired_rust | no-live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 0:+28.571%,1:+12.500% |
| fresh_paired_rust | no-live-subscriber | cancel | measured_turn_cpu_ms | 0.000 | 0:+33.333%,1:+33.333% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_turn_rss_mib | -0.176 | 1:+0.027% |
| fresh_paired_rust | no-live-subscriber | cancel | through_product_exit_wall_ms | -2.266 | 0:+18.165% |
| fresh_paired_rust | no-live-subscriber | cancel | complete_harness_lifecycle_wall_ms | -3.430 | 0:+16.442% |
| fresh_paired_rust | live-subscriber | short-small | mean_turn_ms | 0.576 | 0:+14.396%,1:+0.292%,2:+0.576%,3:+24.053% |
| fresh_paired_rust | live-subscriber | short-small | turn_cpu_ms_per_turn | 0.000 | 0:+8.333%,3:+1.190% |
| fresh_paired_rust | live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.987 | 0:+8.527%,2:+1.987%,3:+10.458% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_ms | 1.019 | 0:+9.946%,1:+0.291%,2:+1.019%,3:+16.348% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_anon_pss_mib | 0.164 | 1:+0.164%,3:+0.864%,4:+0.183% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_pss_mib | -0.146 | 1:+0.349%,3:+0.429% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_rss_mib | -0.127 | 1:+0.343%,3:+0.409% |
| fresh_paired_rust | live-subscriber | short-small | median_measured_turn_ms | -1.813 | 0:+37.785%,1:+2.690% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_self_cpu_ms | 1.987 | 0:+8.527%,2:+1.987%,3:+10.458% |
| fresh_paired_rust | live-subscriber | short-small | measured_turn_cpu_ms | 0.000 | 0:+8.333%,3:+1.190% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_turn_rss_mib | -0.132 | 1:+0.389%,3:+0.077% |
| fresh_paired_rust | live-subscriber | short-small | through_product_exit_wall_ms | 1.386 | 0:+9.073%,2:+1.386%,3:+15.995% |
| fresh_paired_rust | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 2.526 | 0:+7.078%,1:+0.257%,2:+2.526%,3:+15.898% |
| fresh_paired_rust | live-subscriber | short-small | closed_logical_bytes | 0.763 | 0:+0.763%,1:+0.822%,2:+0.763%,3:+0.763%,4:+0.763% |
| fresh_paired_rust | live-subscriber | short-small | closed_allocated_bytes | 0.757 | 0:+0.757%,1:+0.815%,2:+0.757%,3:+0.757%,4:+0.757% |
| fresh_paired_rust | live-subscriber | short-large | mean_turn_ms | -6.637 | 2:+87.532%,3:+122.533% |
| fresh_paired_rust | live-subscriber | short-large | turn_cpu_ms_per_turn | -9.877 | 2:+82.979% |
| fresh_paired_rust | live-subscriber | short-large | through_cleanup_self_cpu_ms | -3.310 | 2:+13.514% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_ms | -2.581 | 2:+11.550%,3:+2.922% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_anon_pss_mib | 0.587 | 0:+0.252%,1:+0.587%,2:+0.825%,4:+0.686% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_pss_mib | 0.413 | 0:+0.197%,1:+0.413%,2:+0.505%,4:+0.514% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_rss_mib | 0.439 | 0:+0.223%,1:+0.439%,2:+0.506%,4:+0.537% |
| fresh_paired_rust | live-subscriber | short-large | median_measured_turn_ms | -12.690 | 2:+96.579%,4:+2.187% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_self_cpu_ms | -3.310 | 2:+13.514% |
| fresh_paired_rust | live-subscriber | short-large | measured_turn_cpu_ms | -9.877 | 2:+82.979% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_turn_rss_mib | 0.339 | 0:+0.233%,1:+0.339%,2:+0.520%,4:+0.789% |
| fresh_paired_rust | live-subscriber | short-large | through_product_exit_wall_ms | -2.356 | 2:+10.977%,3:+2.905% |
| fresh_paired_rust | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | -2.244 | 2:+10.678%,3:+2.670% |
| fresh_paired_rust | live-subscriber | stream | mean_turn_ms | -1.690 | 1:+0.106%,4:+1.444% |
| fresh_paired_rust | live-subscriber | stream | turn_cpu_ms_per_turn | -17.647 | 4:+7.692% |
| fresh_paired_rust | live-subscriber | stream | through_cleanup_self_cpu_ms | -6.897 | 1:+12.000%,4:+17.391% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_ms | -0.969 | 1:+0.680%,4:+0.871% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_anon_pss_mib | -0.062 | 0:+0.110%,3:+0.018% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_pss_mib | -0.245 | 3:+0.051% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_rss_mib | -0.282 | 3:+0.035% |
| fresh_paired_rust | live-subscriber | stream | median_measured_turn_ms | -1.690 | 1:+0.106%,4:+1.444% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_self_cpu_ms | -6.897 | 1:+12.000%,4:+17.391% |
| fresh_paired_rust | live-subscriber | stream | measured_turn_cpu_ms | -17.647 | 4:+7.692% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_turn_rss_mib | -0.218 | 2:+0.136% |
| fresh_paired_rust | live-subscriber | stream | through_product_exit_wall_ms | -1.232 | 1:+0.495%,4:+0.914% |
| fresh_paired_rust | live-subscriber | stream | complete_harness_lifecycle_wall_ms | -1.140 | 1:+0.960%,4:+0.964% |
| fresh_paired_rust | live-subscriber | large | mean_turn_ms | 19.800 | 0:+47.233%,3:+49.090%,4:+19.800% |
| fresh_paired_rust | live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 3:+66.667% |
| fresh_paired_rust | live-subscriber | large | through_cleanup_self_cpu_ms | 12.500 | 0:+12.500%,3:+20.000%,4:+20.000% |
| fresh_paired_rust | live-subscriber | large | lifecycle_ms | 3.232 | 0:+11.626%,3:+3.232%,4:+13.626% |
| fresh_paired_rust | live-subscriber | large | max_sampled_anon_pss_mib | -0.018 | 0:+0.369%,2:+0.149% |
| fresh_paired_rust | live-subscriber | large | max_sampled_pss_mib | -0.235 | 0:+0.136% |
| fresh_paired_rust | live-subscriber | large | max_sampled_rss_mib | -0.276 | 0:+0.066% |
| fresh_paired_rust | live-subscriber | large | median_measured_turn_ms | 19.800 | 0:+47.233%,3:+49.090%,4:+19.800% |
| fresh_paired_rust | live-subscriber | large | lifecycle_self_cpu_ms | 12.500 | 0:+12.500%,3:+20.000%,4:+20.000% |
| fresh_paired_rust | live-subscriber | large | measured_turn_cpu_ms | 0.000 | 3:+66.667% |
| fresh_paired_rust | live-subscriber | large | max_sampled_turn_rss_mib | -0.371 | 0:+0.174% |
| fresh_paired_rust | live-subscriber | large | through_product_exit_wall_ms | 0.761 | 0:+10.661%,3:+0.761%,4:+14.868% |
| fresh_paired_rust | live-subscriber | large | complete_harness_lifecycle_wall_ms | 1.225 | 0:+4.717%,3:+1.225%,4:+16.652% |
| fresh_paired_rust | live-subscriber | tool | mean_turn_ms | 7.879 | 1:+2.066%,2:+7.879%,3:+37.163%,4:+42.550% |
| fresh_paired_rust | live-subscriber | tool | turn_cpu_ms_per_turn | 20.000 | 2:+20.000%,3:+100.000%,4:+40.000% |
| fresh_paired_rust | live-subscriber | tool | through_cleanup_self_cpu_ms | 0.000 | 3:+17.647%,4:+16.667% |
| fresh_paired_rust | live-subscriber | tool | lifecycle_ms | 3.587 | 0:+2.764%,1:+3.587%,2:+1.049%,3:+6.837%,4:+7.447% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_anon_pss_mib | 0.831 | 0:+1.927%,1:+0.559%,2:+0.831%,4:+1.668% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_pss_mib | 0.418 | 0:+1.038%,1:+0.295%,2:+0.418%,4:+0.922% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_rss_mib | 0.343 | 0:+0.975%,1:+0.309%,2:+0.343%,4:+0.970% |
| fresh_paired_rust | live-subscriber | tool | median_measured_turn_ms | 7.879 | 1:+2.066%,2:+7.879%,3:+37.163%,4:+42.550% |
| fresh_paired_rust | live-subscriber | tool | lifecycle_self_cpu_ms | 0.000 | 3:+17.647%,4:+16.667% |
| fresh_paired_rust | live-subscriber | tool | measured_turn_cpu_ms | 20.000 | 2:+20.000%,3:+100.000%,4:+40.000% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_turn_rss_mib | 0.721 | 0:+0.721%,1:+0.320%,2:+0.790%,4:+1.025% |
| fresh_paired_rust | live-subscriber | tool | through_product_exit_wall_ms | 3.730 | 0:+3.730%,1:+3.676%,2:+0.745%,3:+5.851%,4:+8.862% |
| fresh_paired_rust | live-subscriber | tool | complete_harness_lifecycle_wall_ms | 2.860 | 0:+1.930%,1:+2.860%,2:+1.326%,3:+5.858%,4:+8.534% |
| fresh_paired_rust | live-subscriber | cancel | mean_turn_ms | -3.208 | 1:+12.742% |
| fresh_paired_rust | live-subscriber | cancel | turn_cpu_ms_per_turn | 25.000 | 1:+33.333%,2:+25.000%,4:+25.000% |
| fresh_paired_rust | live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 2:+25.000% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_ms | 2.546 | 0:+6.772%,2:+77.722%,4:+2.546% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_anon_pss_mib | 0.119 | 0:+0.760%,2:+0.195%,3:+0.119%,4:+0.057% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_pss_mib | -0.154 | 0:+0.462%,4:+0.162% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_rss_mib | -0.075 | 0:+0.447%,4:+0.219% |
| fresh_paired_rust | live-subscriber | cancel | median_measured_turn_ms | -3.208 | 1:+12.742% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 2:+25.000% |
| fresh_paired_rust | live-subscriber | cancel | measured_turn_cpu_ms | 25.000 | 1:+33.333%,2:+25.000%,4:+25.000% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_turn_rss_mib | -0.071 | 0:+0.216%,4:+0.109% |
| fresh_paired_rust | live-subscriber | cancel | through_product_exit_wall_ms | 3.564 | 0:+6.866%,2:+76.301%,4:+3.564% |
| fresh_paired_rust | live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 5.548 | 0:+6.343%,2:+70.984%,4:+5.548% |
| prior_pi | no-live-subscriber | short-small | closed_allocated_bytes | 41.167 | cross-batch |
| prior_pi | no-live-subscriber | short-small | closed_logical_bytes | 41.072 | cross-batch |
| prior_pi | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 1.668 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_ms | 2.506 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_self_cpu_ms | 2.039 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_anon_pss_mib | 1.129 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_pss_mib | 1.486 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_rss_mib | 1.085 | cross-batch |
| prior_pi | no-live-subscriber | short-small | mean_turn_ms | 2.789 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_cpu_ms | 1.565 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_write_bytes | 4.113 | cross-batch |
| prior_pi | no-live-subscriber | short-small | median_measured_turn_ms | 2.796 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_bytes | 4.056 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_mib | 4.056 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.106 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_product_exit_wall_ms | 1.657 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_cpu_ms_per_turn | 1.565 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_write_bytes_per_turn | 4.113 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_allocated_bytes | 41.133 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_logical_bytes | 41.069 | cross-batch |
| prior_pi | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.141 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_ms | 4.159 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_self_cpu_ms | 3.485 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_anon_pss_mib | 1.277 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_pss_mib | 1.520 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_rss_mib | 1.205 | cross-batch |
| prior_pi | no-live-subscriber | short-large | mean_turn_ms | 3.678 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_cpu_ms | 2.550 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_write_bytes | 4.945 | cross-batch |
| prior_pi | no-live-subscriber | short-large | median_measured_turn_ms | 4.166 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_bytes | 4.551 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_mib | 4.551 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 2.447 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_product_exit_wall_ms | 3.221 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_cpu_ms_per_turn | 2.550 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_write_bytes_per_turn | 4.945 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_allocated_bytes | 15.406 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_logical_bytes | 15.304 | cross-batch |
| prior_pi | no-live-subscriber | stream | max_sampled_pss_mib | 1.253 | cross-batch |
| prior_pi | no-live-subscriber | stream | measured_turn_write_bytes | 2.500 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_bytes | 3.700 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_mib | 3.700 | cross-batch |
| prior_pi | no-live-subscriber | stream | turn_write_bytes_per_turn | 2.500 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_allocated_bytes | 10.158 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_logical_bytes | 10.056 | cross-batch |
| prior_pi | no-live-subscriber | large | lifecycle_ms | 1.135 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_anon_pss_mib | 1.086 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_pss_mib | 1.556 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_rss_mib | 1.052 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_turn_rss_mib | 1.047 | cross-batch |
| prior_pi | no-live-subscriber | large | measured_turn_write_bytes | 2.889 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_bytes | 4.163 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_mib | 4.163 | cross-batch |
| prior_pi | no-live-subscriber | large | turn_write_bytes_per_turn | 2.889 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_allocated_bytes | 17.062 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_logical_bytes | 16.939 | cross-batch |
| prior_pi | no-live-subscriber | tool | lifecycle_ms | 1.109 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_pss_mib | 1.377 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_turn_rss_mib | 1.055 | cross-batch |
| prior_pi | no-live-subscriber | tool | measured_turn_write_bytes | 2.521 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_bytes | 3.683 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_mib | 3.683 | cross-batch |
| prior_pi | no-live-subscriber | tool | turn_write_bytes_per_turn | 2.521 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_allocated_bytes | 12.567 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_logical_bytes | 12.435 | cross-batch |
| prior_pi | no-live-subscriber | cancel | lifecycle_ms | 1.150 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_pss_mib | 1.400 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_turn_rss_mib | 1.039 | cross-batch |
| prior_pi | no-live-subscriber | cancel | measured_turn_write_bytes | 2.970 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_bytes | 4.365 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_mib | 4.365 | cross-batch |
| prior_pi | no-live-subscriber | cancel | turn_write_bytes_per_turn | 2.970 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_allocated_bytes | 41.190 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_logical_bytes | 41.096 | cross-batch |
| prior_pi | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 2.000 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_ms | 3.037 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_self_cpu_ms | 2.833 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_anon_pss_mib | 1.195 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_pss_mib | 1.557 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_rss_mib | 1.131 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_turn_rss_mib | 1.015 | cross-batch |
| prior_pi | live-subscriber | short-small | mean_turn_ms | 4.479 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_cpu_ms | 3.250 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_write_bytes | 4.113 | cross-batch |
| prior_pi | live-subscriber | short-small | median_measured_turn_ms | 4.774 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_bytes | 4.056 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_mib | 4.056 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.577 | cross-batch |
| prior_pi | live-subscriber | short-small | through_product_exit_wall_ms | 2.003 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_cpu_ms_per_turn | 3.250 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_write_bytes_per_turn | 4.113 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_allocated_bytes | 41.100 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_logical_bytes | 41.018 | cross-batch |
| prior_pi | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.485 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_ms | 4.597 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_self_cpu_ms | 4.151 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_anon_pss_mib | 1.284 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_pss_mib | 1.525 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_rss_mib | 1.212 | cross-batch |
| prior_pi | live-subscriber | short-large | mean_turn_ms | 4.562 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_cpu_ms | 3.318 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_write_bytes | 4.940 | cross-batch |
| prior_pi | live-subscriber | short-large | median_measured_turn_ms | 4.991 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_bytes | 4.549 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_mib | 4.549 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_self_cpu_ms | 2.933 | cross-batch |
| prior_pi | live-subscriber | short-large | through_product_exit_wall_ms | 3.569 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_cpu_ms_per_turn | 3.318 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_write_bytes_per_turn | 4.940 | cross-batch |
| prior_pi | live-subscriber | stream | closed_allocated_bytes | 15.406 | cross-batch |
| prior_pi | live-subscriber | stream | closed_logical_bytes | 15.304 | cross-batch |
| prior_pi | live-subscriber | stream | max_sampled_pss_mib | 1.263 | cross-batch |
| prior_pi | live-subscriber | stream | measured_turn_write_bytes | 2.500 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_bytes | 3.700 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_mib | 3.700 | cross-batch |
| prior_pi | live-subscriber | stream | turn_write_bytes_per_turn | 2.500 | cross-batch |
| prior_pi | live-subscriber | large | closed_allocated_bytes | 10.158 | cross-batch |
| prior_pi | live-subscriber | large | closed_logical_bytes | 10.056 | cross-batch |
| prior_pi | live-subscriber | large | lifecycle_ms | 1.218 | cross-batch |
| prior_pi | live-subscriber | large | lifecycle_self_cpu_ms | 1.125 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_anon_pss_mib | 1.090 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_pss_mib | 1.561 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_rss_mib | 1.054 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_turn_rss_mib | 1.053 | cross-batch |
| prior_pi | live-subscriber | large | measured_turn_write_bytes | 2.889 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_bytes | 4.163 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_mib | 4.163 | cross-batch |
| prior_pi | live-subscriber | large | turn_write_bytes_per_turn | 2.889 | cross-batch |
| prior_pi | live-subscriber | tool | closed_allocated_bytes | 17.062 | cross-batch |
| prior_pi | live-subscriber | tool | closed_logical_bytes | 16.939 | cross-batch |
| prior_pi | live-subscriber | tool | lifecycle_ms | 1.251 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_pss_mib | 1.377 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_turn_rss_mib | 1.060 | cross-batch |
| prior_pi | live-subscriber | tool | measured_turn_write_bytes | 2.521 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_bytes | 3.683 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_mib | 3.683 | cross-batch |
| prior_pi | live-subscriber | tool | turn_write_bytes_per_turn | 2.521 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_allocated_bytes | 12.567 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_logical_bytes | 12.435 | cross-batch |
| prior_pi | live-subscriber | cancel | lifecycle_ms | 1.226 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_pss_mib | 1.397 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_turn_rss_mib | 1.039 | cross-batch |
| prior_pi | live-subscriber | cancel | measured_turn_write_bytes | 2.970 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_bytes | 4.365 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_mib | 4.365 | cross-batch |
| prior_pi | live-subscriber | cancel | turn_write_bytes_per_turn | 2.970 | cross-batch |
| prior_deepseek | live-subscriber | short-small | mean_turn_ms | 1.134 | cross-batch |
| prior_deepseek | live-subscriber | short-small | median_measured_turn_ms | 1.105 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_cpu_ms_per_turn | 1.345 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_write_bytes_per_turn | 42.940 | cross-batch |
| prior_deepseek | live-subscriber | short-large | mean_turn_ms | 1.178 | cross-batch |
| prior_deepseek | live-subscriber | short-large | median_measured_turn_ms | 1.152 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_cpu_ms_per_turn | 1.304 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_write_bytes_per_turn | 51.856 | cross-batch |
| prior_deepseek | live-subscriber | stream | turn_write_bytes_per_turn | 45.000 | cross-batch |
| prior_deepseek | live-subscriber | large | turn_write_bytes_per_turn | 29.714 | cross-batch |

- Only the fresh old/new Rust products are interleaved pairs. Pi/DeepSeek use fixed previous batches and separate descriptive ratios of medians; no causal parity or paired confidence claim.
- /proc CPU counters use 100 ticks/s; each cumulative endpoint has 10 ms granularity. This is not profiler sampling.
- A product measured-turn median and a product measured-turn mean are different estimands. Across products we report median and range of those statistics, not pooled turns.
- DeepSeek SDK idle does not await flush/fsync. Native wait4 CPU covers exit/flush and verification separately; full process write_bytes is unavailable, not zero. No equivalent lifecycle ratio is constructed.
- All recorded adverse pairs and all unavailable ratios are retained, including reversals hidden by a favorable group median.

Unavailable ratio records, all per-product values, and reference hashes are retained in report-data.json.
