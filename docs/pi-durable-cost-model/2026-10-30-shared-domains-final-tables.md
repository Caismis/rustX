# Shared domain and native content read performance evidence

Only fresh CPU-2,8 Rust baseline/candidate products are compared. Fixed Pi/DeepSeek measurements used unrestricted CPUs in an earlier batch; their original values are retained separately and no ratio combines those batches.

Fresh Rust: six interleaved pairs per cell, CPU affinity 2,8, with three pairs in each execution order.

| Mode | Case | Paired wall median % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -3.113 [-9.029,6.952] | -0.562 | -0.456 | -0.064 | 0.479 |
| no-live-subscriber | short-large | -2.498 [-4.859,4.780] | -1.123 | -0.408 | -0.007 | 0.374 |
| no-live-subscriber | stream | -0.641 [-2.213,0.828] | -8.083 | -0.246 | -0.336 | 0.380 |
| no-live-subscriber | large | 0.347 [-4.836,10.597] | 0.000 | -0.282 | 0.113 | 0.355 |
| no-live-subscriber | tool | -3.027 [-4.557,-0.905] | -6.458 | -0.114 | -0.214 | 0.394 |
| no-live-subscriber | cancel | 0.428 [-3.492,4.737] | 0.000 | -0.145 | 0.022 | 0.468 |
| live-subscriber | short-small | -5.083 [-8.179,1.278] | -1.495 | -0.450 | 0.469 | 0.673 |
| live-subscriber | short-large | -2.322 [-9.617,2.731] | -0.447 | -0.417 | -0.168 | 0.065 |
| live-subscriber | stream | 1.048 [-1.080,12.266] | 4.257 | -0.246 | -0.105 | 0.439 |
| live-subscriber | large | -1.293 [-34.242,10.555] | -2.778 | -0.282 | 0.002 | 0.632 |
| live-subscriber | tool | -3.803 [-10.256,15.733] | 0.000 | -0.114 | -0.481 | 0.108 |
| live-subscriber | cancel | -2.539 [-6.091,10.783] | 3.846 | -0.145 | 0.009 | 0.460 |

Fresh Rust absolute warm-request windows, each product contributing 32 measured turns:

| Mode | History | Revision | Mean ms/turn median [min,max] | Median ms/turn median [min,max] | CPU ms/turn | Write bytes/turn |
|---|---:|---|---:|---:|---:|---:|
| no-live-subscriber | 8 | baseline | 7.636 [7.583,8.242] | 7.173 [7.074,7.553] | 8.750 [8.438,9.375] | 853760.000 [853248.000,853888.000] |
| no-live-subscriber | 8 | candidate | 7.561 [7.305,8.111] | 6.918 [6.819,7.761] | 8.281 [8.125,9.062] | 850112.000 [850048.000,850560.000] |
| no-live-subscriber | 64 | baseline | 8.540 [8.445,8.602] | 7.779 [7.721,7.866] | 9.375 [9.062,9.688] | 1009216.000 [1008256.000,1010432.000] |
| no-live-subscriber | 64 | candidate | 8.339 [8.212,8.891] | 7.580 [7.430,8.199] | 9.375 [8.750,10.000] | 1005952.000 [1004160.000,1006208.000] |
| live-subscriber | 8 | baseline | 8.729 [8.451,34.338] | 8.287 [8.062,8.842] | 10.938 [10.625,13.438] | 853760.000 [853504.000,854272.000] |
| live-subscriber | 8 | candidate | 8.470 [8.255,34.655] | 7.896 [7.748,8.511] | 10.625 [10.312,14.062] | 850304.000 [849920.000,850560.000] |
| live-subscriber | 64 | baseline | 9.696 [9.630,10.231] | 8.995 [8.834,9.524] | 12.188 [11.875,13.125] | 1008896.000 [1008640.000,1009792.000] |
| live-subscriber | 64 | candidate | 9.460 [9.313,19.104] | 8.699 [8.566,9.123] | 11.875 [11.562,13.438] | 1005184.000 [1003648.000,1005824.000] |

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
| no-live-subscriber | short-small | mean_turn_ms | -2.376 | 2:+6.410%,4:+0.547% |
| no-live-subscriber | short-small | turn_cpu_ms_per_turn | -5.423 | 2:+3.571% |
| no-live-subscriber | short-small | through_cleanup_self_cpu_ms | -0.562 | 2:+2.222%,5:+2.273% |
| no-live-subscriber | short-small | lifecycle_ms | -0.062 | 2:+4.914%,4:+1.144%,5:+2.572% |
| no-live-subscriber | short-small | max_sampled_anon_pss_mib | -0.064 | 0:+0.443%,2:+0.389%,3:+0.180% |
| no-live-subscriber | short-small | max_sampled_pss_mib | 0.507 | 0:+1.033%,2:+0.502%,3:+0.674%,4:+0.511% |
| no-live-subscriber | short-small | max_sampled_rss_mib | 0.479 | 0:+0.982%,2:+0.496%,3:+0.716%,4:+0.463%,5:+0.028% |
| no-live-subscriber | short-small | median_measured_turn_ms | -3.113 | 2:+6.952% |
| no-live-subscriber | short-small | lifecycle_self_cpu_ms | -0.562 | 2:+2.222%,5:+2.273% |
| no-live-subscriber | short-small | measured_turn_cpu_ms | -5.423 | 2:+3.571% |
| no-live-subscriber | short-small | max_sampled_turn_rss_mib | 0.405 | 0:+1.138%,2:+0.630%,3:+0.960%,4:+0.088%,5:+0.181% |
| no-live-subscriber | short-small | through_product_exit_wall_ms | -0.216 | 2:+4.884%,4:+0.982%,5:+2.813% |
| no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 0.335 | 2:+4.565%,4:+1.720%,5:+3.165% |
| no-live-subscriber | short-large | mean_turn_ms | -2.161 | 0:+3.919% |
| no-live-subscriber | short-large | turn_cpu_ms_per_turn | 0.000 | 0:+6.667%,3:+3.333% |
| no-live-subscriber | short-large | through_cleanup_self_cpu_ms | -1.123 | 0:+3.583% |
| no-live-subscriber | short-large | lifecycle_ms | -1.151 | 0:+3.258%,3:+0.583% |
| no-live-subscriber | short-large | max_sampled_anon_pss_mib | -0.007 | 1:+1.055%,4:+0.215%,5:+0.087% |
| no-live-subscriber | short-large | max_sampled_pss_mib | 0.376 | 0:+0.160%,1:+1.136%,2:+0.371%,4:+0.381%,5:+0.448% |
| no-live-subscriber | short-large | max_sampled_rss_mib | 0.374 | 0:+0.194%,1:+1.126%,2:+0.374%,4:+0.374%,5:+0.435% |
| no-live-subscriber | short-large | median_measured_turn_ms | -2.498 | 0:+4.780% |
| no-live-subscriber | short-large | lifecycle_self_cpu_ms | -1.123 | 0:+3.583% |
| no-live-subscriber | short-large | measured_turn_cpu_ms | 0.000 | 0:+6.667%,3:+3.333% |
| no-live-subscriber | short-large | max_sampled_turn_rss_mib | 0.497 | 0:+0.452%,1:+1.804%,2:+0.373%,4:+0.814%,5:+0.543% |
| no-live-subscriber | short-large | through_product_exit_wall_ms | -0.900 | 0:+3.069%,3:+0.537% |
| no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | -0.771 | 0:+2.852%,3:+0.145% |
| no-live-subscriber | stream | mean_turn_ms | -0.641 | 2:+0.053%,5:+0.828% |
| no-live-subscriber | stream | turn_cpu_ms_per_turn | -4.167 | 2:+8.333%,5:+18.182% |
| no-live-subscriber | stream | lifecycle_ms | -0.693 | 2:+0.436%,4:+0.023% |
| no-live-subscriber | stream | max_sampled_anon_pss_mib | -0.336 | 0:+0.062%,5:+0.512% |
| no-live-subscriber | stream | max_sampled_pss_mib | 0.395 | 0:+0.068%,2:+0.541%,3:+0.498%,4:+0.292%,5:+0.559% |
| no-live-subscriber | stream | max_sampled_rss_mib | 0.380 | 0:+0.099%,2:+0.520%,3:+0.475%,4:+0.285%,5:+0.508% |
| no-live-subscriber | stream | median_measured_turn_ms | -0.641 | 2:+0.053%,5:+0.828% |
| no-live-subscriber | stream | measured_turn_cpu_ms | -4.167 | 2:+8.333%,5:+18.182% |
| no-live-subscriber | stream | max_sampled_turn_rss_mib | 0.223 | 0:+0.129%,2:+0.583%,3:+0.245%,4:+0.201%,5:+0.603% |
| no-live-subscriber | stream | through_product_exit_wall_ms | -0.692 | 2:+0.289%,4:+0.074% |
| no-live-subscriber | stream | complete_harness_lifecycle_wall_ms | -0.687 | 2:+0.255%,4:+0.104% |
| no-live-subscriber | large | mean_turn_ms | 0.347 | 0:+2.137%,1:+0.794%,4:+10.597% |
| no-live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 3:+50.000% |
| no-live-subscriber | large | through_cleanup_self_cpu_ms | 0.000 | 3:+7.692%,4:+7.692% |
| no-live-subscriber | large | lifecycle_ms | 1.912 | 1:+2.743%,2:+1.082%,3:+2.875%,4:+3.139% |
| no-live-subscriber | large | max_sampled_anon_pss_mib | 0.113 | 0:+0.177%,3:+0.053%,4:+0.462%,5:+0.173% |
| no-live-subscriber | large | max_sampled_pss_mib | 0.392 | 0:+0.226%,1:+0.548%,3:+0.236%,4:+0.619%,5:+0.570% |
| no-live-subscriber | large | max_sampled_rss_mib | 0.355 | 0:+0.246%,1:+0.463%,3:+0.232%,4:+0.644%,5:+0.519% |
| no-live-subscriber | large | median_measured_turn_ms | 0.347 | 0:+2.137%,1:+0.794%,4:+10.597% |
| no-live-subscriber | large | lifecycle_self_cpu_ms | 0.000 | 3:+7.692%,4:+7.692% |
| no-live-subscriber | large | measured_turn_cpu_ms | 0.000 | 3:+50.000% |
| no-live-subscriber | large | max_sampled_turn_rss_mib | 0.255 | 0:+0.593%,1:+0.433%,3:+0.047%,4:+0.679%,5:+0.077% |
| no-live-subscriber | large | through_product_exit_wall_ms | 1.871 | 1:+3.588%,2:+1.140%,3:+2.602%,4:+3.217% |
| no-live-subscriber | large | complete_harness_lifecycle_wall_ms | 1.879 | 1:+4.001%,2:+0.959%,3:+2.799%,4:+3.515% |
| no-live-subscriber | tool | lifecycle_ms | -1.454 | 4:+5.122% |
| no-live-subscriber | tool | max_sampled_anon_pss_mib | -0.214 | 2:+0.201%,3:+1.644% |
| no-live-subscriber | tool | max_sampled_pss_mib | 0.411 | 0:+0.335%,1:+0.166%,2:+0.795%,3:+1.685%,5:+0.487% |
| no-live-subscriber | tool | max_sampled_rss_mib | 0.394 | 0:+0.341%,1:+0.158%,2:+0.794%,3:+1.657%,5:+0.446% |
| no-live-subscriber | tool | max_sampled_turn_rss_mib | 0.486 | 0:+0.558%,1:+0.049%,2:+0.838%,3:+1.994%,5:+0.414% |
| no-live-subscriber | tool | through_product_exit_wall_ms | -1.385 | 4:+8.286% |
| no-live-subscriber | tool | complete_harness_lifecycle_wall_ms | -1.394 | 4:+8.347% |
| no-live-subscriber | cancel | mean_turn_ms | 0.428 | 1:+0.861%,4:+3.136%,5:+4.737% |
| no-live-subscriber | cancel | turn_cpu_ms_per_turn | 0.000 | 2:+100.000% |
| no-live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 4:+7.692% |
| no-live-subscriber | cancel | lifecycle_ms | 0.226 | 1:+7.036%,3:+2.361%,5:+8.296% |
| no-live-subscriber | cancel | max_sampled_anon_pss_mib | 0.022 | 0:+0.273%,3:+0.179%,5:+0.094% |
| no-live-subscriber | cancel | max_sampled_pss_mib | 0.449 | 0:+0.679%,1:+0.476%,2:+0.421%,3:+0.667%,4:+0.216%,5:+0.263% |
| no-live-subscriber | cancel | max_sampled_rss_mib | 0.468 | 0:+0.624%,1:+0.472%,2:+0.464%,3:+0.692%,4:+0.221%,5:+0.315% |
| no-live-subscriber | cancel | median_measured_turn_ms | 0.428 | 1:+0.861%,4:+3.136%,5:+4.737% |
| no-live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 4:+7.692% |
| no-live-subscriber | cancel | measured_turn_cpu_ms | 0.000 | 2:+100.000% |
| no-live-subscriber | cancel | max_sampled_turn_rss_mib | 0.379 | 0:+0.617%,1:+0.367%,2:+0.392%,3:+0.836%,4:+0.268%,5:+0.366% |
| no-live-subscriber | cancel | through_product_exit_wall_ms | 0.227 | 1:+7.039%,3:+1.513%,5:+7.918% |
| no-live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 0.102 | 1:+6.744%,3:+0.937%,5:+7.504% |
| live-subscriber | short-small | mean_turn_ms | -4.517 | 1:+0.923%,2:+3.211% |
| live-subscriber | short-small | turn_cpu_ms_per_turn | -1.429 | 1:+4.651%,2:+5.882% |
| live-subscriber | short-small | through_cleanup_self_cpu_ms | -1.495 | 1:+1.818%,2:+2.020% |
| live-subscriber | short-small | lifecycle_ms | -1.586 | 2:+3.593% |
| live-subscriber | short-small | max_sampled_anon_pss_mib | 0.469 | 0:+0.549%,1:+0.708%,2:+0.340%,3:+0.973%,4:+0.389% |
| live-subscriber | short-small | max_sampled_pss_mib | 0.663 | 0:+0.767%,1:+0.651%,2:+0.601%,3:+1.075%,4:+0.674%,5:+0.159% |
| live-subscriber | short-small | max_sampled_rss_mib | 0.673 | 0:+0.708%,1:+0.679%,2:+0.631%,3:+1.062%,4:+0.668%,5:+0.131% |
| live-subscriber | short-small | median_measured_turn_ms | -5.083 | 2:+1.278% |
| live-subscriber | short-small | lifecycle_self_cpu_ms | -1.495 | 1:+1.818%,2:+2.020% |
| live-subscriber | short-small | measured_turn_cpu_ms | -1.429 | 1:+4.651%,2:+5.882% |
| live-subscriber | short-small | max_sampled_turn_rss_mib | 0.853 | 0:+0.715%,1:+0.839%,2:+0.866%,3:+1.514%,4:+1.157% |
| live-subscriber | short-small | through_product_exit_wall_ms | -1.438 | 2:+3.218% |
| live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -2.340 | 2:+2.663% |
| live-subscriber | short-large | mean_turn_ms | -2.233 | 1:+97.575%,5:+4.821% |
| live-subscriber | short-large | turn_cpu_ms_per_turn | -2.532 | 1:+10.256% |
| live-subscriber | short-large | through_cleanup_self_cpu_ms | -0.447 | 1:+8.060% |
| live-subscriber | short-large | lifecycle_ms | 1.170 | 0:+2.035%,1:+124.672%,3:+1.671%,5:+0.668% |
| live-subscriber | short-large | max_sampled_anon_pss_mib | -0.168 | 1:+0.037%,5:+0.368% |
| live-subscriber | short-large | max_sampled_pss_mib | 0.072 | 1:+0.258%,3:+0.163%,5:+0.544% |
| live-subscriber | short-large | max_sampled_rss_mib | 0.065 | 1:+0.272%,3:+0.167%,5:+0.548% |
| live-subscriber | short-large | median_measured_turn_ms | -2.322 | 1:+2.731% |
| live-subscriber | short-large | lifecycle_self_cpu_ms | -0.447 | 1:+8.060% |
| live-subscriber | short-large | measured_turn_cpu_ms | -2.532 | 1:+10.256% |
| live-subscriber | short-large | max_sampled_turn_rss_mib | 0.074 | 1:+0.467%,3:+0.170%,5:+0.854% |
| live-subscriber | short-large | through_product_exit_wall_ms | 1.333 | 0:+2.475%,1:+117.493%,2:+0.130%,3:+2.068%,5:+0.598% |
| live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 1.233 | 0:+2.296%,1:+111.997%,2:+0.184%,3:+1.934%,5:+0.532% |
| live-subscriber | stream | mean_turn_ms | 1.048 | 0:+12.266%,1:+0.708%,2:+1.399%,3:+0.326%,4:+1.387% |
| live-subscriber | stream | turn_cpu_ms_per_turn | 0.000 | 1:+8.333%,2:+20.000% |
| live-subscriber | stream | through_cleanup_self_cpu_ms | 4.257 | 1:+4.348%,2:+13.636%,3:+4.167%,4:+9.091% |
| live-subscriber | stream | lifecycle_ms | 1.250 | 0:+16.950%,1:+0.764%,2:+2.430%,3:+0.956%,4:+1.545% |
| live-subscriber | stream | max_sampled_anon_pss_mib | -0.105 | 0:+0.614%,1:+0.204%,3:+0.379% |
| live-subscriber | stream | max_sampled_pss_mib | 0.424 | 0:+1.060%,1:+0.721%,2:+0.369%,3:+0.479% |
| live-subscriber | stream | max_sampled_rss_mib | 0.439 | 0:+1.031%,1:+0.710%,2:+0.366%,3:+0.512% |
| live-subscriber | stream | median_measured_turn_ms | 1.048 | 0:+12.266%,1:+0.708%,2:+1.399%,3:+0.326%,4:+1.387% |
| live-subscriber | stream | lifecycle_self_cpu_ms | 4.257 | 1:+4.348%,2:+13.636%,3:+4.167%,4:+9.091% |
| live-subscriber | stream | measured_turn_cpu_ms | 0.000 | 1:+8.333%,2:+20.000% |
| live-subscriber | stream | max_sampled_turn_rss_mib | 0.564 | 0:+0.936%,1:+0.618%,2:+0.735%,3:+0.511%,5:+0.163% |
| live-subscriber | stream | through_product_exit_wall_ms | 1.157 | 0:+16.817%,1:+0.770%,2:+2.478%,3:+0.927%,4:+1.387% |
| live-subscriber | stream | complete_harness_lifecycle_wall_ms | 1.196 | 0:+16.323%,1:+0.775%,2:+2.419%,3:+1.043%,4:+1.349% |
| live-subscriber | large | mean_turn_ms | -1.293 | 4:+10.555%,5:+0.949% |
| live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 5:+33.333% |
| live-subscriber | large | through_cleanup_self_cpu_ms | -2.778 | 3:+7.692% |
| live-subscriber | large | lifecycle_ms | -0.198 | 1:+0.462%,3:+3.458%,5:+7.129% |
| live-subscriber | large | max_sampled_anon_pss_mib | 0.002 | 0:+2.076%,2:+0.383%,4:+1.442% |
| live-subscriber | large | max_sampled_pss_mib | 0.682 | 0:+2.002%,1:+0.361%,2:+1.003%,4:+1.213%,5:+0.099% |
| live-subscriber | large | max_sampled_rss_mib | 0.632 | 0:+1.972%,1:+0.286%,2:+0.978%,4:+1.194%,5:+0.061% |
| live-subscriber | large | median_measured_turn_ms | -1.293 | 4:+10.555%,5:+0.949% |
| live-subscriber | large | lifecycle_self_cpu_ms | -2.778 | 3:+7.692% |
| live-subscriber | large | measured_turn_cpu_ms | 0.000 | 5:+33.333% |
| live-subscriber | large | max_sampled_turn_rss_mib | 0.675 | 0:+2.114%,1:+0.400%,2:+0.951%,4:+1.222%,5:+0.008% |
| live-subscriber | large | through_product_exit_wall_ms | 0.355 | 1:+0.245%,2:+0.465%,3:+3.522%,5:+6.818% |
| live-subscriber | large | complete_harness_lifecycle_wall_ms | 0.336 | 1:+0.272%,2:+0.400%,3:+3.428%,5:+6.206% |
| live-subscriber | tool | mean_turn_ms | -3.803 | 1:+15.733% |
| live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 4:+33.333%,5:+25.000% |
| live-subscriber | tool | lifecycle_ms | -1.718 | 1:+5.448% |
| live-subscriber | tool | max_sampled_anon_pss_mib | -0.481 | 0:+0.394%,4:+0.210% |
| live-subscriber | tool | max_sampled_pss_mib | 0.081 | 0:+0.871%,1:+0.122%,3:+0.039%,4:+0.541% |
| live-subscriber | tool | max_sampled_rss_mib | 0.108 | 0:+0.824%,1:+0.084%,3:+0.131%,4:+0.486% |
| live-subscriber | tool | median_measured_turn_ms | -3.803 | 1:+15.733% |
| live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 4:+33.333%,5:+25.000% |
| live-subscriber | tool | max_sampled_turn_rss_mib | 0.135 | 0:+1.148%,1:+0.046%,2:+0.264%,4:+0.224% |
| live-subscriber | tool | through_product_exit_wall_ms | -2.161 | 1:+5.116% |
| live-subscriber | tool | complete_harness_lifecycle_wall_ms | -2.111 | 1:+4.796% |
| live-subscriber | cancel | mean_turn_ms | -2.539 | 1:+10.783%,4:+0.810% |
| live-subscriber | cancel | turn_cpu_ms_per_turn | 0.000 | 2:+50.000%,5:+50.000% |
| live-subscriber | cancel | through_cleanup_self_cpu_ms | 3.846 | 1:+7.692%,3:+7.692%,5:+7.692% |
| live-subscriber | cancel | lifecycle_ms | 0.536 | 0:+0.398%,1:+1.197%,3:+0.348%,4:+0.675%,5:+1.245% |
| live-subscriber | cancel | max_sampled_anon_pss_mib | 0.009 | 0:+0.782%,2:+1.498%,3:+0.089% |
| live-subscriber | cancel | max_sampled_pss_mib | 0.493 | 0:+0.815%,1:+0.642%,2:+1.514%,3:+0.345% |
| live-subscriber | cancel | max_sampled_rss_mib | 0.460 | 0:+0.800%,1:+0.632%,2:+1.413%,3:+0.288% |
| live-subscriber | cancel | median_measured_turn_ms | -2.539 | 1:+10.783%,4:+0.810% |
| live-subscriber | cancel | lifecycle_self_cpu_ms | 3.846 | 1:+7.692%,3:+7.692%,5:+7.692% |
| live-subscriber | cancel | measured_turn_cpu_ms | 0.000 | 2:+50.000%,5:+50.000% |
| live-subscriber | cancel | max_sampled_turn_rss_mib | 0.444 | 0:+0.417%,1:+1.036%,2:+1.675%,3:+0.471% |
| live-subscriber | cancel | through_product_exit_wall_ms | 0.277 | 0:+0.406%,1:+0.900%,4:+0.147%,5:+1.225% |
| live-subscriber | cancel | complete_harness_lifecycle_wall_ms | -0.021 | 1:+0.590%,5:+1.364% |

- Only fresh CPU-2,8 Rust baseline/candidate products are compared. Fixed Pi/DeepSeek measurements used unrestricted CPUs in an earlier batch; their original values are retained separately and no ratio combines those batches.
- Six balanced-order pairs per cell: three baseline-first and three candidate-first. All reversals and unavailable counters remain visible; no confidence interval or parity claim.
- /proc CPU counters use 100 ticks/s: cumulative CPU endpoints have 10 ms granularity, not profiler samples.
- Product measured-turn medians and means are different estimands. Across products report median and range of those statistics; never pool turns.
- DeepSeek SDK idle does not await native flush/fsync. Native wait4 exit/flush and verification CPU remain separate; full process write_bytes is unavailable, not zero.

Full per-product values, raw source hashes, separate DeepSeek exit metrics and unavailable ratios are retained in report-data.json.
