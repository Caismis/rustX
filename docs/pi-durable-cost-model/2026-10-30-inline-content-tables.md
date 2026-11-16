# Inline immutable content and construction reuse performance evidence

Fresh Rust comparison: five interleaved pairs per cell. Historical Pi/DeepSeek rows are fixed, separate batches; their ratios are ratios of medians, not paired results.

| Mode | Case | Paired median wall % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -0.278 [-38.042,96.404] | 0.800 | -5.980 | -0.263 | -0.293 |
| no-live-subscriber | short-large | 4.352 [-23.664,51.836] | 0.255 | -7.421 | 0.113 | 0.051 |
| no-live-subscriber | stream | 0.548 [-0.921,1.946] | 7.143 | -0.123 | 0.342 | 0.043 |
| no-live-subscriber | large | 8.604 [-76.642,27.159] | 0.000 | -0.838 | 0.348 | -0.021 |
| no-live-subscriber | tool | 0.028 [-11.707,30.866] | 6.250 | -0.339 | -0.355 | -0.344 |
| no-live-subscriber | cancel | -5.064 [-11.168,18.644] | 6.667 | -0.576 | 0.712 | 0.140 |
| live-subscriber | short-small | 66.937 [-49.706,92.796] | 23.478 | -6.013 | -0.316 | -0.484 |
| live-subscriber | short-large | -1.361 [-5.812,0.705] | 0.427 | -7.219 | -0.099 | -0.166 |
| live-subscriber | stream | 1.852 [-1.974,3.157] | 6.897 | -0.123 | 0.119 | 0.016 |
| live-subscriber | large | 13.617 [-13.877,28.399] | 5.882 | -0.838 | -0.889 | -0.503 |
| live-subscriber | tool | 14.610 [-11.800,22.557] | 5.882 | -0.339 | -0.261 | -0.412 |
| live-subscriber | cancel | 10.255 [-2.826,18.663] | 0.000 | -0.576 | 0.164 | -0.189 |

| Mode | Case | Mean wall / old Pi | Median wall / old Pi | Full CPU / old Pi | Full writes / old Pi |
|---|---|---:|---:|---:|---:|
| no-live-subscriber | short-small | 4.198 | 4.769 | 1.340 | 3.812 |
| no-live-subscriber | short-large | 4.602 | 5.170 | 2.851 | 4.214 |
| no-live-subscriber | stream | 0.927 | 0.927 | 0.273 | 3.695 |
| no-live-subscriber | large | 0.449 | 0.449 | 0.271 | 4.128 |
| no-live-subscriber | tool | 0.393 | 0.393 | 0.286 | 3.671 |
| no-live-subscriber | cancel | 0.312 | 0.312 | 0.246 | 4.340 |
| live-subscriber | short-small | 3.978 | 4.667 | 1.464 | 3.812 |
| live-subscriber | short-large | 5.047 | 5.589 | 3.140 | 4.218 |
| live-subscriber | stream | 0.917 | 0.917 | 0.270 | 3.695 |
| live-subscriber | large | 0.480 | 0.480 | 0.288 | 4.128 |
| live-subscriber | tool | 0.490 | 0.490 | 0.286 | 3.671 |
| live-subscriber | cancel | 0.378 | 0.378 | 0.267 | 4.340 |

Warm measured windows (per-product statistic, then median [minimum,maximum] across five Rust/Pi products; each has 32 measured turns):

| Mode | History | Engine/batch | Mean ms/turn | Median ms/turn | CPU ms/turn | Write bytes/turn |
|---|---:|---|---:|---:|---:|---:|
| no-live-subscriber | 8 | current Rust | 15.219 [12.100,17.067] | 14.950 [9.700,16.521] | 17.500 [14.062,19.375] | 853888.000 [853760.000,854144.000] |
| no-live-subscriber | 8 | fixed old Pi | 3.625 [3.524,4.546] | 3.135 [3.014,3.328] | 7.188 [6.875,9.688] | 224512.000 [224512.000,224512.000] |
| no-live-subscriber | 64 | current Rust | 17.012 [13.521,18.992] | 17.056 [13.386,18.079] | 19.375 [15.938,20.938] | 1009280.000 [1008512.000,1013120.000] |
| no-live-subscriber | 64 | fixed old Pi | 3.697 [3.667,3.911] | 3.299 [3.159,3.524] | 6.250 [5.938,6.875] | 224384.000 [224384.000,224384.000] |
| live-subscriber | 8 | current Rust | 15.646 [9.562,18.281] | 15.960 [9.146,17.517] | 21.250 [13.750,25.000] | 853632.000 [853376.000,853632.000] |
| live-subscriber | 8 | fixed old Pi | 3.934 [3.784,4.165] | 3.420 [3.309,3.699] | 7.500 [7.188,8.438] | 224512.000 [224512.000,224512.000] |
| live-subscriber | 64 | current Rust | 20.133 [19.920,21.056] | 19.787 [19.463,20.081] | 27.500 [27.188,28.750] | 1012096.000 [1011584.000,1013504.000] |
| live-subscriber | 64 | fixed old Pi | 3.989 [3.827,4.233] | 3.541 [3.482,3.679] | 6.875 [6.250,7.188] | 224384.000 [224384.000,224384.000] |

