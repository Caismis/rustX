# Shared domain and native content read performance evidence

Only fresh CPU-2,8 Rust baseline/candidate products are compared. Fixed Pi/DeepSeek measurements used unrestricted CPUs in an earlier batch; their original values are retained separately and no ratio combines those batches.

Fresh Rust: six interleaved pairs per cell, CPU affinity 2,8, with three pairs in each execution order.

| Mode | Case | Paired wall median % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -1.539 [-19.396,3.782] | -0.562 | -0.439 | 0.307 | 0.163 |
| no-live-subscriber | short-large | -3.601 [-13.275,-1.730] | -2.848 | -0.437 | -0.124 | 0.046 |
| no-live-subscriber | stream | 0.946 [-0.660,2.950] | 2.083 | -0.246 | -0.113 | -0.130 |
| no-live-subscriber | large | -1.388 [-5.109,1.472] | 0.000 | -0.282 | 0.007 | 0.267 |
| no-live-subscriber | tool | -1.312 [-7.062,0.674] | 0.000 | -0.114 | 0.053 | 0.487 |
| no-live-subscriber | cancel | -3.426 [-7.251,4.467] | 0.000 | -0.145 | -0.062 | 0.049 |
| live-subscriber | short-small | -3.140 [-10.544,6.977] | -1.015 | -0.433 | -0.102 | 0.221 |
| live-subscriber | short-large | -14.069 [-20.583,-6.198] | -3.083 | -0.424 | -0.058 | -0.066 |
| live-subscriber | stream | 1.621 [-0.469,2.399] | 13.636 | -0.246 | 0.080 | 0.202 |
| live-subscriber | large | -2.115 [-4.403,0.840] | 0.000 | -0.282 | 0.145 | 0.217 |
| live-subscriber | tool | -5.397 [-18.116,-0.837] | 0.000 | -0.114 | -0.628 | 0.014 |
| live-subscriber | cancel | -4.543 [-7.698,7.748] | 0.000 | -0.145 | -0.069 | 0.212 |

Fresh Rust absolute warm-request windows, each product contributing 32 measured turns:

| Mode | History | Revision | Mean ms/turn median [min,max] | Median ms/turn median [min,max] | CPU ms/turn | Write bytes/turn |
|---|---:|---|---:|---:|---:|---:|
| no-live-subscriber | 8 | baseline | 7.639 [7.530,42.158] | 7.054 [7.024,9.136] | 8.438 [8.125,12.812] | 853504.000 [853504.000,855808.000] |
| no-live-subscriber | 8 | candidate | 7.551 [7.406,9.376] | 7.032 [6.895,7.364] | 8.594 [8.125,9.062] | 850112.000 [850048.000,850304.000] |
| no-live-subscriber | 64 | baseline | 8.747 [8.486,9.182] | 7.864 [7.723,8.663] | 9.844 [9.375,10.625] | 1009216.000 [1008384.000,1010048.000] |
| no-live-subscriber | 64 | candidate | 8.358 [8.233,8.455] | 7.581 [7.513,7.655] | 9.219 [8.750,9.688] | 1004736.000 [1003904.000,1006464.000] |
| live-subscriber | 8 | baseline | 8.680 [8.603,9.071] | 8.150 [8.050,8.725] | 10.938 [10.625,11.250] | 853632.000 [853120.000,854016.000] |
| live-subscriber | 8 | candidate | 8.393 [8.294,8.882] | 7.966 [7.805,8.687] | 10.781 [10.312,11.562] | 850240.000 [849792.000,850304.000] |
| live-subscriber | 64 | baseline | 10.668 [10.128,11.302] | 9.938 [9.277,10.726] | 13.438 [12.812,14.062] | 1008768.000 [1007744.000,1009792.000] |
| live-subscriber | 64 | candidate | 9.301 [9.144,9.618] | 8.683 [8.396,8.829] | 11.562 [11.250,12.188] | 1004800.000 [1003904.000,1005696.000] |

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
| no-live-subscriber | short-small | mean_turn_ms | -1.597 | 3:+0.023%,5:+4.628% |
| no-live-subscriber | short-small | turn_cpu_ms_per_turn | 0.000 | 3:+3.704%,5:+3.704% |
| no-live-subscriber | short-small | through_cleanup_self_cpu_ms | -0.562 | 3:+2.247%,5:+2.222% |
| no-live-subscriber | short-small | lifecycle_ms | -1.016 | 3:+2.640%,5:+2.778% |
| no-live-subscriber | short-small | max_sampled_anon_pss_mib | 0.307 | 0:+1.018%,1:+0.242%,2:+1.214%,4:+0.372%,5:+0.086% |
| no-live-subscriber | short-small | max_sampled_pss_mib | 0.200 | 0:+0.476%,1:+0.120%,2:+1.073%,4:+0.279%,5:+0.047% |
| no-live-subscriber | short-small | max_sampled_rss_mib | 0.163 | 0:+0.464%,1:+0.093%,2:+1.036%,4:+0.233%,5:+0.013% |
| no-live-subscriber | short-small | median_measured_turn_ms | -1.539 | 5:+3.782% |
| no-live-subscriber | short-small | lifecycle_self_cpu_ms | -0.562 | 3:+2.247%,5:+2.222% |
| no-live-subscriber | short-small | measured_turn_cpu_ms | 0.000 | 3:+3.704%,5:+3.704% |
| no-live-subscriber | short-small | max_sampled_turn_rss_mib | 0.005 | 0:+0.257%,2:+0.754%,4:+0.145% |
| no-live-subscriber | short-small | through_product_exit_wall_ms | -0.780 | 3:+2.619%,5:+3.135% |
| no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -0.926 | 3:+2.744%,5:+3.531% |
| no-live-subscriber | short-large | turn_cpu_ms_per_turn | -7.964 | 1:+3.333% |
| no-live-subscriber | short-large | max_sampled_anon_pss_mib | -0.124 | 2:+0.316%,5:+0.257% |
| no-live-subscriber | short-large | max_sampled_pss_mib | 0.062 | 2:+0.168%,3:+0.140%,5:+0.388% |
| no-live-subscriber | short-large | max_sampled_rss_mib | 0.046 | 2:+0.181%,3:+0.109%,5:+0.384% |
| no-live-subscriber | short-large | measured_turn_cpu_ms | -7.964 | 1:+3.333% |
| no-live-subscriber | short-large | max_sampled_turn_rss_mib | -0.166 | 3:+0.230%,5:+0.611% |
| no-live-subscriber | stream | mean_turn_ms | 0.946 | 0:+0.835%,2:+1.057%,3:+1.379%,4:+0.780%,5:+2.950% |
| no-live-subscriber | stream | turn_cpu_ms_per_turn | 3.846 | 0:+7.692%,3:+30.000%,5:+40.000% |
| no-live-subscriber | stream | through_cleanup_self_cpu_ms | 2.083 | 2:+4.167%,3:+9.091%,5:+19.048% |
| no-live-subscriber | stream | lifecycle_ms | 0.839 | 0:+1.209%,2:+0.488%,3:+1.189%,5:+2.936% |
| no-live-subscriber | stream | max_sampled_anon_pss_mib | -0.113 | 4:+0.106%,5:+0.829% |
| no-live-subscriber | stream | max_sampled_pss_mib | -0.133 | 1:+0.041%,5:+0.101% |
| no-live-subscriber | stream | max_sampled_rss_mib | -0.130 | 1:+0.040%,5:+0.113% |
| no-live-subscriber | stream | median_measured_turn_ms | 0.946 | 0:+0.835%,2:+1.057%,3:+1.379%,4:+0.780%,5:+2.950% |
| no-live-subscriber | stream | lifecycle_self_cpu_ms | 2.083 | 2:+4.167%,3:+9.091%,5:+19.048% |
| no-live-subscriber | stream | measured_turn_cpu_ms | 3.846 | 0:+7.692%,3:+30.000%,5:+40.000% |
| no-live-subscriber | stream | max_sampled_turn_rss_mib | -0.195 | 2:+0.289%,5:+0.140% |
| no-live-subscriber | stream | through_product_exit_wall_ms | 0.838 | 0:+1.290%,2:+0.387%,3:+1.324%,5:+2.844% |
| no-live-subscriber | stream | complete_harness_lifecycle_wall_ms | 0.757 | 0:+1.130%,2:+0.384%,3:+1.509%,5:+2.766% |
| no-live-subscriber | large | mean_turn_ms | -1.388 | 0:+1.472%,4:+0.101% |
| no-live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 0:+100.000% |
| no-live-subscriber | large | through_cleanup_self_cpu_ms | 0.000 | 5:+7.692% |
| no-live-subscriber | large | lifecycle_ms | 1.513 | 0:+0.719%,1:+2.267%,3:+3.037%,4:+0.760%,5:+3.485% |
| no-live-subscriber | large | max_sampled_anon_pss_mib | 0.007 | 0:+0.222%,2:+0.053%,4:+0.354% |
| no-live-subscriber | large | max_sampled_pss_mib | 0.256 | 0:+0.165%,1:+0.347%,2:+0.586%,4:+0.552% |
| no-live-subscriber | large | max_sampled_rss_mib | 0.267 | 0:+0.150%,1:+0.384%,2:+0.585%,4:+0.496% |
| no-live-subscriber | large | median_measured_turn_ms | -1.388 | 0:+1.472%,4:+0.101% |
| no-live-subscriber | large | lifecycle_self_cpu_ms | 0.000 | 5:+7.692% |
| no-live-subscriber | large | measured_turn_cpu_ms | 0.000 | 0:+100.000% |
| no-live-subscriber | large | max_sampled_turn_rss_mib | 0.280 | 0:+0.187%,1:+0.686%,2:+0.888%,3:+0.130%,4:+0.373% |
| no-live-subscriber | large | through_product_exit_wall_ms | 0.851 | 0:+0.979%,1:+0.724%,3:+2.556%,5:+4.140% |
| no-live-subscriber | large | complete_harness_lifecycle_wall_ms | 0.749 | 0:+1.033%,1:+0.465%,3:+2.205%,5:+4.085% |
| no-live-subscriber | tool | mean_turn_ms | -1.312 | 1:+0.651%,3:+0.674% |
| no-live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 1:+50.000%,3:+33.333% |
| no-live-subscriber | tool | lifecycle_ms | 0.670 | 1:+1.055%,2:+2.822%,3:+3.597%,5:+0.285% |
| no-live-subscriber | tool | max_sampled_anon_pss_mib | 0.053 | 1:+0.004%,3:+0.529%,4:+0.101%,5:+1.126% |
| no-live-subscriber | tool | max_sampled_pss_mib | 0.471 | 1:+0.421%,2:+0.106%,3:+0.633%,4:+0.521%,5:+1.112% |
| no-live-subscriber | tool | max_sampled_rss_mib | 0.487 | 1:+0.456%,2:+0.145%,3:+0.621%,4:+0.519%,5:+1.064% |
| no-live-subscriber | tool | median_measured_turn_ms | -1.312 | 1:+0.651%,3:+0.674% |
| no-live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 1:+50.000%,3:+33.333% |
| no-live-subscriber | tool | max_sampled_turn_rss_mib | 0.323 | 1:+0.262%,2:+0.278%,3:+0.729%,4:+0.369%,5:+1.581% |
| no-live-subscriber | tool | through_product_exit_wall_ms | -0.017 | 1:+0.912%,2:+2.912%,3:+3.417% |
| no-live-subscriber | tool | complete_harness_lifecycle_wall_ms | -0.002 | 1:+0.626%,2:+2.606%,3:+3.555% |
| no-live-subscriber | cancel | mean_turn_ms | -3.426 | 1:+4.467%,2:+3.147% |
| no-live-subscriber | cancel | turn_cpu_ms_per_turn | 25.000 | 1:+50.000%,3:+50.000%,4:+50.000% |
| no-live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 2:+7.692% |
| no-live-subscriber | cancel | lifecycle_ms | -0.455 | 2:+3.248%,3:+3.362% |
| no-live-subscriber | cancel | max_sampled_anon_pss_mib | -0.062 | 2:+0.153%,5:+0.398% |
| no-live-subscriber | cancel | max_sampled_pss_mib | 0.023 | 1:+0.010%,2:+0.036%,3:+0.100%,4:+0.160%,5:+0.011% |
| no-live-subscriber | cancel | max_sampled_rss_mib | 0.049 | 1:+0.059%,2:+0.038%,3:+0.149%,4:+0.132%,5:+0.019% |
| no-live-subscriber | cancel | median_measured_turn_ms | -3.426 | 1:+4.467%,2:+3.147% |
| no-live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 2:+7.692% |
| no-live-subscriber | cancel | measured_turn_cpu_ms | 25.000 | 1:+50.000%,3:+50.000%,4:+50.000% |
| no-live-subscriber | cancel | max_sampled_turn_rss_mib | 0.100 | 0:+0.260%,1:+0.047%,2:+0.152%,3:+0.360% |
| no-live-subscriber | cancel | through_product_exit_wall_ms | 0.349 | 2:+3.630%,3:+3.135%,5:+0.858% |
| no-live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 0.568 | 2:+3.385%,3:+3.058%,5:+1.141% |
| live-subscriber | short-small | mean_turn_ms | -2.576 | 1:+2.757% |
| live-subscriber | short-small | turn_cpu_ms_per_turn | -2.817 | 1:+5.714% |
| live-subscriber | short-small | through_cleanup_self_cpu_ms | -1.015 | 1:+3.030%,3:+1.010% |
| live-subscriber | short-small | lifecycle_ms | -0.857 | 1:+0.975% |
| live-subscriber | short-small | max_sampled_anon_pss_mib | -0.102 | 1:+0.313%,3:+0.058%,5:+0.819% |
| live-subscriber | short-small | max_sampled_pss_mib | 0.208 | 1:+0.548%,2:+0.012%,3:+0.405%,5:+0.797% |
| live-subscriber | short-small | max_sampled_rss_mib | 0.221 | 1:+0.508%,2:+0.069%,3:+0.373%,5:+0.796% |
| live-subscriber | short-small | median_measured_turn_ms | -3.140 | 1:+6.977% |
| live-subscriber | short-small | lifecycle_self_cpu_ms | -1.015 | 1:+3.030%,3:+1.010% |
| live-subscriber | short-small | measured_turn_cpu_ms | -2.817 | 1:+5.714% |
| live-subscriber | short-small | max_sampled_turn_rss_mib | 0.279 | 1:+0.711%,3:+0.604%,5:+0.899% |
| live-subscriber | short-small | through_product_exit_wall_ms | -0.681 | 1:+1.219% |
| live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -0.779 | 1:+1.934%,3:+0.341% |
| live-subscriber | short-large | max_sampled_anon_pss_mib | -0.058 | 0:+0.096%,5:+0.814% |
| live-subscriber | short-large | max_sampled_pss_mib | -0.053 | 0:+0.210%,1:+0.045%,5:+0.587% |
| live-subscriber | short-large | max_sampled_rss_mib | -0.066 | 0:+0.217%,1:+0.022%,5:+0.586% |
| live-subscriber | short-large | max_sampled_turn_rss_mib | -0.082 | 0:+0.026%,1:+0.353%,5:+0.846% |
| live-subscriber | stream | mean_turn_ms | 1.621 | 1:+0.441%,2:+2.249%,3:+2.399%,4:+1.023%,5:+2.218% |
| live-subscriber | stream | turn_cpu_ms_per_turn | 14.091 | 1:+18.182%,2:+18.182%,3:+33.333%,4:+9.091%,5:+10.000% |
| live-subscriber | stream | through_cleanup_self_cpu_ms | 13.636 | 1:+4.545%,2:+13.636%,3:+20.000%,4:+13.636%,5:+14.286% |
| live-subscriber | stream | lifecycle_ms | 1.638 | 1:+0.126%,2:+1.854%,3:+3.017%,4:+1.422%,5:+2.713% |
| live-subscriber | stream | max_sampled_anon_pss_mib | 0.080 | 0:+0.306%,2:+0.185%,4:+0.132%,5:+0.027% |
| live-subscriber | stream | max_sampled_pss_mib | 0.202 | 0:+0.174%,2:+0.230%,4:+0.372%,5:+0.438% |
| live-subscriber | stream | max_sampled_rss_mib | 0.202 | 0:+0.174%,2:+0.229%,4:+0.366%,5:+0.434% |
| live-subscriber | stream | median_measured_turn_ms | 1.621 | 1:+0.441%,2:+2.249%,3:+2.399%,4:+1.023%,5:+2.218% |
| live-subscriber | stream | lifecycle_self_cpu_ms | 13.636 | 1:+4.545%,2:+13.636%,3:+20.000%,4:+13.636%,5:+14.286% |
| live-subscriber | stream | measured_turn_cpu_ms | 14.091 | 1:+18.182%,2:+18.182%,3:+33.333%,4:+9.091%,5:+10.000% |
| live-subscriber | stream | max_sampled_turn_rss_mib | 0.263 | 0:+0.194%,2:+0.331%,3:+0.060%,4:+0.334%,5:+0.470% |
| live-subscriber | stream | through_product_exit_wall_ms | 1.613 | 1:+0.129%,2:+1.821%,3:+3.046%,4:+1.406%,5:+2.529% |
| live-subscriber | stream | complete_harness_lifecycle_wall_ms | 1.610 | 1:+0.167%,2:+1.800%,3:+3.175%,4:+1.420%,5:+2.457% |
| live-subscriber | large | mean_turn_ms | -2.115 | 1:+0.840%,5:+0.226% |
| live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 5:+33.333% |
| live-subscriber | large | lifecycle_ms | 0.693 | 1:+3.451%,2:+1.928%,5:+6.740% |
| live-subscriber | large | max_sampled_anon_pss_mib | 0.145 | 0:+0.057%,1:+0.525%,4:+0.490%,5:+0.233% |
| live-subscriber | large | max_sampled_pss_mib | 0.215 | 0:+0.022%,1:+0.352%,2:+0.078%,4:+0.462%,5:+0.476% |
| live-subscriber | large | max_sampled_rss_mib | 0.217 | 0:+0.016%,1:+0.346%,2:+0.088%,4:+0.452%,5:+0.463% |
| live-subscriber | large | median_measured_turn_ms | -2.115 | 1:+0.840%,5:+0.226% |
| live-subscriber | large | measured_turn_cpu_ms | 0.000 | 5:+33.333% |
| live-subscriber | large | max_sampled_turn_rss_mib | 0.356 | 1:+0.353%,2:+0.022%,3:+0.358%,4:+0.588%,5:+0.514% |
| live-subscriber | large | through_product_exit_wall_ms | 1.561 | 1:+3.248%,2:+2.837%,3:+0.285%,5:+6.753% |
| live-subscriber | large | complete_harness_lifecycle_wall_ms | 1.672 | 1:+3.076%,2:+3.097%,3:+0.268%,5:+6.150% |
| live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 5:+33.333% |
| live-subscriber | tool | max_sampled_anon_pss_mib | -0.628 | 0:+0.401%,4:+0.895% |
| live-subscriber | tool | max_sampled_pss_mib | 0.009 | 0:+0.531%,4:+0.615%,5:+0.098% |
| live-subscriber | tool | max_sampled_rss_mib | 0.014 | 0:+0.518%,4:+0.598%,5:+0.074% |
| live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 5:+33.333% |
| live-subscriber | tool | max_sampled_turn_rss_mib | -0.101 | 4:+1.049%,5:+0.024% |
| live-subscriber | tool | closed_logical_bytes | 0.000 | 4:+0.000% |
| live-subscriber | cancel | mean_turn_ms | -4.543 | 1:+0.930%,3:+7.748% |
| live-subscriber | cancel | turn_cpu_ms_per_turn | 0.000 | 2:+200.000% |
| live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 3:+23.077% |
| live-subscriber | cancel | through_cleanup_process_write_mib | -0.145 | 2:+4.079% |
| live-subscriber | cancel | lifecycle_ms | -0.575 | 1:+1.776%,3:+10.578%,5:+1.910% |
| live-subscriber | cancel | max_sampled_anon_pss_mib | -0.069 | 2:+1.156%,3:+0.205%,5:+0.054% |
| live-subscriber | cancel | max_sampled_pss_mib | 0.197 | 0:+0.308%,2:+1.092%,3:+0.286%,5:+0.107% |
| live-subscriber | cancel | max_sampled_rss_mib | 0.212 | 0:+0.345%,2:+1.087%,3:+0.279%,5:+0.146% |
| live-subscriber | cancel | median_measured_turn_ms | -4.543 | 1:+0.930%,3:+7.748% |
| live-subscriber | cancel | measured_turn_write_bytes | -0.510 | 2:+10.795% |
| live-subscriber | cancel | turn_write_bytes_per_turn | -0.510 | 2:+10.795% |
| live-subscriber | cancel | through_cleanup_process_write_bytes | -0.145 | 2:+4.079% |
| live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 3:+23.077% |
| live-subscriber | cancel | measured_turn_cpu_ms | 0.000 | 2:+200.000% |
| live-subscriber | cancel | max_sampled_turn_rss_mib | 0.103 | 0:+0.085%,1:+0.099%,2:+1.290%,3:+0.747%,5:+0.107% |
| live-subscriber | cancel | through_product_exit_wall_ms | -1.078 | 1:+2.050%,3:+12.320%,5:+0.493% |
| live-subscriber | cancel | complete_harness_lifecycle_wall_ms | -1.121 | 1:+2.308%,3:+13.730%,5:+0.201% |
| live-subscriber | cancel | closed_logical_bytes | -0.272 | 2:+5.462% |
| live-subscriber | cancel | closed_allocated_bytes | -0.267 | 2:+5.367% |

- Only fresh CPU-2,8 Rust baseline/candidate products are compared. Fixed Pi/DeepSeek measurements used unrestricted CPUs in an earlier batch; their original values are retained separately and no ratio combines those batches.
- Six balanced-order pairs per cell: three baseline-first and three candidate-first. All reversals and unavailable counters remain visible; no confidence interval or parity claim.
- /proc CPU counters use 100 ticks/s: cumulative CPU endpoints have 10 ms granularity, not profiler samples.
- Product measured-turn medians and means are different estimands. Across products report median and range of those statistics; never pool turns.
- DeepSeek SDK idle does not await native flush/fsync. Native wait4 exit/flush and verification CPU remain separate; full process write_bytes is unavailable, not zero.

Full per-product values, raw source hashes, separate DeepSeek exit metrics and unavailable ratios are retained in report-data.json.
