# Compact content keys and SQL tail append performance evidence

Only fresh CPU-2,8 Rust baseline/candidate products are compared. Fixed Pi/DeepSeek measurements used unrestricted CPUs in an earlier batch; their original values are retained separately and no ratio combines those batches.

Fresh Rust: six interleaved pairs per cell, CPU affinity 2,8, with three pairs in each execution order.

| Mode | Case | Paired wall median % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -1.299 [-8.952,5.174] | 0.575 | -0.599 | 0.154 | 0.106 |
| no-live-subscriber | short-large | -1.999 [-8.541,0.046] | -0.159 | -0.852 | -0.378 | -0.236 |
| no-live-subscriber | stream | 0.823 [0.520,2.285] | 6.719 | 0.000 | 0.042 | -0.007 |
| no-live-subscriber | large | 1.245 [-4.728,8.959] | 0.000 | 0.000 | -0.276 | 0.584 |
| no-live-subscriber | tool | 1.780 [-1.526,5.059] | 0.000 | -0.455 | -0.338 | -0.022 |
| no-live-subscriber | cancel | -0.077 [-3.309,7.159] | 0.000 | 0.000 | 0.315 | 0.250 |
| live-subscriber | short-small | -4.229 [-6.132,1.019] | 0.000 | -0.611 | -0.072 | 0.370 |
| live-subscriber | short-large | 3.277 [-4.137,7.413] | 1.233 | -0.856 | 0.034 | 0.365 |
| live-subscriber | stream | -0.989 [-1.968,0.041] | 0.000 | 0.000 | -0.312 | 0.273 |
| live-subscriber | large | 0.127 [-2.253,5.161] | 0.000 | 0.000 | -0.011 | 0.345 |
| live-subscriber | tool | 0.497 [-1.539,13.867] | 0.000 | 0.000 | 0.297 | 0.622 |
| live-subscriber | cancel | 3.230 [-6.676,7.818] | 0.000 | 0.000 | 0.084 | 0.377 |

Fresh Rust absolute warm-request windows, each product contributing 32 measured turns:

| Mode | History | Revision | Mean ms/turn median [min,max] | Median ms/turn median [min,max] | CPU ms/turn | Write bytes/turn |
|---|---:|---|---:|---:|---:|---:|
| no-live-subscriber | 8 | baseline | 7.314 [7.257,7.891] | 6.756 [6.668,7.333] | 8.125 [7.812,8.750] | 846208.000 [846080.000,846336.000] |
| no-live-subscriber | 8 | candidate | 7.285 [7.202,7.592] | 6.712 [6.615,7.013] | 8.125 [7.812,8.750] | 841024.000 [840576.000,841216.000] |
| no-live-subscriber | 64 | baseline | 8.253 [8.147,9.042] | 7.552 [7.437,8.102] | 9.219 [9.062,10.312] | 1004864.000 [1003648.000,1006848.000] |
| no-live-subscriber | 64 | candidate | 8.201 [8.138,8.549] | 7.406 [7.369,7.620] | 9.062 [9.062,9.062] | 990784.000 [989184.000,991360.000] |
| live-subscriber | 8 | baseline | 8.438 [8.095,8.949] | 7.875 [7.605,8.235] | 10.312 [10.000,11.250] | 846144.000 [846080.000,846336.000] |
| live-subscriber | 8 | candidate | 8.120 [8.080,8.483] | 7.662 [7.457,7.833] | 10.000 [9.688,10.312] | 840960.000 [840320.000,841216.000] |
| live-subscriber | 64 | baseline | 9.148 [8.993,9.451] | 8.444 [8.398,8.798] | 11.250 [10.938,11.250] | 1004800.000 [1004416.000,1006976.000] |
| live-subscriber | 64 | candidate | 9.473 [8.897,9.868] | 8.705 [8.342,9.041] | 11.562 [11.250,12.500] | 990720.000 [988672.000,991744.000] |

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
| no-live-subscriber | short-small | mean_turn_ms | -0.376 | 0:+0.425%,1:+4.289%,5:+0.330% |
| no-live-subscriber | short-small | turn_cpu_ms_per_turn | 0.000 | 1:+7.692% |
| no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 0.575 | 0:+1.149%,1:+3.409%,2:+1.149% |
| no-live-subscriber | short-small | lifecycle_ms | 0.304 | 1:+2.869%,2:+0.401%,4:+0.208%,5:+0.434% |
| no-live-subscriber | short-small | max_sampled_anon_pss_mib | 0.154 | 0:+0.344%,2:+0.095%,3:+0.214%,4:+0.057%,5:+0.528% |
| no-live-subscriber | short-small | max_sampled_pss_mib | 0.107 | 0:+0.616%,2:+0.408%,5:+0.216% |
| no-live-subscriber | short-small | max_sampled_rss_mib | 0.106 | 0:+0.634%,2:+0.412%,5:+0.212% |
| no-live-subscriber | short-small | median_measured_turn_ms | -1.299 | 1:+5.174%,5:+0.714% |
| no-live-subscriber | short-small | lifecycle_self_cpu_ms | 0.575 | 0:+1.149%,1:+3.409%,2:+1.149% |
| no-live-subscriber | short-small | measured_turn_cpu_ms | 0.000 | 1:+7.692% |
| no-live-subscriber | short-small | max_sampled_turn_rss_mib | 0.194 | 0:+1.013%,2:+0.887%,4:+0.082%,5:+0.305% |
| no-live-subscriber | short-small | through_product_exit_wall_ms | 0.348 | 1:+2.662%,2:+0.207%,4:+0.489%,5:+0.535% |
| no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -0.103 | 1:+3.227%,4:+0.117%,5:+0.546% |
| no-live-subscriber | short-large | mean_turn_ms | -0.627 | 2:+4.032% |
| no-live-subscriber | short-large | through_cleanup_self_cpu_ms | -0.159 | 2:+0.660%,3:+0.656%,5:+2.980% |
| no-live-subscriber | short-large | lifecycle_ms | 0.146 | 2:+0.791%,3:+0.559%,5:+2.806% |
| no-live-subscriber | short-large | max_sampled_anon_pss_mib | -0.378 | 3:+0.052% |
| no-live-subscriber | short-large | max_sampled_pss_mib | -0.250 | 0:+0.054%,3:+0.084% |
| no-live-subscriber | short-large | max_sampled_rss_mib | -0.236 | 0:+0.060%,3:+0.075% |
| no-live-subscriber | short-large | median_measured_turn_ms | -1.999 | 5:+0.046% |
| no-live-subscriber | short-large | lifecycle_self_cpu_ms | -0.159 | 2:+0.660%,3:+0.656%,5:+2.980% |
| no-live-subscriber | short-large | through_product_exit_wall_ms | 0.397 | 2:+0.673%,3:+0.878%,4:+0.121%,5:+2.661% |
| no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 0.448 | 2:+0.940%,3:+0.742%,4:+0.155%,5:+2.546% |
| no-live-subscriber | short-large | closed_logical_bytes | 0.506 | 2:+0.571%,3:+0.616%,4:+0.484%,5:+0.528% |
| no-live-subscriber | short-large | closed_allocated_bytes | 0.480 | 2:+0.568%,3:+0.612%,4:+0.480%,5:+0.480% |
| no-live-subscriber | stream | mean_turn_ms | 0.823 | 0:+0.520%,1:+1.819%,2:+2.285%,3:+0.589%,4:+0.788%,5:+0.858% |
| no-live-subscriber | stream | turn_cpu_ms_per_turn | 13.846 | 0:+7.692%,1:+33.333%,2:+30.000%,4:+20.000% |
| no-live-subscriber | stream | through_cleanup_self_cpu_ms | 6.719 | 1:+14.286%,2:+9.091%,3:+4.348%,4:+14.286% |
| no-live-subscriber | stream | lifecycle_ms | 1.329 | 0:+0.089%,1:+1.765%,2:+2.067%,3:+0.961%,4:+1.698%,5:+0.211% |
| no-live-subscriber | stream | max_sampled_anon_pss_mib | 0.042 | 0:+0.120%,2:+0.342%,5:+2.215% |
| no-live-subscriber | stream | max_sampled_pss_mib | -0.046 | 2:+0.288%,5:+1.823% |
| no-live-subscriber | stream | max_sampled_rss_mib | -0.007 | 0:+0.013%,2:+0.284%,5:+1.804% |
| no-live-subscriber | stream | median_measured_turn_ms | 0.823 | 0:+0.520%,1:+1.819%,2:+2.285%,3:+0.589%,4:+0.788%,5:+0.858% |
| no-live-subscriber | stream | lifecycle_self_cpu_ms | 6.719 | 1:+14.286%,2:+9.091%,3:+4.348%,4:+14.286% |
| no-live-subscriber | stream | measured_turn_cpu_ms | 13.846 | 0:+7.692%,1:+33.333%,2:+30.000%,4:+20.000% |
| no-live-subscriber | stream | max_sampled_turn_rss_mib | -0.216 | 2:+0.105%,5:+1.613% |
| no-live-subscriber | stream | through_product_exit_wall_ms | 1.398 | 0:+0.190%,1:+1.850%,2:+2.180%,3:+0.968%,4:+1.828%,5:+0.143% |
| no-live-subscriber | stream | complete_harness_lifecycle_wall_ms | 1.464 | 0:+0.007%,1:+1.869%,2:+2.172%,3:+1.114%,4:+1.815%,5:+0.146% |
| no-live-subscriber | large | mean_turn_ms | 1.245 | 1:+8.953%,3:+8.959%,4:+3.102% |
| no-live-subscriber | large | through_cleanup_self_cpu_ms | 0.000 | 3:+7.692% |
| no-live-subscriber | large | lifecycle_ms | 0.707 | 1:+1.032%,3:+3.854%,4:+0.382%,5:+1.049% |
| no-live-subscriber | large | max_sampled_anon_pss_mib | -0.276 | 0:+0.538%,2:+0.253% |
| no-live-subscriber | large | max_sampled_pss_mib | 0.563 | 0:+0.803%,1:+0.459%,2:+0.978%,3:+0.667% |
| no-live-subscriber | large | max_sampled_rss_mib | 0.584 | 0:+0.785%,1:+0.449%,2:+0.922%,3:+0.720% |
| no-live-subscriber | large | median_measured_turn_ms | 1.245 | 1:+8.953%,3:+8.959%,4:+3.102% |
| no-live-subscriber | large | lifecycle_self_cpu_ms | 0.000 | 3:+7.692% |
| no-live-subscriber | large | max_sampled_turn_rss_mib | 0.552 | 0:+0.647%,1:+0.457%,2:+0.986%,3:+0.868% |
| no-live-subscriber | large | through_product_exit_wall_ms | 0.593 | 1:+1.575%,3:+3.776%,4:+0.205%,5:+0.981% |
| no-live-subscriber | large | complete_harness_lifecycle_wall_ms | 0.872 | 1:+2.023%,3:+3.230%,4:+0.450%,5:+1.294% |
| no-live-subscriber | tool | mean_turn_ms | 1.780 | 1:+1.417%,3:+2.828%,4:+2.142%,5:+5.059% |
| no-live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 4:+100.000% |
| no-live-subscriber | tool | through_cleanup_self_cpu_ms | 0.000 | 0:+7.143%,2:+7.143% |
| no-live-subscriber | tool | lifecycle_ms | 1.322 | 0:+4.986%,2:+2.151%,3:+1.406%,4:+0.272%,5:+1.237% |
| no-live-subscriber | tool | max_sampled_anon_pss_mib | -0.338 | 3:+0.018% |
| no-live-subscriber | tool | max_sampled_pss_mib | -0.043 | 0:+0.103%,3:+0.259%,5:+0.141% |
| no-live-subscriber | tool | max_sampled_rss_mib | -0.022 | 0:+0.140%,3:+0.255%,5:+0.152% |
| no-live-subscriber | tool | median_measured_turn_ms | 1.780 | 1:+1.417%,3:+2.828%,4:+2.142%,5:+5.059% |
| no-live-subscriber | tool | lifecycle_self_cpu_ms | 0.000 | 0:+7.143%,2:+7.143% |
| no-live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 4:+100.000% |
| no-live-subscriber | tool | max_sampled_turn_rss_mib | 0.128 | 0:+0.171%,2:+0.111%,4:+0.144%,5:+0.737% |
| no-live-subscriber | tool | through_product_exit_wall_ms | 1.589 | 0:+5.413%,2:+2.270%,3:+0.611%,4:+1.308%,5:+1.870% |
| no-live-subscriber | tool | complete_harness_lifecycle_wall_ms | 1.640 | 0:+6.444%,2:+2.412%,3:+0.036%,4:+1.166%,5:+2.113% |
| no-live-subscriber | cancel | mean_turn_ms | -0.077 | 2:+0.502%,4:+3.052%,5:+7.159% |
| no-live-subscriber | cancel | lifecycle_ms | 1.194 | 0:+5.800%,2:+1.378%,3:+1.605%,5:+1.010% |
| no-live-subscriber | cancel | max_sampled_anon_pss_mib | 0.315 | 0:+0.394%,1:+0.390%,2:+0.375%,3:+0.256% |
| no-live-subscriber | cancel | max_sampled_pss_mib | 0.216 | 0:+0.672%,1:+0.363%,2:+0.003%,3:+0.501%,4:+0.068% |
| no-live-subscriber | cancel | max_sampled_rss_mib | 0.250 | 0:+0.606%,1:+0.432%,2:+0.003%,3:+0.531%,4:+0.067% |
| no-live-subscriber | cancel | median_measured_turn_ms | -0.077 | 2:+0.502%,4:+3.052%,5:+7.159% |
| no-live-subscriber | cancel | max_sampled_turn_rss_mib | 0.331 | 0:+0.803%,1:+0.426%,3:+0.500%,4:+0.237% |
| no-live-subscriber | cancel | through_product_exit_wall_ms | 1.123 | 0:+5.706%,2:+1.111%,3:+1.611%,5:+1.136% |
| no-live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 0.862 | 0:+5.396%,2:+0.661%,3:+1.668%,5:+1.063% |
| live-subscriber | short-small | mean_turn_ms | -3.282 | 0:+2.147%,5:+0.622% |
| live-subscriber | short-small | through_cleanup_self_cpu_ms | 0.000 | 3:+1.042% |
| live-subscriber | short-small | lifecycle_ms | -1.022 | 0:+0.469%,5:+0.720% |
| live-subscriber | short-small | max_sampled_anon_pss_mib | -0.072 | 3:+0.132%,4:+0.575%,5:+0.111% |
| live-subscriber | short-small | max_sampled_pss_mib | 0.354 | 1:+0.261%,3:+0.772%,4:+0.496%,5:+0.447% |
| live-subscriber | short-small | max_sampled_rss_mib | 0.370 | 1:+0.262%,3:+0.775%,4:+0.498%,5:+0.477% |
| live-subscriber | short-small | median_measured_turn_ms | -4.229 | 5:+1.019% |
| live-subscriber | short-small | lifecycle_self_cpu_ms | 0.000 | 3:+1.042% |
| live-subscriber | short-small | max_sampled_turn_rss_mib | 0.400 | 1:+0.222%,2:+0.211%,3:+0.579%,4:+0.674%,5:+0.732% |
| live-subscriber | short-small | through_product_exit_wall_ms | -0.472 | 0:+0.682%,5:+0.780% |
| live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -0.762 | 5:+1.109% |
| live-subscriber | short-large | mean_turn_ms | 2.395 | 1:+8.536%,3:+0.871%,4:+3.918%,5:+4.658% |
| live-subscriber | short-large | turn_cpu_ms_per_turn | 4.206 | 1:+11.111%,2:+2.857%,3:+5.556%,4:+11.111%,5:+2.857% |
| live-subscriber | short-large | through_cleanup_self_cpu_ms | 1.233 | 0:+1.231%,1:+3.704%,2:+1.235%,3:+0.613%,4:+2.154% |
| live-subscriber | short-large | lifecycle_ms | 1.373 | 0:+1.170%,1:+3.832%,2:+1.575%,3:+0.356%,4:+2.237%,5:+0.291% |
| live-subscriber | short-large | max_sampled_anon_pss_mib | 0.034 | 0:+0.186%,3:+0.512%,5:+0.155% |
| live-subscriber | short-large | max_sampled_pss_mib | 0.367 | 0:+0.594%,3:+0.971%,4:+0.450%,5:+0.283% |
| live-subscriber | short-large | max_sampled_rss_mib | 0.365 | 0:+0.592%,3:+0.984%,4:+0.451%,5:+0.280% |
| live-subscriber | short-large | median_measured_turn_ms | 3.277 | 1:+7.413%,3:+3.328%,4:+3.770%,5:+3.227% |
| live-subscriber | short-large | lifecycle_self_cpu_ms | 1.233 | 0:+1.231%,1:+3.704%,2:+1.235%,3:+0.613%,4:+2.154% |
| live-subscriber | short-large | measured_turn_cpu_ms | 4.206 | 1:+11.111%,2:+2.857%,3:+5.556%,4:+11.111%,5:+2.857% |
| live-subscriber | short-large | max_sampled_turn_rss_mib | 0.532 | 0:+0.693%,3:+1.511%,4:+0.632%,5:+0.432% |
| live-subscriber | short-large | through_product_exit_wall_ms | 1.568 | 0:+1.632%,1:+3.673%,2:+1.505%,3:+0.247%,4:+2.082% |
| live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 1.296 | 0:+1.470%,1:+3.313%,2:+1.121%,4:+2.244% |
| live-subscriber | short-large | closed_logical_bytes | 0.418 | 0:+0.440%,1:+0.748%,2:+0.396%,3:+0.661%,4:+0.396% |
| live-subscriber | short-large | closed_allocated_bytes | 0.371 | 0:+0.393%,1:+0.743%,2:+0.349%,3:+0.612%,4:+0.349% |
| live-subscriber | stream | mean_turn_ms | -0.989 | 1:+0.041% |
| live-subscriber | stream | turn_cpu_ms_per_turn | -4.167 | 0:+9.091%,5:+9.091% |
| live-subscriber | stream | through_cleanup_self_cpu_ms | 0.000 | 0:+4.545% |
| live-subscriber | stream | through_cleanup_process_write_mib | 0.000 | 0:+0.123% |
| live-subscriber | stream | lifecycle_ms | -0.448 | 0:+0.192%,1:+0.121% |
| live-subscriber | stream | max_sampled_anon_pss_mib | -0.312 | 4:+2.255% |
| live-subscriber | stream | max_sampled_pss_mib | 0.252 | 0:+0.035%,1:+0.215%,2:+0.290%,3:+0.381%,4:+2.028% |
| live-subscriber | stream | max_sampled_rss_mib | 0.273 | 0:+0.037%,1:+0.253%,2:+0.293%,3:+0.407%,4:+1.997% |
| live-subscriber | stream | median_measured_turn_ms | -0.989 | 1:+0.041% |
| live-subscriber | stream | through_cleanup_process_write_bytes | 0.000 | 0:+0.123% |
| live-subscriber | stream | lifecycle_self_cpu_ms | 0.000 | 0:+4.545% |
| live-subscriber | stream | measured_turn_cpu_ms | -4.167 | 0:+9.091%,5:+9.091% |
| live-subscriber | stream | max_sampled_turn_rss_mib | 0.407 | 0:+0.437%,1:+0.755%,2:+0.150%,3:+0.377%,4:+2.256% |
| live-subscriber | stream | through_product_exit_wall_ms | -0.492 | 0:+0.312%,1:+0.048% |
| live-subscriber | stream | complete_harness_lifecycle_wall_ms | -0.390 | 0:+0.214%,1:+0.046%,3:+0.024% |
| live-subscriber | large | mean_turn_ms | 0.127 | 0:+0.398%,1:+0.725%,2:+5.161% |
| live-subscriber | large | turn_cpu_ms_per_turn | 25.000 | 0:+100.000%,2:+50.000%,5:+50.000% |
| live-subscriber | large | through_cleanup_self_cpu_ms | 0.000 | 4:+7.692%,5:+7.692% |
| live-subscriber | large | lifecycle_ms | 0.457 | 0:+2.392%,1:+1.825%,5:+1.073% |
| live-subscriber | large | max_sampled_anon_pss_mib | -0.011 | 1:+0.092%,3:+0.430%,4:+0.357% |
| live-subscriber | large | max_sampled_pss_mib | 0.330 | 0:+0.308%,1:+0.525%,2:+0.451%,4:+0.353%,5:+0.267% |
| live-subscriber | large | max_sampled_rss_mib | 0.345 | 0:+0.342%,1:+0.570%,2:+0.408%,4:+0.348%,5:+0.268% |
| live-subscriber | large | median_measured_turn_ms | 0.127 | 0:+0.398%,1:+0.725%,2:+5.161% |
| live-subscriber | large | lifecycle_self_cpu_ms | 0.000 | 4:+7.692%,5:+7.692% |
| live-subscriber | large | measured_turn_cpu_ms | 25.000 | 0:+100.000%,2:+50.000%,5:+50.000% |
| live-subscriber | large | max_sampled_turn_rss_mib | 0.422 | 0:+0.090%,1:+0.968%,2:+0.500%,4:+0.534%,5:+0.344% |
| live-subscriber | large | through_product_exit_wall_ms | 0.948 | 0:+1.569%,1:+1.519%,4:+0.377%,5:+2.142% |
| live-subscriber | large | complete_harness_lifecycle_wall_ms | 0.806 | 0:+1.149%,1:+1.235%,4:+0.464%,5:+1.519% |
| live-subscriber | tool | mean_turn_ms | 0.497 | 1:+10.534%,2:+0.408%,4:+13.867%,5:+0.586% |
| live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 2:+33.333%,5:+33.333% |
| live-subscriber | tool | through_cleanup_self_cpu_ms | 0.000 | 4:+6.667%,5:+7.143% |
| live-subscriber | tool | lifecycle_ms | 0.490 | 1:+2.106%,4:+7.154%,5:+1.240% |
| live-subscriber | tool | max_sampled_anon_pss_mib | 0.297 | 1:+0.013%,2:+0.581%,3:+1.884%,4:+1.881% |
| live-subscriber | tool | max_sampled_pss_mib | 0.579 | 1:+0.814%,2:+0.345%,3:+1.613%,4:+1.649%,5:+0.341% |
| live-subscriber | tool | max_sampled_rss_mib | 0.622 | 1:+0.845%,2:+0.289%,3:+1.589%,4:+1.588%,5:+0.398% |
| live-subscriber | tool | median_measured_turn_ms | 0.497 | 1:+10.534%,2:+0.408%,4:+13.867%,5:+0.586% |
| live-subscriber | tool | lifecycle_self_cpu_ms | 0.000 | 4:+6.667%,5:+7.143% |
| live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 2:+33.333%,5:+33.333% |
| live-subscriber | tool | max_sampled_turn_rss_mib | 0.708 | 1:+0.757%,2:+0.659%,3:+1.571%,4:+1.479%,5:+0.483% |
| live-subscriber | tool | through_product_exit_wall_ms | 0.907 | 1:+2.783%,2:+0.651%,4:+6.352%,5:+1.162% |
| live-subscriber | tool | complete_harness_lifecycle_wall_ms | 0.641 | 1:+2.956%,2:+0.432%,4:+6.219%,5:+0.850% |
| live-subscriber | cancel | mean_turn_ms | 3.230 | 0:+0.803%,1:+4.426%,2:+4.715%,3:+2.034%,4:+7.818% |
| live-subscriber | cancel | turn_cpu_ms_per_turn | 25.000 | 1:+50.000%,3:+50.000%,4:+50.000% |
| live-subscriber | cancel | lifecycle_ms | 0.400 | 1:+0.065%,2:+0.735%,3:+2.917%,4:+1.890% |
| live-subscriber | cancel | max_sampled_anon_pss_mib | 0.084 | 0:+0.249%,1:+2.254%,2:+2.339% |
| live-subscriber | cancel | max_sampled_pss_mib | 0.403 | 0:+0.014%,1:+2.208%,2:+1.427%,3:+0.472%,4:+0.063%,5:+0.333% |
| live-subscriber | cancel | max_sampled_rss_mib | 0.377 | 0:+0.016%,1:+2.126%,2:+1.411%,3:+0.471%,4:+0.097%,5:+0.284% |
| live-subscriber | cancel | median_measured_turn_ms | 3.230 | 0:+0.803%,1:+4.426%,2:+4.715%,3:+2.034%,4:+7.818% |
| live-subscriber | cancel | measured_turn_cpu_ms | 25.000 | 1:+50.000%,3:+50.000%,4:+50.000% |
| live-subscriber | cancel | max_sampled_turn_rss_mib | 0.422 | 1:+2.239%,2:+1.232%,3:+0.580%,4:+0.264%,5:+0.224% |
| live-subscriber | cancel | through_product_exit_wall_ms | 0.147 | 1:+0.560%,3:+2.933%,4:+2.186% |
| live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 0.323 | 1:+1.021%,3:+2.703%,4:+2.434% |

- Only fresh CPU-2,8 Rust baseline/candidate products are compared. Fixed Pi/DeepSeek measurements used unrestricted CPUs in an earlier batch; their original values are retained separately and no ratio combines those batches.
- Six balanced-order pairs per cell: three baseline-first and three candidate-first. All reversals and unavailable counters remain visible; no confidence interval or parity claim.
- /proc CPU counters use 100 ticks/s: cumulative CPU endpoints have 10 ms granularity, not profiler samples.
- Product measured-turn medians and means are different estimands. Across products report median and range of those statistics; never pool turns.
- DeepSeek SDK idle does not await native flush/fsync. Native wait4 exit/flush and verification CPU remain separate; full process write_bytes is unavailable, not zero.

Full per-product values, raw source hashes, separate DeepSeek exit metrics and unavailable ratios are retained in report-data.json.
