# Shared cut metadata and indexed merge performance evidence

Only fresh CPU-2,8 Rust baseline/candidate products are compared. Fixed Pi/DeepSeek measurements used unrestricted CPUs in an earlier batch; their original values are retained separately and no ratio combines those batches.

Fresh Rust: six interleaved pairs per cell, CPU affinity 2,8, with three pairs in each execution order.

| Mode | Case | Paired wall median % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -0.006 [-7.220,1.505] | 0.000 | -0.006 | -0.255 | -0.261 |
| no-live-subscriber | short-large | -0.053 [-2.664,9.044] | 0.979 | 0.018 | -0.783 | -0.578 |
| no-live-subscriber | stream | 0.036 [-0.669,2.325] | 0.000 | 0.000 | 0.031 | -0.055 |
| no-live-subscriber | large | -2.539 [-11.811,-1.501] | 0.000 | 0.000 | 0.106 | 0.163 |
| no-live-subscriber | tool | -2.968 [-7.260,5.230] | 0.000 | 0.000 | 0.329 | -0.282 |
| no-live-subscriber | cancel | -1.833 [-6.038,0.451] | 0.000 | 0.000 | -0.176 | -0.233 |
| live-subscriber | short-small | -4.681 [-17.163,-2.497] | -2.476 | -0.017 | -0.164 | -0.103 |
| live-subscriber | short-large | -1.731 [-12.669,7.963] | 0.753 | 0.036 | -0.457 | -0.414 |
| live-subscriber | stream | 1.317 [-1.229,2.635] | 4.026 | 0.000 | -0.376 | -0.299 |
| live-subscriber | large | 0.604 [-7.748,7.617] | 3.571 | 0.000 | 0.454 | -0.227 |
| live-subscriber | tool | -0.455 [-4.118,7.464] | 0.000 | 0.000 | 0.253 | 0.212 |
| live-subscriber | cancel | -3.970 [-7.755,5.180] | 0.000 | 0.000 | -0.877 | -0.593 |

Fresh Rust absolute warm-request windows, each product contributing 32 measured turns:

| Mode | History | Revision | Mean ms/turn median [min,max] | Median ms/turn median [min,max] | CPU ms/turn | Write bytes/turn |
|---|---:|---|---:|---:|---:|---:|
| no-live-subscriber | 8 | baseline | 7.350 [7.253,7.834] | 6.812 [6.744,7.424] | 8.281 [8.125,9.062] | 846272.000 [845824.000,846592.000] |
| no-live-subscriber | 8 | candidate | 7.357 [7.273,7.385] | 6.845 [6.718,6.888] | 8.125 [8.125,8.125] | 846208.000 [846080.000,846208.000] |
| no-live-subscriber | 64 | baseline | 8.299 [8.211,8.435] | 7.560 [7.455,7.734] | 9.062 [9.062,9.375] | 1005056.000 [1004416.000,1005696.000] |
| no-live-subscriber | 64 | candidate | 8.277 [8.169,9.218] | 7.558 [7.438,8.259] | 9.219 [9.062,10.625] | 1005760.000 [1003904.000,1008000.000] |
| live-subscriber | 8 | baseline | 8.760 [8.134,9.494] | 8.165 [7.801,9.293] | 10.781 [10.000,11.250] | 846144.000 [845824.000,846592.000] |
| live-subscriber | 8 | candidate | 8.135 [8.083,8.366] | 7.693 [7.569,7.873] | 10.156 [10.000,10.312] | 846080.000 [845696.000,846336.000] |
| live-subscriber | 64 | baseline | 9.737 [9.195,18.447] | 8.848 [8.614,9.890] | 12.188 [11.875,13.125] | 1004864.000 [1003520.000,1005696.000] |
| live-subscriber | 64 | candidate | 9.644 [9.289,10.200] | 8.758 [8.637,9.300] | 11.875 [11.562,12.500] | 1006336.000 [1005056.000,1008000.000] |

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
| no-live-subscriber | short-small | mean_turn_ms | -0.014 | 2:+0.137%,3:+1.708%,5:+0.180% |
| no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 0.000 | 2:+2.273% |
| no-live-subscriber | short-small | through_cleanup_process_write_mib | -0.006 | 3:+0.035%,4:+0.023% |
| no-live-subscriber | short-small | lifecycle_ms | -0.771 | 2:+1.773%,3:+0.279% |
| no-live-subscriber | short-small | max_sampled_anon_pss_mib | -0.255 | 2:+0.981%,3:+1.353% |
| no-live-subscriber | short-small | max_sampled_pss_mib | -0.234 | 2:+0.974%,3:+0.634% |
| no-live-subscriber | short-small | max_sampled_rss_mib | -0.261 | 2:+0.975%,3:+0.617% |
| no-live-subscriber | short-small | median_measured_turn_ms | -0.006 | 0:+0.654%,3:+1.505%,5:+0.528% |
| no-live-subscriber | short-small | measured_turn_write_bytes | -0.008 | 3:+0.045%,4:+0.030% |
| no-live-subscriber | short-small | turn_write_bytes_per_turn | -0.008 | 3:+0.045%,4:+0.030% |
| no-live-subscriber | short-small | through_cleanup_process_write_bytes | -0.006 | 3:+0.035%,4:+0.023% |
| no-live-subscriber | short-small | lifecycle_self_cpu_ms | 0.000 | 2:+2.273% |
| no-live-subscriber | short-small | max_sampled_turn_rss_mib | -0.095 | 2:+0.992%,3:+0.765%,4:+0.005% |
| no-live-subscriber | short-small | through_product_exit_wall_ms | -0.520 | 2:+1.433%,3:+0.138% |
| no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -1.387 | 2:+0.598%,3:+0.724% |
| no-live-subscriber | short-small | closed_logical_bytes | 0.000 | 2:+0.061% |
| no-live-subscriber | short-small | closed_allocated_bytes | 0.000 | 2:+0.060% |
| no-live-subscriber | short-large | mean_turn_ms | 0.199 | 0:+0.202%,1:+0.195%,4:+3.578%,5:+10.065% |
| no-live-subscriber | short-large | turn_cpu_ms_per_turn | 1.724 | 1:+3.448%,4:+3.448%,5:+17.241% |
| no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 0.979 | 1:+0.977%,3:+1.618%,4:+0.980%,5:+2.951% |
| no-live-subscriber | short-large | through_cleanup_process_write_mib | 0.018 | 1:+0.126%,3:+0.036%,4:+0.135% |
| no-live-subscriber | short-large | lifecycle_ms | 1.059 | 1:+0.902%,3:+1.570%,4:+1.216%,5:+3.178% |
| no-live-subscriber | short-large | max_sampled_anon_pss_mib | -0.783 | 3:+0.172%,5:+0.128% |
| no-live-subscriber | short-large | max_sampled_pss_mib | -0.570 | 5:+0.353% |
| no-live-subscriber | short-large | max_sampled_rss_mib | -0.578 | 5:+0.353% |
| no-live-subscriber | short-large | median_measured_turn_ms | -0.053 | 1:+0.802%,4:+1.764%,5:+9.044% |
| no-live-subscriber | short-large | measured_turn_write_bytes | 0.070 | 0:+0.025%,2:+0.115%,4:+0.229%,5:+0.293% |
| no-live-subscriber | short-large | turn_write_bytes_per_turn | 0.070 | 0:+0.025%,2:+0.115%,4:+0.229%,5:+0.293% |
| no-live-subscriber | short-large | through_cleanup_process_write_bytes | 0.018 | 1:+0.126%,3:+0.036%,4:+0.135% |
| no-live-subscriber | short-large | lifecycle_self_cpu_ms | 0.979 | 1:+0.977%,3:+1.618%,4:+0.980%,5:+2.951% |
| no-live-subscriber | short-large | measured_turn_cpu_ms | 1.724 | 1:+3.448%,4:+3.448%,5:+17.241% |
| no-live-subscriber | short-large | max_sampled_turn_rss_mib | -0.596 | 3:+0.043%,5:+0.663% |
| no-live-subscriber | short-large | through_product_exit_wall_ms | 1.206 | 1:+1.045%,3:+1.389%,4:+1.367%,5:+3.388% |
| no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 1.131 | 1:+1.028%,2:+0.013%,3:+1.235%,4:+1.341%,5:+3.250% |
| no-live-subscriber | short-large | closed_logical_bytes | 0.088 | 2:+1.142%,3:+0.176%,5:+1.186% |
| no-live-subscriber | short-large | closed_allocated_bytes | 0.087 | 2:+1.135%,3:+0.175%,5:+1.222% |
| no-live-subscriber | stream | mean_turn_ms | 0.036 | 0:+2.325%,1:+0.073%,2:+1.501% |
| no-live-subscriber | stream | turn_cpu_ms_per_turn | 4.167 | 0:+30.000%,2:+18.182%,5:+8.333% |
| no-live-subscriber | stream | through_cleanup_self_cpu_ms | 0.000 | 0:+8.696%,2:+19.048% |
| no-live-subscriber | stream | lifecycle_ms | -0.168 | 0:+1.970%,2:+2.125% |
| no-live-subscriber | stream | max_sampled_anon_pss_mib | 0.031 | 2:+0.254%,4:+0.071%,5:+0.102% |
| no-live-subscriber | stream | max_sampled_pss_mib | -0.045 | 1:+0.067%,3:+0.236%,4:+0.238% |
| no-live-subscriber | stream | max_sampled_rss_mib | -0.055 | 1:+0.059%,3:+0.278%,4:+0.220% |
| no-live-subscriber | stream | median_measured_turn_ms | 0.036 | 0:+2.325%,1:+0.073%,2:+1.501% |
| no-live-subscriber | stream | lifecycle_self_cpu_ms | 0.000 | 0:+8.696%,2:+19.048% |
| no-live-subscriber | stream | measured_turn_cpu_ms | 4.167 | 0:+30.000%,2:+18.182%,5:+8.333% |
| no-live-subscriber | stream | max_sampled_turn_rss_mib | 0.060 | 2:+0.215%,3:+0.143%,4:+0.375% |
| no-live-subscriber | stream | through_product_exit_wall_ms | -0.129 | 0:+1.764%,2:+2.176% |
| no-live-subscriber | stream | complete_harness_lifecycle_wall_ms | -0.058 | 0:+1.600%,2:+2.161% |
| no-live-subscriber | large | turn_cpu_ms_per_turn | 33.333 | 1:+33.333%,2:+33.333%,3:+50.000%,4:+33.333% |
| no-live-subscriber | large | through_cleanup_self_cpu_ms | 0.000 | 1:+7.692% |
| no-live-subscriber | large | lifecycle_ms | -0.939 | 0:+1.478%,2:+0.288% |
| no-live-subscriber | large | max_sampled_anon_pss_mib | 0.106 | 0:+0.124%,2:+0.088%,3:+0.345%,4:+0.452% |
| no-live-subscriber | large | max_sampled_pss_mib | 0.164 | 1:+0.009%,3:+0.319%,4:+0.692%,5:+0.409% |
| no-live-subscriber | large | max_sampled_rss_mib | 0.163 | 1:+0.051%,3:+0.275%,4:+0.718%,5:+0.359% |
| no-live-subscriber | large | lifecycle_self_cpu_ms | 0.000 | 1:+7.692% |
| no-live-subscriber | large | measured_turn_cpu_ms | 33.333 | 1:+33.333%,2:+33.333%,3:+50.000%,4:+33.333% |
| no-live-subscriber | large | max_sampled_turn_rss_mib | 0.339 | 1:+0.600%,3:+0.544%,4:+0.809%,5:+0.135% |
| no-live-subscriber | large | through_product_exit_wall_ms | -0.861 | 0:+2.464%,2:+0.335% |
| no-live-subscriber | large | complete_harness_lifecycle_wall_ms | -0.755 | 0:+2.435%,2:+0.268% |
| no-live-subscriber | tool | mean_turn_ms | -2.968 | 1:+5.230% |
| no-live-subscriber | tool | turn_cpu_ms_per_turn | -12.500 | 4:+50.000%,5:+100.000% |
| no-live-subscriber | tool | through_cleanup_self_cpu_ms | 0.000 | 1:+6.667%,5:+7.143% |
| no-live-subscriber | tool | lifecycle_ms | 0.395 | 0:+0.238%,1:+3.703%,2:+0.552%,5:+0.972% |
| no-live-subscriber | tool | max_sampled_anon_pss_mib | 0.329 | 0:+0.123%,1:+0.487%,2:+1.124%,3:+0.171%,4:+1.251% |
| no-live-subscriber | tool | max_sampled_pss_mib | -0.272 | 2:+0.530%,4:+0.480% |
| no-live-subscriber | tool | max_sampled_rss_mib | -0.282 | 2:+0.468%,4:+0.441% |
| no-live-subscriber | tool | median_measured_turn_ms | -2.968 | 1:+5.230% |
| no-live-subscriber | tool | lifecycle_self_cpu_ms | 0.000 | 1:+6.667%,5:+7.143% |
| no-live-subscriber | tool | measured_turn_cpu_ms | -12.500 | 4:+50.000%,5:+100.000% |
| no-live-subscriber | tool | max_sampled_turn_rss_mib | -0.101 | 0:+0.014%,2:+0.661%,4:+0.543% |
| no-live-subscriber | tool | through_product_exit_wall_ms | 0.368 | 0:+0.400%,1:+3.455%,2:+1.207%,5:+0.337% |
| no-live-subscriber | tool | complete_harness_lifecycle_wall_ms | 0.772 | 0:+1.015%,1:+3.796%,2:+1.090%,5:+0.529% |
| no-live-subscriber | cancel | mean_turn_ms | -1.833 | 1:+0.451% |
| no-live-subscriber | cancel | lifecycle_ms | -0.827 | 2:+1.036% |
| no-live-subscriber | cancel | max_sampled_anon_pss_mib | -0.176 | 2:+0.152%,4:+0.192% |
| no-live-subscriber | cancel | max_sampled_pss_mib | -0.227 | 3:+0.155% |
| no-live-subscriber | cancel | max_sampled_rss_mib | -0.233 | 3:+0.143% |
| no-live-subscriber | cancel | median_measured_turn_ms | -1.833 | 1:+0.451% |
| no-live-subscriber | cancel | max_sampled_turn_rss_mib | -0.354 | 3:+0.357%,5:+0.160% |
| no-live-subscriber | cancel | through_product_exit_wall_ms | -0.875 | 2:+1.283% |
| no-live-subscriber | cancel | complete_harness_lifecycle_wall_ms | -1.068 | 2:+1.213% |
| live-subscriber | short-small | through_cleanup_self_cpu_ms | -2.476 | 4:+2.105% |
| live-subscriber | short-small | through_cleanup_process_write_mib | -0.017 | 4:+0.047% |
| live-subscriber | short-small | lifecycle_ms | -3.804 | 4:+1.810% |
| live-subscriber | short-small | max_sampled_anon_pss_mib | -0.164 | 3:+0.049%,4:+0.172% |
| live-subscriber | short-small | max_sampled_pss_mib | -0.127 | 1:+0.034%,3:+0.258% |
| live-subscriber | short-small | max_sampled_rss_mib | -0.103 | 1:+0.029%,3:+0.290% |
| live-subscriber | short-small | measured_turn_write_bytes | -0.023 | 4:+0.061% |
| live-subscriber | short-small | turn_write_bytes_per_turn | -0.023 | 4:+0.061% |
| live-subscriber | short-small | through_cleanup_process_write_bytes | -0.017 | 4:+0.047% |
| live-subscriber | short-small | lifecycle_self_cpu_ms | -2.476 | 4:+2.105% |
| live-subscriber | short-small | max_sampled_turn_rss_mib | -0.116 | 3:+0.138% |
| live-subscriber | short-small | through_product_exit_wall_ms | -3.509 | 4:+1.626% |
| live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -3.925 | 4:+1.122% |
| live-subscriber | short-small | closed_logical_bytes | 0.061 | 0:+0.061%,2:+0.061%,4:+0.061%,5:+0.061% |
| live-subscriber | short-small | closed_allocated_bytes | 0.060 | 0:+0.060%,2:+0.060%,4:+0.060%,5:+0.060% |
| live-subscriber | short-large | mean_turn_ms | -0.984 | 0:+5.332%,2:+3.604%,4:+1.659% |
| live-subscriber | short-large | through_cleanup_self_cpu_ms | 0.753 | 0:+2.134%,1:+0.901%,2:+0.606%,3:+0.302%,4:+1.824% |
| live-subscriber | short-large | through_cleanup_process_write_mib | 0.036 | 1:+0.086%,2:+0.144%,4:+0.023%,5:+0.050% |
| live-subscriber | short-large | lifecycle_ms | 0.097 | 0:+2.833%,2:+0.603%,4:+2.926% |
| live-subscriber | short-large | median_measured_turn_ms | -1.731 | 1:+7.963%,2:+2.005% |
| live-subscriber | short-large | measured_turn_write_bytes | 0.172 | 1:+0.178%,2:+0.318%,3:+0.280%,4:+0.064%,5:+0.166% |
| live-subscriber | short-large | turn_write_bytes_per_turn | 0.172 | 1:+0.178%,2:+0.318%,3:+0.280%,4:+0.064%,5:+0.166% |
| live-subscriber | short-large | through_cleanup_process_write_bytes | 0.036 | 1:+0.086%,2:+0.144%,4:+0.023%,5:+0.050% |
| live-subscriber | short-large | lifecycle_self_cpu_ms | 0.753 | 0:+2.134%,1:+0.901%,2:+0.606%,3:+0.302%,4:+1.824% |
| live-subscriber | short-large | through_product_exit_wall_ms | 0.353 | 0:+2.897%,2:+0.588%,3:+0.117%,4:+2.316% |
| live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 0.332 | 0:+2.331%,2:+0.650%,3:+0.015%,4:+2.188% |
| live-subscriber | short-large | closed_logical_bytes | 0.022 | 1:+0.044%,2:+0.261%,3:+1.319% |
| live-subscriber | short-large | closed_allocated_bytes | 0.044 | 1:+0.087%,2:+0.259%,3:+1.311% |
| live-subscriber | stream | mean_turn_ms | 1.317 | 2:+1.115%,3:+2.353%,4:+2.635%,5:+1.519% |
| live-subscriber | stream | turn_cpu_ms_per_turn | 19.091 | 2:+20.000%,3:+20.000%,4:+18.182%,5:+27.273% |
| live-subscriber | stream | through_cleanup_self_cpu_ms | 4.026 | 1:+3.704%,3:+23.810%,4:+4.348%,5:+13.636% |
| live-subscriber | stream | lifecycle_ms | 1.636 | 1:+5.701%,2:+0.326%,3:+2.915%,4:+1.362%,5:+1.911% |
| live-subscriber | stream | max_sampled_anon_pss_mib | -0.376 | 2:+0.129%,5:+0.053% |
| live-subscriber | stream | max_sampled_pss_mib | -0.323 | 5:+0.271% |
| live-subscriber | stream | max_sampled_rss_mib | -0.299 | 5:+0.279% |
| live-subscriber | stream | median_measured_turn_ms | 1.317 | 2:+1.115%,3:+2.353%,4:+2.635%,5:+1.519% |
| live-subscriber | stream | lifecycle_self_cpu_ms | 4.026 | 1:+3.704%,3:+23.810%,4:+4.348%,5:+13.636% |
| live-subscriber | stream | measured_turn_cpu_ms | 19.091 | 2:+20.000%,3:+20.000%,4:+18.182%,5:+27.273% |
| live-subscriber | stream | max_sampled_turn_rss_mib | -0.080 | 0:+0.086%,3:+0.074%,5:+0.564% |
| live-subscriber | stream | through_product_exit_wall_ms | 1.652 | 1:+5.678%,2:+0.391%,3:+2.842%,4:+1.384%,5:+1.921% |
| live-subscriber | stream | complete_harness_lifecycle_wall_ms | 1.615 | 1:+5.808%,2:+0.462%,3:+2.910%,4:+1.340%,5:+1.890% |
| live-subscriber | large | mean_turn_ms | 0.604 | 0:+6.741%,3:+7.617%,4:+5.091% |
| live-subscriber | large | through_cleanup_self_cpu_ms | 3.571 | 0:+7.692%,1:+7.143%,3:+15.385% |
| live-subscriber | large | lifecycle_ms | -0.497 | 0:+0.492%,1:+95.862%,3:+3.807% |
| live-subscriber | large | max_sampled_anon_pss_mib | 0.454 | 0:+1.990%,3:+0.948%,4:+2.462% |
| live-subscriber | large | max_sampled_pss_mib | -0.220 | 0:+1.449%,3:+0.076%,4:+1.698% |
| live-subscriber | large | max_sampled_rss_mib | -0.227 | 0:+1.361%,3:+0.045%,4:+1.674% |
| live-subscriber | large | median_measured_turn_ms | 0.604 | 0:+6.741%,3:+7.617%,4:+5.091% |
| live-subscriber | large | lifecycle_self_cpu_ms | 3.571 | 0:+7.692%,1:+7.143%,3:+15.385% |
| live-subscriber | large | max_sampled_turn_rss_mib | -0.334 | 0:+2.416%,4:+1.455% |
| live-subscriber | large | through_product_exit_wall_ms | -0.063 | 0:+0.034%,1:+93.221%,3:+4.364% |
| live-subscriber | large | complete_harness_lifecycle_wall_ms | 0.906 | 1:+89.376%,3:+4.455%,5:+1.999% |
| live-subscriber | tool | mean_turn_ms | -0.455 | 4:+7.464%,5:+2.637% |
| live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 2:+33.333% |
| live-subscriber | tool | through_cleanup_self_cpu_ms | 0.000 | 1:+6.667%,4:+7.143% |
| live-subscriber | tool | lifecycle_ms | 1.678 | 0:+2.731%,1:+5.242%,2:+1.021%,4:+2.336% |
| live-subscriber | tool | max_sampled_anon_pss_mib | 0.253 | 1:+0.375%,2:+0.430%,3:+0.271%,4:+0.236% |
| live-subscriber | tool | max_sampled_pss_mib | 0.198 | 1:+0.124%,2:+0.412%,3:+0.271%,4:+0.463% |
| live-subscriber | tool | max_sampled_rss_mib | 0.212 | 1:+0.144%,2:+0.408%,3:+0.281%,4:+0.455% |
| live-subscriber | tool | median_measured_turn_ms | -0.455 | 4:+7.464%,5:+2.637% |
| live-subscriber | tool | lifecycle_self_cpu_ms | 0.000 | 1:+6.667%,4:+7.143% |
| live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 2:+33.333% |
| live-subscriber | tool | max_sampled_turn_rss_mib | 0.277 | 1:+0.404%,2:+0.286%,3:+0.267%,4:+0.565% |
| live-subscriber | tool | through_product_exit_wall_ms | 1.029 | 0:+1.748%,1:+5.556%,2:+0.229%,4:+2.409%,5:+0.310% |
| live-subscriber | tool | complete_harness_lifecycle_wall_ms | 1.078 | 0:+1.599%,1:+5.477%,4:+2.352%,5:+0.557% |
| live-subscriber | cancel | mean_turn_ms | -3.970 | 0:+2.785%,1:+5.180% |
| live-subscriber | cancel | turn_cpu_ms_per_turn | 0.000 | 3:+50.000%,4:+50.000% |
| live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 4:+7.692% |
| live-subscriber | cancel | lifecycle_ms | 0.437 | 0:+1.482%,4:+1.843%,5:+2.448% |
| live-subscriber | cancel | max_sampled_anon_pss_mib | -0.877 | 1:+0.120% |
| live-subscriber | cancel | max_sampled_pss_mib | -0.589 | 0:+0.024% |
| live-subscriber | cancel | max_sampled_rss_mib | -0.593 | 0:+0.021% |
| live-subscriber | cancel | median_measured_turn_ms | -3.970 | 0:+2.785%,1:+5.180% |
| live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 4:+7.692% |
| live-subscriber | cancel | measured_turn_cpu_ms | 0.000 | 3:+50.000%,4:+50.000% |
| live-subscriber | cancel | max_sampled_turn_rss_mib | -0.832 | 0:+0.391% |
| live-subscriber | cancel | through_product_exit_wall_ms | 1.037 | 0:+1.674%,2:+0.399%,4:+2.748%,5:+2.105% |
| live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 0.961 | 0:+1.441%,2:+0.481%,4:+2.768%,5:+1.915% |

- Only fresh CPU-2,8 Rust baseline/candidate products are compared. Fixed Pi/DeepSeek measurements used unrestricted CPUs in an earlier batch; their original values are retained separately and no ratio combines those batches.
- Six balanced-order pairs per cell: three baseline-first and three candidate-first. All reversals and unavailable counters remain visible; no confidence interval or parity claim.
- /proc CPU counters use 100 ticks/s: cumulative CPU endpoints have 10 ms granularity, not profiler samples.
- Product measured-turn medians and means are different estimands. Across products report median and range of those statistics; never pool turns.
- DeepSeek SDK idle does not await native flush/fsync. Native wait4 exit/flush and verification CPU remain separate; full process write_bytes is unavailable, not zero.

Full per-product values, raw source hashes, separate DeepSeek exit metrics and unavailable ratios are retained in report-data.json.
