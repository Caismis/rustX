# Incremental Attempt and narrow-index performance evidence

Only fresh CPU-2,8 Rust baseline/candidate products are compared. Fixed Pi/DeepSeek measurements used unrestricted CPUs in an earlier batch; their original values are retained separately and no ratio combines those batches.

Fresh Rust: six interleaved pairs per cell, CPU affinity 2,8, with three pairs in each execution order.

| Mode | Case | Paired wall median % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -1.572 [-23.181,1.683] | -2.210 | -0.336 | -0.754 | -0.667 |
| no-live-subscriber | short-large | 5.109 [-2.149,13.852] | 0.812 | -0.560 | -0.104 | -0.199 |
| no-live-subscriber | stream | -0.240 [-2.340,1.966] | 2.083 | 0.000 | -0.115 | -0.196 |
| no-live-subscriber | large | -0.884 [-8.107,7.695] | -3.571 | 0.000 | 0.000 | -0.214 |
| no-live-subscriber | tool | -0.995 [-7.793,13.025] | 0.000 | 0.000 | -0.289 | -0.470 |
| no-live-subscriber | cancel | 1.519 [-15.187,51.698] | 0.000 | 0.000 | -0.490 | -0.664 |
| live-subscriber | short-small | -3.139 [-8.930,1.137] | -1.526 | -0.348 | -0.181 | -0.351 |
| live-subscriber | short-large | 4.987 [2.143,25.022] | 1.358 | -0.614 | -0.330 | -0.420 |
| live-subscriber | stream | -1.794 [-2.428,0.564] | -10.000 | 0.000 | -0.084 | -0.330 |
| live-subscriber | large | 0.583 [-1.097,6.185] | 0.000 | 0.000 | 0.031 | -0.188 |
| live-subscriber | tool | -0.183 [-6.009,4.801] | 0.000 | 0.000 | 0.050 | -0.198 |
| live-subscriber | cancel | -0.500 [-3.273,2.910] | 0.000 | 0.000 | 0.092 | -0.266 |

Fresh Rust absolute warm-request windows, each product contributing 32 measured turns:

| Mode | History | Revision | Mean ms/turn median [min,max] | Median ms/turn median [min,max] | CPU ms/turn | Write bytes/turn |
|---|---:|---|---:|---:|---:|---:|
| no-live-subscriber | 8 | baseline | 7.476 [7.390,29.980] | 6.986 [6.804,10.103] | 8.594 [8.125,12.500] | 850368.000 [849920.000,850560.000] |
| no-live-subscriber | 8 | candidate | 7.460 [7.310,31.648] | 6.878 [6.748,7.761] | 8.438 [8.125,10.625] | 846080.000 [845696.000,848384.000] |
| no-live-subscriber | 64 | baseline | 8.296 [8.254,8.350] | 7.532 [7.449,7.702] | 9.375 [9.062,9.375] | 1005056.000 [1003776.000,1008768.000] |
| no-live-subscriber | 64 | candidate | 8.670 [8.309,15.472] | 7.885 [7.507,8.585] | 9.688 [9.375,10.938] | 1006336.000 [1004288.000,1008384.000] |
| live-subscriber | 8 | baseline | 8.600 [8.218,9.346] | 7.959 [7.694,8.981] | 10.469 [10.000,11.250] | 850304.000 [850176.000,850432.000] |
| live-subscriber | 8 | candidate | 8.256 [8.120,8.898] | 7.744 [7.619,8.179] | 10.312 [10.000,10.625] | 846208.000 [845824.000,846336.000] |
| live-subscriber | 64 | baseline | 9.367 [9.172,9.762] | 8.702 [8.493,8.780] | 11.406 [11.250,11.875] | 1005504.000 [1004160.000,1006848.000] |
| live-subscriber | 64 | candidate | 9.952 [9.319,11.283] | 9.136 [8.675,10.637] | 12.656 [11.562,13.438] | 1005440.000 [1004800.000,1006080.000] |

Fixed historical references below were measured with unrestricted CPUs. They are not denominators for this round.