| Live case | Current mean ms/turn | Fixed DS mean ms/turn | Wall ratio | CPU ratio | Measured write ratio |
|---|---:|---:|---:|---:|---:|
| short-small | 15.646 | 15.541 | 1.007 | 1.172 | 39.696 |
| short-large | 20.133 | 15.450 | 1.303 | 1.571 | 47.347 |
| stream | 1592.490 | 1583.218 | 1.006 | 0.533 | 44.714 |
| large | 43.170 | 165.613 | 0.261 | 0.250 | 29.143 |

DeepSeek exit/flush CPU remains separate from measured turns and from Rust full-lifecycle CPU; full DeepSeek process write bytes remain unavailable. The JSON retains the exact wait4 metrics and source-specific limits.

All adverse metrics (including individual fresh paired reversals):

| Comparison | Mode | Case | Metric | Median % or cross-batch ratio | Adverse pairs |
|---|---|---|---|---:|---|
| fresh_paired_rust | no-live-subscriber | short-small | mean_turn_ms | -4.049 | 0:+5.793%,2:+87.876% |
| fresh_paired_rust | no-live-subscriber | short-small | turn_cpu_ms_per_turn | -3.390 | 0:+3.704%,2:+82.353% |
| fresh_paired_rust | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 0.800 | 0:+3.226%,2:+29.245%,4:+0.800% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_ms | 1.871 | 0:+3.179%,2:+26.377%,3:+1.871%,4:+0.575% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_anon_pss_mib | -0.263 | 0:+0.616%,3:+0.029% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_pss_mib | -0.346 | 0:+0.257% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_rss_mib | -0.293 | 0:+0.264% |
| fresh_paired_rust | no-live-subscriber | short-small | median_measured_turn_ms | -0.278 | 0:+4.033%,2:+96.404% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_self_cpu_ms | 0.800 | 0:+3.226%,2:+29.245%,4:+0.800% |
| fresh_paired_rust | no-live-subscriber | short-small | measured_turn_cpu_ms | -3.390 | 0:+3.704%,2:+82.353% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_turn_rss_mib | -0.585 | 0:+0.479% |
| fresh_paired_rust | no-live-subscriber | short-small | through_product_exit_wall_ms | 1.797 | 0:+2.916%,2:+24.157%,3:+1.797%,4:+0.325% |
| fresh_paired_rust | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 1.652 | 0:+1.652%,2:+22.462%,3:+2.014% |
| fresh_paired_rust | no-live-subscriber | short-large | mean_turn_ms | 13.264 | 0:+35.614%,2:+13.264%,4:+19.437% |
| fresh_paired_rust | no-live-subscriber | short-large | turn_cpu_ms_per_turn | 9.836 | 0:+34.043%,2:+9.836%,4:+19.231% |
| fresh_paired_rust | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 0.255 | 0:+4.134%,2:+1.481%,4:+0.255% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_ms | -0.303 | 0:+4.664%,2:+1.920% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_anon_pss_mib | 0.113 | 1:+0.050%,2:+0.380%,3:+0.310%,4:+0.113% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_pss_mib | 0.032 | 2:+0.329%,3:+0.032%,4:+0.191% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_rss_mib | 0.051 | 2:+0.326%,3:+0.051%,4:+0.193% |
| fresh_paired_rust | no-live-subscriber | short-large | median_measured_turn_ms | 4.352 | 0:+51.836%,2:+4.352%,4:+34.417% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_self_cpu_ms | 0.255 | 0:+4.134%,2:+1.481%,4:+0.255% |
| fresh_paired_rust | no-live-subscriber | short-large | measured_turn_cpu_ms | 9.836 | 0:+34.043%,2:+9.836%,4:+19.231% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_turn_rss_mib | -0.156 | 2:+0.634%,4:+0.433% |
| fresh_paired_rust | no-live-subscriber | short-large | through_product_exit_wall_ms | -0.134 | 0:+4.428%,2:+2.247% |
| fresh_paired_rust | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | -0.161 | 0:+4.189%,2:+2.165% |
| fresh_paired_rust | no-live-subscriber | stream | mean_turn_ms | 0.548 | 0:+0.548%,2:+1.946%,3:+0.163%,4:+1.735% |
| fresh_paired_rust | no-live-subscriber | stream | turn_cpu_ms_per_turn | 6.667 | 2:+20.000%,3:+6.667%,4:+23.077% |
| fresh_paired_rust | no-live-subscriber | stream | through_cleanup_self_cpu_ms | 7.143 | 2:+21.429%,3:+7.143%,4:+15.385% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_ms | 0.623 | 0:+0.623%,2:+3.618%,3:+0.324%,4:+2.298% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_anon_pss_mib | 0.342 | 0:+0.342%,1:+0.982%,2:+0.645%,3:+0.013% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_pss_mib | 0.017 | 1:+0.917%,2:+0.017%,3:+0.107% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_rss_mib | 0.043 | 1:+0.925%,2:+0.043%,3:+0.168% |
| fresh_paired_rust | no-live-subscriber | stream | median_measured_turn_ms | 0.548 | 0:+0.548%,2:+1.946%,3:+0.163%,4:+1.735% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_self_cpu_ms | 7.143 | 2:+21.429%,3:+7.143%,4:+15.385% |
| fresh_paired_rust | no-live-subscriber | stream | measured_turn_cpu_ms | 6.667 | 2:+20.000%,3:+6.667%,4:+23.077% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_turn_rss_mib | -0.157 | 1:+0.851%,3:+0.297% |
| fresh_paired_rust | no-live-subscriber | stream | through_product_exit_wall_ms | 0.685 | 0:+0.685%,2:+3.630%,3:+0.351%,4:+2.356% |
| fresh_paired_rust | no-live-subscriber | stream | complete_harness_lifecycle_wall_ms | 1.521 | 0:+0.599%,2:+3.370%,3:+1.521%,4:+2.481% |
| fresh_paired_rust | no-live-subscriber | large | mean_turn_ms | 8.604 | 0:+25.170%,1:+8.604%,3:+27.159% |
| fresh_paired_rust | no-live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 0:+33.333%,3:+33.333% |
| fresh_paired_rust | no-live-subscriber | large | through_cleanup_self_cpu_ms | 0.000 | 0:+13.333%,3:+20.000% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_ms | -0.357 | 3:+6.654%,4:+1.308% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_anon_pss_mib | 0.348 | 0:+0.348%,1:+0.768%,2:+0.426%,4:+0.190% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_pss_mib | -0.030 | 1:+0.612%,4:+0.041% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_rss_mib | -0.021 | 1:+0.603%,4:+0.051% |
| fresh_paired_rust | no-live-subscriber | large | median_measured_turn_ms | 8.604 | 0:+25.170%,1:+8.604%,3:+27.159% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_self_cpu_ms | 0.000 | 0:+13.333%,3:+20.000% |
| fresh_paired_rust | no-live-subscriber | large | measured_turn_cpu_ms | 0.000 | 0:+33.333%,3:+33.333% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_turn_rss_mib | -0.198 | 1:+0.575%,3:+0.118% |
| fresh_paired_rust | no-live-subscriber | large | through_product_exit_wall_ms | -0.197 | 3:+5.851%,4:+1.971% |
| fresh_paired_rust | no-live-subscriber | large | complete_harness_lifecycle_wall_ms | 0.809 | 0:+0.809%,3:+5.982%,4:+3.006% |
| fresh_paired_rust | no-live-subscriber | tool | mean_turn_ms | 0.028 | 0:+0.028%,1:+30.866%,4:+13.598% |
| fresh_paired_rust | no-live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 0:+33.333%,1:+25.000% |
| fresh_paired_rust | no-live-subscriber | tool | through_cleanup_self_cpu_ms | 6.250 | 0:+6.250%,1:+11.765%,3:+5.882%,4:+12.500% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_ms | 1.814 | 0:+7.417%,1:+0.431%,3:+1.814%,4:+1.912% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_anon_pss_mib | -0.355 | 1:+0.048% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_pss_mib | -0.393 | 0:+0.091% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_rss_mib | -0.344 | 0:+0.092% |
| fresh_paired_rust | no-live-subscriber | tool | median_measured_turn_ms | 0.028 | 0:+0.028%,1:+30.866%,4:+13.598% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_self_cpu_ms | 6.250 | 0:+6.250%,1:+11.765%,3:+5.882%,4:+12.500% |
| fresh_paired_rust | no-live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 0:+33.333%,1:+25.000% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_turn_rss_mib | -0.628 | 4:+0.038% |
| fresh_paired_rust | no-live-subscriber | tool | through_product_exit_wall_ms | 2.079 | 0:+6.913%,3:+2.923%,4:+2.079% |
| fresh_paired_rust | no-live-subscriber | tool | complete_harness_lifecycle_wall_ms | 2.068 | 0:+6.049%,3:+7.122%,4:+2.068% |
| fresh_paired_rust | no-live-subscriber | cancel | mean_turn_ms | -5.064 | 0:+18.644%,3:+3.780% |
| fresh_paired_rust | no-live-subscriber | cancel | turn_cpu_ms_per_turn | 0.000 | 3:+33.333% |
| fresh_paired_rust | no-live-subscriber | cancel | through_cleanup_self_cpu_ms | 6.667 | 0:+7.143%,3:+7.143%,4:+6.667% |
| fresh_paired_rust | no-live-subscriber | cancel | lifecycle_ms | 1.438 | 0:+1.438%,2:+3.861%,4:+4.472% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_anon_pss_mib | 0.712 | 0:+0.726%,1:+0.712%,2:+0.089%,3:+1.339%,4:+0.013% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_pss_mib | 0.142 | 0:+0.546%,1:+0.142%,3:+0.216% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_rss_mib | 0.140 | 0:+0.574%,1:+0.140%,3:+0.212% |
| fresh_paired_rust | no-live-subscriber | cancel | median_measured_turn_ms | -5.064 | 0:+18.644%,3:+3.780% |
| fresh_paired_rust | no-live-subscriber | cancel | lifecycle_self_cpu_ms | 6.667 | 0:+7.143%,3:+7.143%,4:+6.667% |
| fresh_paired_rust | no-live-subscriber | cancel | measured_turn_cpu_ms | 0.000 | 3:+33.333% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_turn_rss_mib | -0.251 | 0:+0.487%,1:+0.127% |
| fresh_paired_rust | no-live-subscriber | cancel | through_product_exit_wall_ms | 2.269 | 0:+2.269%,2:+5.384%,3:+0.272%,4:+4.392% |
| fresh_paired_rust | no-live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 2.138 | 0:+2.138%,2:+2.441%,3:+1.026%,4:+3.103% |
| fresh_paired_rust | live-subscriber | short-small | mean_turn_ms | 52.604 | 0:+82.135%,2:+92.881%,4:+52.604% |
| fresh_paired_rust | live-subscriber | short-small | turn_cpu_ms_per_turn | 47.826 | 0:+75.556%,2:+86.047%,4:+47.826% |
| fresh_paired_rust | live-subscriber | short-small | through_cleanup_self_cpu_ms | 23.478 | 0:+36.607%,2:+36.937%,4:+23.478% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_ms | 17.542 | 0:+29.091%,2:+28.357%,4:+17.542% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_anon_pss_mib | -0.316 | 0:+0.675%,2:+0.287% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_pss_mib | -0.451 | 0:+0.681%,2:+0.548% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_rss_mib | -0.484 | 0:+0.660%,2:+0.542% |
| fresh_paired_rust | live-subscriber | short-small | median_measured_turn_ms | 66.937 | 0:+92.796%,2:+90.598%,4:+66.937% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_self_cpu_ms | 23.478 | 0:+36.607%,2:+36.937%,4:+23.478% |
| fresh_paired_rust | live-subscriber | short-small | measured_turn_cpu_ms | 47.826 | 0:+75.556%,2:+86.047%,4:+47.826% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_turn_rss_mib | -0.566 | 0:+0.532%,2:+0.475% |
| fresh_paired_rust | live-subscriber | short-small | through_product_exit_wall_ms | 16.686 | 0:+27.227%,2:+26.962%,4:+16.686% |
| fresh_paired_rust | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 15.185 | 0:+23.676%,2:+25.548%,4:+15.185% |
| fresh_paired_rust | live-subscriber | short-large | mean_turn_ms | -3.529 | 1:+3.682%,2:+0.081% |
| fresh_paired_rust | live-subscriber | short-large | turn_cpu_ms_per_turn | -3.297 | 1:+5.952% |
| fresh_paired_rust | live-subscriber | short-large | through_cleanup_self_cpu_ms | 0.427 | 1:+2.169%,2:+0.427%,3:+1.253% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_ms | 0.689 | 1:+2.103%,2:+0.689%,3:+0.995%,4:+0.001% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_anon_pss_mib | -0.099 | 0:+0.109%,4:+0.104% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_pss_mib | -0.167 | 4:+0.111% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_rss_mib | -0.166 | 4:+0.110% |
| fresh_paired_rust | live-subscriber | short-large | median_measured_turn_ms | -1.361 | 1:+0.705% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_self_cpu_ms | 0.427 | 1:+2.169%,2:+0.427%,3:+1.253% |
| fresh_paired_rust | live-subscriber | short-large | measured_turn_cpu_ms | -3.297 | 1:+5.952% |
| fresh_paired_rust | live-subscriber | short-large | through_product_exit_wall_ms | 0.409 | 1:+1.681%,2:+0.409%,3:+1.137%,4:+0.051% |
| fresh_paired_rust | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 0.574 | 1:+1.437%,2:+0.574%,3:+1.051%,4:+0.026% |
| fresh_paired_rust | live-subscriber | stream | mean_turn_ms | 1.852 | 0:+1.993%,2:+3.157%,4:+1.852% |
| fresh_paired_rust | live-subscriber | stream | turn_cpu_ms_per_turn | 14.286 | 0:+14.286%,2:+38.462%,4:+20.000% |
| fresh_paired_rust | live-subscriber | stream | through_cleanup_self_cpu_ms | 6.897 | 0:+15.385%,2:+23.077%,4:+6.897% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_ms | 1.410 | 0:+3.152%,2:+4.033%,4:+1.410% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_anon_pss_mib | 0.119 | 0:+0.711%,1:+0.119%,4:+1.390% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_pss_mib | -0.028 | 4:+0.639% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_rss_mib | 0.016 | 0:+0.043%,2:+0.016%,4:+0.579% |
| fresh_paired_rust | live-subscriber | stream | median_measured_turn_ms | 1.852 | 0:+1.993%,2:+3.157%,4:+1.852% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_self_cpu_ms | 6.897 | 0:+15.385%,2:+23.077%,4:+6.897% |
| fresh_paired_rust | live-subscriber | stream | measured_turn_cpu_ms | 14.286 | 0:+14.286%,2:+38.462%,4:+20.000% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_turn_rss_mib | -0.125 | 4:+0.812% |
| fresh_paired_rust | live-subscriber | stream | through_product_exit_wall_ms | 1.115 | 0:+3.165%,2:+4.040%,4:+1.115% |
| fresh_paired_rust | live-subscriber | stream | complete_harness_lifecycle_wall_ms | 0.435 | 0:+2.259%,2:+4.210%,4:+0.435% |
| fresh_paired_rust | live-subscriber | large | mean_turn_ms | 13.617 | 0:+28.399%,1:+18.737%,3:+13.617%,4:+4.693% |
| fresh_paired_rust | live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 0:+50.000%,3:+25.000% |
| fresh_paired_rust | live-subscriber | large | through_cleanup_self_cpu_ms | 5.882 | 0:+6.250%,1:+12.500%,3:+5.882% |
| fresh_paired_rust | live-subscriber | large | lifecycle_ms | -2.109 | 0:+9.720%,1:+13.120% |
| fresh_paired_rust | live-subscriber | large | max_sampled_anon_pss_mib | -0.889 | 1:+2.061% |
| fresh_paired_rust | live-subscriber | large | max_sampled_pss_mib | -0.537 | 1:+1.096% |
| fresh_paired_rust | live-subscriber | large | max_sampled_rss_mib | -0.503 | 1:+1.085% |
| fresh_paired_rust | live-subscriber | large | median_measured_turn_ms | 13.617 | 0:+28.399%,1:+18.737%,3:+13.617%,4:+4.693% |
| fresh_paired_rust | live-subscriber | large | lifecycle_self_cpu_ms | 5.882 | 0:+6.250%,1:+12.500%,3:+5.882% |
| fresh_paired_rust | live-subscriber | large | measured_turn_cpu_ms | 0.000 | 0:+50.000%,3:+25.000% |
| fresh_paired_rust | live-subscriber | large | max_sampled_turn_rss_mib | -0.330 | 1:+0.947% |
| fresh_paired_rust | live-subscriber | large | through_product_exit_wall_ms | 0.857 | 0:+8.883%,1:+11.280%,2:+0.857% |
| fresh_paired_rust | live-subscriber | large | complete_harness_lifecycle_wall_ms | 5.998 | 0:+6.939%,1:+8.955%,2:+5.998% |
| fresh_paired_rust | live-subscriber | tool | mean_turn_ms | 14.610 | 2:+22.557%,3:+14.610%,4:+16.773% |
| fresh_paired_rust | live-subscriber | tool | turn_cpu_ms_per_turn | 16.667 | 1:+16.667%,2:+25.000%,4:+20.000% |
| fresh_paired_rust | live-subscriber | tool | through_cleanup_self_cpu_ms | 5.882 | 2:+18.750%,3:+5.882%,4:+17.647% |
| fresh_paired_rust | live-subscriber | tool | lifecycle_ms | 2.451 | 2:+3.386%,3:+2.451%,4:+18.722% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_anon_pss_mib | -0.261 | 2:+1.075%,4:+2.382% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_pss_mib | -0.429 | 2:+0.667%,4:+1.328% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_rss_mib | -0.412 | 2:+0.657%,4:+1.308% |
| fresh_paired_rust | live-subscriber | tool | median_measured_turn_ms | 14.610 | 2:+22.557%,3:+14.610%,4:+16.773% |
| fresh_paired_rust | live-subscriber | tool | lifecycle_self_cpu_ms | 5.882 | 2:+18.750%,3:+5.882%,4:+17.647% |
| fresh_paired_rust | live-subscriber | tool | measured_turn_cpu_ms | 16.667 | 1:+16.667%,2:+25.000%,4:+20.000% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_turn_rss_mib | -0.353 | 2:+0.507%,4:+0.906% |
| fresh_paired_rust | live-subscriber | tool | through_product_exit_wall_ms | 1.808 | 0:+1.808%,2:+4.716%,3:+1.771%,4:+19.708% |
| fresh_paired_rust | live-subscriber | tool | complete_harness_lifecycle_wall_ms | 3.127 | 0:+3.127%,2:+6.321%,3:+1.653%,4:+18.914% |
| fresh_paired_rust | live-subscriber | cancel | mean_turn_ms | 10.255 | 0:+3.828%,1:+18.663%,2:+12.270%,4:+10.255% |
| fresh_paired_rust | live-subscriber | cancel | turn_cpu_ms_per_turn | 0.000 | 1:+25.000%,2:+25.000% |
| fresh_paired_rust | live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 2:+6.250%,4:+6.667% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_ms | 2.028 | 1:+3.280%,2:+2.028%,4:+3.212% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_anon_pss_mib | 0.164 | 2:+0.227%,3:+0.164%,4:+0.257% |
| fresh_paired_rust | live-subscriber | cancel | median_measured_turn_ms | 10.255 | 0:+3.828%,1:+18.663%,2:+12.270%,4:+10.255% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 2:+6.250%,4:+6.667% |
| fresh_paired_rust | live-subscriber | cancel | measured_turn_cpu_ms | 0.000 | 1:+25.000%,2:+25.000% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_turn_rss_mib | -0.542 | 1:+0.011% |
| fresh_paired_rust | live-subscriber | cancel | through_product_exit_wall_ms | 2.135 | 1:+2.947%,2:+2.135%,4:+3.828% |
| fresh_paired_rust | live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 1.340 | 1:+3.759%,2:+1.340%,4:+1.989% |
| prior_pi | no-live-subscriber | short-small | closed_allocated_bytes | 40.476 | cross-batch |
| prior_pi | no-live-subscriber | short-small | closed_logical_bytes | 40.383 | cross-batch |
| prior_pi | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 1.874 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_ms | 2.890 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_self_cpu_ms | 2.471 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_anon_pss_mib | 1.122 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_pss_mib | 1.479 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_rss_mib | 1.080 | cross-batch |
| prior_pi | no-live-subscriber | short-small | mean_turn_ms | 4.198 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_cpu_ms | 2.435 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_write_bytes | 3.803 | cross-batch |
| prior_pi | no-live-subscriber | short-small | median_measured_turn_ms | 4.769 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_bytes | 3.812 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_mib | 3.812 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.340 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_product_exit_wall_ms | 1.888 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_cpu_ms_per_turn | 2.435 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_write_bytes_per_turn | 3.803 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_allocated_bytes | 39.850 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_logical_bytes | 39.785 | cross-batch |
| prior_pi | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.574 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_ms | 4.800 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_self_cpu_ms | 4.061 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_anon_pss_mib | 1.276 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_pss_mib | 1.517 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_rss_mib | 1.202 | cross-batch |
| prior_pi | no-live-subscriber | short-large | mean_turn_ms | 4.602 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_cpu_ms | 3.100 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_write_bytes | 4.498 | cross-batch |
| prior_pi | no-live-subscriber | short-large | median_measured_turn_ms | 5.170 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_bytes | 4.214 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_mib | 4.214 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 2.851 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_product_exit_wall_ms | 3.690 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_cpu_ms_per_turn | 3.100 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_write_bytes_per_turn | 4.498 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_allocated_bytes | 15.344 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_logical_bytes | 15.242 | cross-batch |
| prior_pi | no-live-subscriber | stream | max_sampled_pss_mib | 1.253 | cross-batch |
| prior_pi | no-live-subscriber | stream | measured_turn_write_bytes | 2.484 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_bytes | 3.695 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_mib | 3.695 | cross-batch |
| prior_pi | no-live-subscriber | stream | turn_write_bytes_per_turn | 2.484 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_allocated_bytes | 10.053 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_logical_bytes | 9.950 | cross-batch |
| prior_pi | no-live-subscriber | large | lifecycle_ms | 1.244 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_anon_pss_mib | 1.089 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_pss_mib | 1.554 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_rss_mib | 1.051 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_turn_rss_mib | 1.045 | cross-batch |
| prior_pi | no-live-subscriber | large | measured_turn_write_bytes | 2.833 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_bytes | 4.128 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_mib | 4.128 | cross-batch |
| prior_pi | no-live-subscriber | large | turn_write_bytes_per_turn | 2.833 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_allocated_bytes | 16.938 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_logical_bytes | 16.813 | cross-batch |
| prior_pi | no-live-subscriber | tool | lifecycle_ms | 1.180 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_pss_mib | 1.377 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_turn_rss_mib | 1.053 | cross-batch |
| prior_pi | no-live-subscriber | tool | measured_turn_write_bytes | 2.493 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_bytes | 3.671 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_mib | 3.671 | cross-batch |
| prior_pi | no-live-subscriber | tool | turn_write_bytes_per_turn | 2.493 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_allocated_bytes | 12.467 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_logical_bytes | 12.335 | cross-batch |
| prior_pi | no-live-subscriber | cancel | lifecycle_ms | 1.184 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_pss_mib | 1.403 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_turn_rss_mib | 1.038 | cross-batch |
| prior_pi | no-live-subscriber | cancel | measured_turn_write_bytes | 2.925 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_bytes | 4.340 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_mib | 4.340 | cross-batch |
| prior_pi | no-live-subscriber | cancel | turn_write_bytes_per_turn | 2.925 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_allocated_bytes | 40.476 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_logical_bytes | 40.383 | cross-batch |
| prior_pi | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 1.875 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_ms | 2.856 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_self_cpu_ms | 2.630 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_anon_pss_mib | 1.194 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_pss_mib | 1.550 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_rss_mib | 1.126 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_turn_rss_mib | 1.011 | cross-batch |
| prior_pi | live-subscriber | short-small | mean_turn_ms | 3.978 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_cpu_ms | 2.833 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_write_bytes | 3.802 | cross-batch |
| prior_pi | live-subscriber | short-small | median_measured_turn_ms | 4.667 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_bytes | 3.812 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_mib | 3.812 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.464 | cross-batch |
| prior_pi | live-subscriber | short-small | through_product_exit_wall_ms | 1.888 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_cpu_ms_per_turn | 2.833 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_write_bytes_per_turn | 3.802 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_allocated_bytes | 39.850 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_logical_bytes | 39.785 | cross-batch |
| prior_pi | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.666 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_ms | 4.883 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_self_cpu_ms | 4.443 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_anon_pss_mib | 1.280 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_pss_mib | 1.520 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_rss_mib | 1.208 | cross-batch |
| prior_pi | live-subscriber | short-large | mean_turn_ms | 5.047 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_cpu_ms | 4.000 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_write_bytes | 4.511 | cross-batch |
| prior_pi | live-subscriber | short-large | median_measured_turn_ms | 5.589 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_bytes | 4.218 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_mib | 4.218 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_self_cpu_ms | 3.140 | cross-batch |
| prior_pi | live-subscriber | short-large | through_product_exit_wall_ms | 3.759 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_cpu_ms_per_turn | 4.000 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_write_bytes_per_turn | 4.511 | cross-batch |
| prior_pi | live-subscriber | stream | closed_allocated_bytes | 15.344 | cross-batch |
| prior_pi | live-subscriber | stream | closed_logical_bytes | 15.242 | cross-batch |
| prior_pi | live-subscriber | stream | max_sampled_pss_mib | 1.258 | cross-batch |
| prior_pi | live-subscriber | stream | measured_turn_write_bytes | 2.484 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_bytes | 3.695 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_mib | 3.695 | cross-batch |
| prior_pi | live-subscriber | stream | turn_write_bytes_per_turn | 2.484 | cross-batch |
| prior_pi | live-subscriber | large | closed_allocated_bytes | 10.053 | cross-batch |
| prior_pi | live-subscriber | large | closed_logical_bytes | 9.950 | cross-batch |
| prior_pi | live-subscriber | large | lifecycle_ms | 1.219 | cross-batch |
| prior_pi | live-subscriber | large | lifecycle_self_cpu_ms | 1.062 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_anon_pss_mib | 1.089 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_pss_mib | 1.558 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_rss_mib | 1.053 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_turn_rss_mib | 1.050 | cross-batch |
| prior_pi | live-subscriber | large | measured_turn_write_bytes | 2.833 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_bytes | 4.128 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_mib | 4.128 | cross-batch |
| prior_pi | live-subscriber | large | turn_write_bytes_per_turn | 2.833 | cross-batch |
| prior_pi | live-subscriber | tool | closed_allocated_bytes | 16.938 | cross-batch |
| prior_pi | live-subscriber | tool | closed_logical_bytes | 16.813 | cross-batch |
| prior_pi | live-subscriber | tool | lifecycle_ms | 1.182 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_pss_mib | 1.369 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_turn_rss_mib | 1.054 | cross-batch |
| prior_pi | live-subscriber | tool | measured_turn_write_bytes | 2.493 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_bytes | 3.671 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_mib | 3.671 | cross-batch |
| prior_pi | live-subscriber | tool | turn_write_bytes_per_turn | 2.493 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_allocated_bytes | 12.467 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_logical_bytes | 12.335 | cross-batch |
| prior_pi | live-subscriber | cancel | lifecycle_ms | 1.223 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_pss_mib | 1.391 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_turn_rss_mib | 1.033 | cross-batch |
| prior_pi | live-subscriber | cancel | measured_turn_write_bytes | 2.925 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_bytes | 4.340 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_mib | 4.340 | cross-batch |
| prior_pi | live-subscriber | cancel | turn_write_bytes_per_turn | 2.925 | cross-batch |
| prior_deepseek | live-subscriber | short-small | mean_turn_ms | 1.007 | cross-batch |
| prior_deepseek | live-subscriber | short-small | median_measured_turn_ms | 1.080 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_cpu_ms_per_turn | 1.172 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_write_bytes_per_turn | 39.696 | cross-batch |
| prior_deepseek | live-subscriber | short-large | mean_turn_ms | 1.303 | cross-batch |
| prior_deepseek | live-subscriber | short-large | median_measured_turn_ms | 1.290 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_cpu_ms_per_turn | 1.571 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_write_bytes_per_turn | 47.347 | cross-batch |
| prior_deepseek | live-subscriber | stream | mean_turn_ms | 1.006 | cross-batch |
| prior_deepseek | live-subscriber | stream | median_measured_turn_ms | 1.006 | cross-batch |
| prior_deepseek | live-subscriber | stream | turn_write_bytes_per_turn | 44.714 | cross-batch |
| prior_deepseek | live-subscriber | large | turn_write_bytes_per_turn | 29.143 | cross-batch |

- Only the fresh old/new Rust products are interleaved pairs. Pi/DeepSeek use fixed previous batches and separate descriptive ratios of medians; no causal parity or paired confidence claim.
- /proc CPU counters use 100 ticks/s; each cumulative endpoint has 10 ms granularity. This is not profiler sampling.
- A product measured-turn median and a product measured-turn mean are different estimands. Across products we report median and range of those statistics, not pooled turns.
- DeepSeek SDK idle does not await flush/fsync. Native wait4 CPU covers exit/flush and verification separately; full process write_bytes is unavailable, not zero. No equivalent lifecycle ratio is constructed.
- All recorded adverse pairs and all unavailable ratios are retained, including reversals hidden by a favorable group median.

Unavailable ratio records, all per-product values, and reference hashes are retained in report-data.json.