| Historical engine | Mode | Case | Mean ms/turn | Median ms/turn | CPU ms/turn |
|---|---|---|---:|---:|---:|
| Pi | no-live-subscriber | short-small | 3.625 | 3.135 | 7.188 |
| Pi | no-live-subscriber | short-large | 3.697 | 3.299 | 6.250 |
| Pi | no-live-subscriber | stream | 1717.671 | 1717.671 | 600.000 |
| Pi | no-live-subscriber | large | 87.532 | 87.532 | 110.000 |
| Pi | no-live-subscriber | tool | 101.341 | 101.341 | 130.000 |
| Pi | no-live-subscriber | cancel | 92.503 | 92.503 | 120.000 |
| Pi | live-subscriber | short-small | 3.934 | 3.420 | 7.500 |
| Pi | live-subscriber | short-large | 3.989 | 3.541 | 6.875 |
| Pi | live-subscriber | stream | 1735.929 | 1735.929 | 640.000 |
| Pi | live-subscriber | large | 89.922 | 89.922 | 120.000 |
| Pi | live-subscriber | tool | 102.904 | 102.904 | 130.000 |
| Pi | live-subscriber | cancel | 92.069 | 92.069 | 120.000 |
| DeepSeek harness | live-subscriber | short-small | 15.541 | 14.774 | 18.125 |
| DeepSeek harness | live-subscriber | short-large | 15.450 | 15.336 | 17.500 |
| DeepSeek harness | live-subscriber | stream | 1583.218 | 1583.218 | 300.000 |
| DeepSeek harness | live-subscriber | large | 165.613 | 165.613 | 200.000 |

All adverse fresh paired metrics, including individual reversals:

| Mode | Case | Metric | Paired median % | Adverse pair IDs and % |
|---|---|---|---:|---|
| no-live-subscriber | short-small | mean_turn_ms | -0.510 | 1:+5.561%,2:+1.546% |
| no-live-subscriber | short-small | turn_cpu_ms_per_turn | -1.786 | 5:+3.846% |
| no-live-subscriber | short-small | lifecycle_ms | -0.809 | 1:+4.089% |
| no-live-subscriber | short-small | max_sampled_anon_pss_mib | -0.754 | 4:+0.890% |
| no-live-subscriber | short-small | max_sampled_pss_mib | -0.687 | 4:+0.440% |
| no-live-subscriber | short-small | max_sampled_rss_mib | -0.667 | 4:+0.428% |
| no-live-subscriber | short-small | median_measured_turn_ms | -1.572 | 2:+1.683% |
| no-live-subscriber | short-small | measured_turn_cpu_ms | -1.786 | 5:+3.846% |
| no-live-subscriber | short-small | max_sampled_turn_rss_mib | -0.794 | 4:+0.303% |
| no-live-subscriber | short-small | through_product_exit_wall_ms | -0.922 | 1:+3.908% |
| no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -1.085 | 1:+3.891% |
| no-live-subscriber | short-large | mean_turn_ms | 4.467 | 1:+87.446%,2:+0.227%,3:+10.387%,5:+8.708% |
| no-live-subscriber | short-large | turn_cpu_ms_per_turn | 5.057 | 1:+20.690%,3:+10.000%,4:+3.448%,5:+6.667% |
| no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 0.812 | 1:+8.170%,2:+0.647%,3:+2.922%,4:+0.977% |
| no-live-subscriber | short-large | lifecycle_ms | 1.105 | 1:+85.863%,2:+0.969%,3:+3.110%,4:+1.241%,5:+0.086% |
| no-live-subscriber | short-large | max_sampled_anon_pss_mib | -0.104 | 3:+0.220%,5:+0.230% |
| no-live-subscriber | short-large | max_sampled_pss_mib | -0.210 | 3:+0.078%,5:+0.064% |
| no-live-subscriber | short-large | max_sampled_rss_mib | -0.199 | 3:+0.045%,5:+0.101% |
| no-live-subscriber | short-large | median_measured_turn_ms | 5.109 | 1:+13.852%,2:+1.297%,3:+12.721%,4:+0.254%,5:+8.921% |
| no-live-subscriber | short-large | measured_turn_write_bytes | 0.026 | 1:+0.357%,2:+0.332%,5:+0.089% |
| no-live-subscriber | short-large | turn_write_bytes_per_turn | 0.026 | 1:+0.357%,2:+0.332%,5:+0.089% |
| no-live-subscriber | short-large | lifecycle_self_cpu_ms | 0.812 | 1:+8.170%,2:+0.647%,3:+2.922%,4:+0.977% |
| no-live-subscriber | short-large | measured_turn_cpu_ms | 5.057 | 1:+20.690%,3:+10.000%,4:+3.448%,5:+6.667% |
| no-live-subscriber | short-large | max_sampled_turn_rss_mib | -0.454 | 3:+0.166% |
| no-live-subscriber | short-large | through_product_exit_wall_ms | 1.626 | 1:+80.704%,2:+1.783%,3:+3.007%,4:+1.469%,5:+0.120% |
| no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 1.391 | 1:+76.869%,2:+1.513%,3:+2.659%,4:+1.269% |
| no-live-subscriber | short-large | closed_logical_bytes | 0.174 | 0:+0.742%,1:+0.743%,2:+0.699% |
| no-live-subscriber | short-large | closed_allocated_bytes | 0.196 | 0:+0.739%,1:+0.740%,2:+0.739% |
| no-live-subscriber | stream | mean_turn_ms | -0.240 | 0:+1.966%,1:+0.817% |
| no-live-subscriber | stream | turn_cpu_ms_per_turn | -0.000 | 0:+30.000%,1:+8.333%,2:+20.000% |
| no-live-subscriber | stream | through_cleanup_self_cpu_ms | 2.083 | 0:+13.636%,1:+4.167%,5:+4.348% |
| no-live-subscriber | stream | lifecycle_ms | 0.232 | 0:+1.633%,1:+0.887%,2:+0.062%,3:+0.242%,5:+0.221% |
| no-live-subscriber | stream | max_sampled_anon_pss_mib | -0.115 | 0:+0.445%,3:+0.833% |
| no-live-subscriber | stream | max_sampled_pss_mib | -0.178 | 0:+0.234%,3:+0.209% |
| no-live-subscriber | stream | max_sampled_rss_mib | -0.196 | 0:+0.244%,3:+0.195% |
| no-live-subscriber | stream | median_measured_turn_ms | -0.240 | 0:+1.966%,1:+0.817% |
| no-live-subscriber | stream | lifecycle_self_cpu_ms | 2.083 | 0:+13.636%,1:+4.167%,5:+4.348% |
| no-live-subscriber | stream | measured_turn_cpu_ms | -0.000 | 0:+30.000%,1:+8.333%,2:+20.000% |
| no-live-subscriber | stream | max_sampled_turn_rss_mib | -0.295 | 0:+0.181% |
| no-live-subscriber | stream | through_product_exit_wall_ms | 0.214 | 0:+1.656%,1:+0.996%,2:+0.191%,3:+0.238%,5:+0.101% |
| no-live-subscriber | stream | complete_harness_lifecycle_wall_ms | 0.298 | 0:+1.428%,1:+1.046%,2:+0.189%,3:+0.406%,5:+0.105% |
| no-live-subscriber | large | mean_turn_ms | -0.884 | 5:+7.695% |
| no-live-subscriber | large | through_cleanup_self_cpu_ms | -3.571 | 0:+7.692% |
| no-live-subscriber | large | lifecycle_ms | -1.216 | 2:+0.654%,5:+2.253% |
| no-live-subscriber | large | max_sampled_anon_pss_mib | 0.000 | 0:+0.463%,3:+0.115%,5:+0.071% |
| no-live-subscriber | large | max_sampled_pss_mib | -0.208 | 0:+0.113% |
| no-live-subscriber | large | max_sampled_rss_mib | -0.214 | 0:+0.075% |
| no-live-subscriber | large | median_measured_turn_ms | -0.884 | 5:+7.695% |
| no-live-subscriber | large | lifecycle_self_cpu_ms | -3.571 | 0:+7.692% |
| no-live-subscriber | large | max_sampled_turn_rss_mib | -0.400 | 0:+0.289%,1:+0.102% |
| no-live-subscriber | large | through_product_exit_wall_ms | -1.434 | 2:+0.597%,5:+1.953% |
| no-live-subscriber | large | complete_harness_lifecycle_wall_ms | -1.509 | 2:+0.490%,5:+2.222% |
| no-live-subscriber | tool | mean_turn_ms | -0.995 | 1:+13.025%,2:+1.244%,4:+3.268% |
| no-live-subscriber | tool | turn_cpu_ms_per_turn | 25.000 | 0:+100.000%,1:+100.000%,3:+50.000% |
| no-live-subscriber | tool | through_cleanup_self_cpu_ms | 0.000 | 2:+7.143% |
| no-live-subscriber | tool | lifecycle_ms | 0.220 | 1:+3.680%,2:+4.436%,3:+0.127%,5:+0.312% |
| no-live-subscriber | tool | max_sampled_anon_pss_mib | -0.289 | 1:+0.052% |
| no-live-subscriber | tool | median_measured_turn_ms | -0.995 | 1:+13.025%,2:+1.244%,4:+3.268% |
| no-live-subscriber | tool | lifecycle_self_cpu_ms | 0.000 | 2:+7.143% |
| no-live-subscriber | tool | measured_turn_cpu_ms | 25.000 | 0:+100.000%,1:+100.000%,3:+50.000% |
| no-live-subscriber | tool | max_sampled_turn_rss_mib | -0.370 | 1:+0.038% |
| no-live-subscriber | tool | through_product_exit_wall_ms | -0.237 | 1:+3.438%,2:+3.899%,5:+0.369% |
| no-live-subscriber | tool | complete_harness_lifecycle_wall_ms | 0.056 | 1:+3.045%,2:+3.126%,5:+0.756% |
| no-live-subscriber | cancel | mean_turn_ms | 1.519 | 0:+51.698%,1:+2.304%,3:+14.091%,5:+0.733% |
| no-live-subscriber | cancel | turn_cpu_ms_per_turn | 25.000 | 0:+50.000%,2:+50.000%,3:+50.000% |
| no-live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 0:+7.692% |
| no-live-subscriber | cancel | lifecycle_ms | 0.575 | 0:+62.520%,1:+7.041%,3:+1.266% |
| no-live-subscriber | cancel | max_sampled_anon_pss_mib | -0.490 | 4:+0.340%,5:+0.367% |
| no-live-subscriber | cancel | median_measured_turn_ms | 1.519 | 0:+51.698%,1:+2.304%,3:+14.091%,5:+0.733% |
| no-live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 0:+7.692% |
| no-live-subscriber | cancel | measured_turn_cpu_ms | 25.000 | 0:+50.000%,2:+50.000%,3:+50.000% |
| no-live-subscriber | cancel | through_product_exit_wall_ms | 1.477 | 0:+60.421%,1:+6.829%,3:+3.081% |
| no-live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 1.405 | 0:+59.813%,1:+6.672%,3:+3.498% |
| live-subscriber | short-small | through_cleanup_self_cpu_ms | -1.526 | 4:+4.082% |
| live-subscriber | short-small | lifecycle_ms | -1.429 | 4:+3.700% |
| live-subscriber | short-small | max_sampled_anon_pss_mib | -0.181 | 0:+0.647%,2:+0.289% |
| live-subscriber | short-small | max_sampled_pss_mib | -0.391 | 0:+0.045%,2:+0.102% |
| live-subscriber | short-small | max_sampled_rss_mib | -0.351 | 0:+0.048%,2:+0.080% |
| live-subscriber | short-small | median_measured_turn_ms | -3.139 | 5:+1.137% |
| live-subscriber | short-small | lifecycle_self_cpu_ms | -1.526 | 4:+4.082% |
| live-subscriber | short-small | max_sampled_turn_rss_mib | -0.501 | 0:+0.239% |
| live-subscriber | short-small | through_product_exit_wall_ms | -1.286 | 4:+3.340% |
| live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -1.111 | 4:+1.536% |
| live-subscriber | short-large | mean_turn_ms | 5.992 | 0:+7.627%,2:+22.516%,3:+8.868%,4:+4.356%,5:+1.602% |
| live-subscriber | short-large | turn_cpu_ms_per_turn | 8.001 | 0:+7.895%,1:+8.108%,2:+19.444%,3:+11.111%,4:+7.895%,5:+2.778% |
| live-subscriber | short-large | through_cleanup_self_cpu_ms | 1.358 | 0:+1.511%,1:+1.205%,2:+3.354%,4:+1.829%,5:+0.304% |
| live-subscriber | short-large | lifecycle_ms | 1.315 | 0:+1.811%,1:+1.261%,2:+4.601%,3:+0.698%,4:+1.369%,5:+1.101% |
| live-subscriber | short-large | max_sampled_anon_pss_mib | -0.330 | 2:+0.006%,3:+0.867% |
| live-subscriber | short-large | max_sampled_pss_mib | -0.413 | 3:+0.447% |
| live-subscriber | short-large | max_sampled_rss_mib | -0.420 | 3:+0.439% |
| live-subscriber | short-large | median_measured_turn_ms | 4.987 | 0:+6.987%,1:+3.104%,2:+25.022%,3:+5.859%,4:+4.115%,5:+2.143% |
| live-subscriber | short-large | measured_turn_write_bytes | 0.013 | 0:+0.076%,2:+0.025%,5:+0.076% |
| live-subscriber | short-large | turn_write_bytes_per_turn | 0.013 | 0:+0.076%,2:+0.025%,5:+0.076% |
| live-subscriber | short-large | lifecycle_self_cpu_ms | 1.358 | 0:+1.511%,1:+1.205%,2:+3.354%,4:+1.829%,5:+0.304% |
| live-subscriber | short-large | measured_turn_cpu_ms | 8.001 | 0:+7.895%,1:+8.108%,2:+19.444%,3:+11.111%,4:+7.895%,5:+2.778% |
| live-subscriber | short-large | max_sampled_turn_rss_mib | -0.510 | 2:+0.190%,3:+0.651% |
| live-subscriber | short-large | through_product_exit_wall_ms | 1.295 | 0:+1.530%,1:+1.129%,2:+4.345%,3:+0.727%,4:+1.462%,5:+1.110% |
| live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 1.289 | 0:+1.535%,1:+1.026%,2:+4.220%,3:+0.604%,4:+1.699%,5:+1.042% |
| live-subscriber | short-large | closed_logical_bytes | 0.108 | 0:+0.480%,1:+0.743%,3:+0.742% |
| live-subscriber | short-large | closed_allocated_bytes | 0.109 | 0:+0.478%,1:+0.740%,3:+0.739% |
| live-subscriber | stream | mean_turn_ms | -1.794 | 1:+0.564%,3:+0.504% |
| live-subscriber | stream | turn_cpu_ms_per_turn | -12.500 | 3:+9.091% |
| live-subscriber | stream | lifecycle_ms | -1.231 | 1:+0.787% |
| live-subscriber | stream | max_sampled_anon_pss_mib | -0.084 | 0:+2.114%,2:+1.129%,4:+0.119% |
| live-subscriber | stream | max_sampled_pss_mib | -0.337 | 0:+0.943%,2:+0.392% |
| live-subscriber | stream | max_sampled_rss_mib | -0.330 | 0:+0.969%,2:+0.429% |
| live-subscriber | stream | median_measured_turn_ms | -1.794 | 1:+0.564%,3:+0.504% |
| live-subscriber | stream | measured_turn_cpu_ms | -12.500 | 3:+9.091% |
| live-subscriber | stream | max_sampled_turn_rss_mib | -0.286 | 0:+0.783%,2:+0.265%,4:+0.035% |
| live-subscriber | stream | through_product_exit_wall_ms | -1.269 | 1:+0.765% |
| live-subscriber | stream | complete_harness_lifecycle_wall_ms | -1.259 | 1:+0.756% |
| live-subscriber | large | mean_turn_ms | 0.583 | 0:+1.751%,1:+1.230%,2:+6.185% |
| live-subscriber | large | turn_cpu_ms_per_turn | -12.500 | 1:+33.333%,2:+33.333% |
| live-subscriber | large | through_cleanup_self_cpu_ms | 0.000 | 0:+7.692%,1:+7.692% |
| live-subscriber | large | lifecycle_ms | 1.101 | 0:+0.855%,1:+1.348%,2:+2.643%,4:+1.625% |
| live-subscriber | large | max_sampled_anon_pss_mib | 0.031 | 2:+1.849%,3:+0.375%,4:+0.255% |
| live-subscriber | large | max_sampled_pss_mib | -0.177 | 2:+0.906%,3:+0.018%,4:+0.074% |
| live-subscriber | large | max_sampled_rss_mib | -0.188 | 2:+0.851%,4:+0.032% |
| live-subscriber | large | median_measured_turn_ms | 0.583 | 0:+1.751%,1:+1.230%,2:+6.185% |
| live-subscriber | large | lifecycle_self_cpu_ms | 0.000 | 0:+7.692%,1:+7.692% |
| live-subscriber | large | measured_turn_cpu_ms | -12.500 | 1:+33.333%,2:+33.333% |
| live-subscriber | large | max_sampled_turn_rss_mib | -0.348 | 2:+1.337% |
| live-subscriber | large | through_product_exit_wall_ms | 0.555 | 0:+0.590%,1:+0.520%,2:+3.620%,4:+2.882% |
| live-subscriber | large | complete_harness_lifecycle_wall_ms | 0.805 | 0:+1.010%,1:+0.600%,2:+3.671%,4:+2.902% |
| live-subscriber | tool | mean_turn_ms | -0.183 | 0:+0.834%,1:+3.782%,4:+4.801% |
| live-subscriber | tool | turn_cpu_ms_per_turn | 12.500 | 0:+33.333%,1:+25.000%,4:+150.000% |
| live-subscriber | tool | lifecycle_ms | 1.122 | 1:+0.945%,2:+4.049%,3:+2.053%,4:+1.298% |
| live-subscriber | tool | max_sampled_anon_pss_mib | 0.050 | 0:+0.316%,1:+0.706%,2:+0.052%,4:+0.048% |
| live-subscriber | tool | max_sampled_pss_mib | -0.226 | 0:+0.111%,1:+0.606% |
| live-subscriber | tool | max_sampled_rss_mib | -0.198 | 0:+0.095%,1:+0.594% |
| live-subscriber | tool | median_measured_turn_ms | -0.183 | 0:+0.834%,1:+3.782%,4:+4.801% |
| live-subscriber | tool | measured_turn_cpu_ms | 12.500 | 0:+33.333%,1:+25.000%,4:+150.000% |
| live-subscriber | tool | max_sampled_turn_rss_mib | -0.137 | 1:+0.424% |
| live-subscriber | tool | through_product_exit_wall_ms | 0.982 | 0:+0.257%,1:+0.781%,2:+4.412%,3:+1.801%,4:+1.183% |
| live-subscriber | tool | complete_harness_lifecycle_wall_ms | 0.962 | 0:+0.598%,1:+0.695%,2:+4.041%,3:+1.863%,4:+1.229% |
| live-subscriber | cancel | mean_turn_ms | -0.500 | 3:+1.713%,5:+2.910% |
| live-subscriber | cancel | turn_cpu_ms_per_turn | 50.000 | 1:+50.000%,2:+50.000%,4:+50.000%,5:+50.000% |
| live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 0:+8.333% |
| live-subscriber | cancel | lifecycle_ms | 1.715 | 1:+3.358%,2:+0.156%,3:+5.566%,5:+3.274% |
| live-subscriber | cancel | max_sampled_anon_pss_mib | 0.092 | 0:+0.576%,2:+0.460%,3:+0.581% |
| live-subscriber | cancel | max_sampled_pss_mib | -0.294 | 0:+0.139% |
| live-subscriber | cancel | max_sampled_rss_mib | -0.266 | 0:+0.134%,2:+0.013% |
| live-subscriber | cancel | median_measured_turn_ms | -0.500 | 3:+1.713%,5:+2.910% |
| live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 0:+8.333% |
| live-subscriber | cancel | measured_turn_cpu_ms | 50.000 | 1:+50.000%,2:+50.000%,4:+50.000%,5:+50.000% |
| live-subscriber | cancel | through_product_exit_wall_ms | 2.086 | 1:+3.456%,2:+0.991%,3:+5.047%,4:+0.613%,5:+3.181% |
| live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 2.319 | 1:+3.442%,2:+1.479%,3:+4.844%,4:+1.026%,5:+3.158% |

- Only fresh CPU-2,8 Rust baseline/candidate products are compared. Fixed Pi/DeepSeek measurements used unrestricted CPUs in an earlier batch; their original values are retained separately and no ratio combines those batches.
- Six balanced-order pairs per cell: three baseline-first and three candidate-first. All reversals and unavailable counters remain visible; no confidence interval or parity claim.
- /proc CPU counters use 100 ticks/s: cumulative CPU endpoints have 10 ms granularity, not profiler samples.
- Product measured-turn medians and means are different estimands. Across products report median and range of those statistics; never pool turns.
- DeepSeek SDK idle does not await native flush/fsync. Native wait4 exit/flush and verification CPU remain separate; full process write_bytes is unavailable, not zero.

Full per-product values, raw source hashes, separate DeepSeek exit metrics and unavailable ratios are retained in report-data.json.
