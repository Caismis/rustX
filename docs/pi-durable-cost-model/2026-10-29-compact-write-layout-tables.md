# Compact write layout performance evidence

Fresh Rust comparison: five interleaved pairs per cell. Historical Pi/DeepSeek rows are fixed, separate batches; their ratios are ratios of medians, not paired results.

| Mode | Case | Paired median wall % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | 6.812 [-40.375,65.441] | 6.400 | -2.504 | 0.261 | 1.202 |
| no-live-subscriber | short-large | -1.751 [-48.754,57.837] | 1.733 | -2.172 | -0.113 | 0.433 |
| no-live-subscriber | stream | 0.428 [-3.645,4.518] | 0.000 | -3.081 | -0.382 | 1.067 |
| no-live-subscriber | large | -13.888 [-20.205,2.339] | -6.250 | -3.217 | 0.246 | 1.364 |
| no-live-subscriber | tool | 8.517 [-16.223,40.459] | 0.000 | -3.652 | 0.083 | 0.864 |
| no-live-subscriber | cancel | -7.638 [-13.607,8.320] | -5.882 | -3.315 | -0.232 | 0.891 |
| live-subscriber | short-small | -3.142 [-47.472,4.485] | 1.974 | -2.484 | 0.024 | 0.774 |
| live-subscriber | short-large | 31.506 [-41.497,100.273] | -0.217 | -2.196 | -0.105 | 0.527 |
| live-subscriber | stream | -0.465 [-3.945,0.819] | 0.000 | -2.844 | -0.597 | 0.353 |
| live-subscriber | large | 12.147 [-22.347,159.889] | -6.250 | -3.217 | -0.616 | 0.973 |
| live-subscriber | tool | -3.176 [-23.998,85.894] | -5.556 | -3.652 | -0.013 | 0.775 |
| live-subscriber | cancel | 16.667 [-9.838,121.783] | 0.000 | -3.315 | -0.708 | 0.756 |

| Mode | Case | Mean wall / old Pi | Median wall / old Pi | Full CPU / old Pi | Full writes / old Pi |
|---|---|---:|---:|---:|---:|
| no-live-subscriber | short-small | 3.706 | 4.435 | 1.277 | 4.145 |
| no-live-subscriber | short-large | 4.073 | 4.650 | 2.794 | 4.630 |
| no-live-subscriber | stream | 0.933 | 0.933 | 0.255 | 3.718 |
| no-live-subscriber | large | 0.377 | 0.377 | 0.254 | 4.198 |
| no-live-subscriber | tool | 0.418 | 0.418 | 0.286 | 3.737 |
| no-live-subscriber | cancel | 0.312 | 0.312 | 0.246 | 4.403 |
| live-subscriber | short-small | 4.656 | 4.930 | 1.608 | 4.146 |
| live-subscriber | short-large | 4.804 | 5.462 | 3.060 | 4.630 |
| live-subscriber | stream | 0.919 | 0.919 | 0.279 | 3.727 |
| live-subscriber | large | 0.496 | 0.496 | 0.288 | 4.198 |
| live-subscriber | tool | 0.427 | 0.427 | 0.270 | 3.737 |
| live-subscriber | cancel | 0.353 | 0.353 | 0.267 | 4.403 |

Warm measured windows (per-product statistic, then median [minimum,maximum] across five Rust/Pi products; each has 32 measured turns):

| Mode | History | Engine/batch | Mean ms/turn | Median ms/turn | CPU ms/turn | Write bytes/turn |
|---|---:|---|---:|---:|---:|---:|
| no-live-subscriber | 8 | current Rust | 13.435 [8.709,17.255] | 13.905 [8.144,16.245] | 15.312 [10.312,19.688] | 944000.000 [943744.000,944384.000] |
| no-live-subscriber | 8 | fixed old Pi | 3.625 [3.524,4.546] | 3.135 [3.014,3.328] | 7.188 [6.875,9.688] | 224512.000 [224512.000,224512.000] |
| no-live-subscriber | 64 | current Rust | 15.057 [9.871,18.274] | 15.341 [9.142,18.472] | 17.188 [11.875,20.938] | 1132032.000 [1130880.000,1133184.000] |
| no-live-subscriber | 64 | fixed old Pi | 3.697 [3.667,3.911] | 3.299 [3.159,3.524] | 6.250 [5.938,6.875] | 224384.000 [224384.000,224384.000] |
| live-subscriber | 8 | current Rust | 18.316 [12.959,18.618] | 16.860 [9.822,17.576] | 24.688 [18.125,25.938] | 944256.000 [944000.000,944640.000] |
| live-subscriber | 8 | fixed old Pi | 3.934 [3.784,4.165] | 3.420 [3.309,3.699] | 7.500 [7.188,8.438] | 224512.000 [224512.000,224512.000] |
| live-subscriber | 64 | current Rust | 19.161 [11.649,21.386] | 19.338 [11.523,20.519] | 26.250 [16.250,28.438] | 1132928.000 [1131648.000,1133184.000] |
| live-subscriber | 64 | fixed old Pi | 3.989 [3.827,4.233] | 3.541 [3.482,3.679] | 6.875 [6.250,7.188] | 224384.000 [224384.000,224384.000] |

| Live case | Current mean ms/turn | Fixed DS mean ms/turn | Wall ratio | CPU ratio | Measured write ratio |
|---|---:|---:|---:|---:|---:|
| short-small | 18.316 | 15.541 | 1.179 | 1.362 | 43.911 |
| short-large | 19.161 | 15.450 | 1.240 | 1.500 | 53.000 |
| stream | 1595.470 | 1583.218 | 1.008 | 0.533 | 45.429 |
| large | 44.566 | 165.613 | 0.269 | 0.250 | 30.143 |

DeepSeek exit/flush CPU remains separate from measured turns and from Rust full-lifecycle CPU; full DeepSeek process write bytes remain unavailable. The JSON retains the exact wait4 metrics and source-specific limits.

All adverse metrics (including individual fresh paired reversals):

| Comparison | Mode | Case | Metric | Median % or cross-batch ratio | Adverse pairs |
|---|---|---|---|---:|---|
| fresh_paired_rust | no-live-subscriber | short-small | mean_turn_ms | 10.295 | 0:+50.224%,3:+15.503%,4:+10.295% |
| fresh_paired_rust | no-live-subscriber | short-small | turn_cpu_ms_per_turn | 10.526 | 0:+44.118%,3:+13.208%,4:+10.526% |
| fresh_paired_rust | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 6.400 | 0:+21.212%,3:+6.400%,4:+7.874% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_ms | 7.912 | 0:+18.861%,3:+7.912%,4:+8.474% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_anon_pss_mib | 0.261 | 0:+0.102%,1:+0.709%,2:+0.261%,4:+1.399% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_pss_mib | 1.217 | 0:+0.831%,1:+1.217%,2:+1.279%,3:+0.351%,4:+1.729% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_rss_mib | 1.202 | 0:+0.760%,1:+1.202%,2:+1.249%,3:+0.399%,4:+1.761% |
| fresh_paired_rust | no-live-subscriber | short-small | median_measured_turn_ms | 6.812 | 0:+65.441%,3:+7.185%,4:+6.812% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_self_cpu_ms | 6.400 | 0:+21.212%,3:+6.400%,4:+7.874% |
| fresh_paired_rust | no-live-subscriber | short-small | measured_turn_cpu_ms | 10.526 | 0:+44.118%,3:+13.208%,4:+10.526% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_turn_rss_mib | 0.959 | 0:+0.641%,1:+1.617%,2:+0.959%,3:+0.527%,4:+1.997% |
| fresh_paired_rust | no-live-subscriber | short-small | through_product_exit_wall_ms | 7.697 | 0:+17.520%,3:+7.697%,4:+8.250% |
| fresh_paired_rust | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 7.560 | 0:+15.531%,3:+7.560%,4:+8.164% |
| fresh_paired_rust | no-live-subscriber | short-large | mean_turn_ms | -5.411 | 0:+6.664%,4:+46.442% |
| fresh_paired_rust | no-live-subscriber | short-large | turn_cpu_ms_per_turn | -6.061 | 0:+6.349%,4:+41.026% |
| fresh_paired_rust | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 1.733 | 0:+1.733%,1:+2.604%,4:+13.239% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_ms | 1.290 | 0:+1.290%,1:+2.512%,4:+12.519% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_anon_pss_mib | -0.113 | 1:+0.227% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_pss_mib | 0.431 | 0:+0.431%,1:+0.727%,2:+0.605%,3:+0.359%,4:+0.378% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_rss_mib | 0.433 | 0:+0.433%,1:+0.729%,2:+0.598%,3:+0.365%,4:+0.363% |
| fresh_paired_rust | no-live-subscriber | short-large | median_measured_turn_ms | -1.751 | 0:+3.673%,4:+57.837% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_self_cpu_ms | 1.733 | 0:+1.733%,1:+2.604%,4:+13.239% |
| fresh_paired_rust | no-live-subscriber | short-large | measured_turn_cpu_ms | -6.061 | 0:+6.349%,4:+41.026% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_turn_rss_mib | 0.649 | 0:+0.649%,1:+1.290%,2:+0.675%,3:+0.388%,4:+0.531% |
| fresh_paired_rust | no-live-subscriber | short-large | through_product_exit_wall_ms | 1.272 | 0:+1.272%,1:+2.297%,4:+11.811% |
| fresh_paired_rust | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 1.068 | 0:+1.068%,1:+1.999%,4:+11.235% |
| fresh_paired_rust | no-live-subscriber | stream | mean_turn_ms | 0.428 | 0:+4.518%,3:+1.662%,4:+0.428% |
| fresh_paired_rust | no-live-subscriber | stream | turn_cpu_ms_per_turn | 6.250 | 0:+50.000%,3:+14.286%,4:+6.250% |
| fresh_paired_rust | no-live-subscriber | stream | through_cleanup_self_cpu_ms | 0.000 | 0:+24.000%,3:+11.538% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_ms | 1.390 | 0:+5.409%,3:+2.490%,4:+1.390% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_anon_pss_mib | -0.382 | 3:+0.532% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_pss_mib | 1.099 | 0:+1.245%,1:+0.653%,2:+0.889%,3:+1.099%,4:+1.215% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_rss_mib | 1.067 | 0:+1.210%,1:+0.653%,2:+0.835%,3:+1.067%,4:+1.148% |
| fresh_paired_rust | no-live-subscriber | stream | median_measured_turn_ms | 0.428 | 0:+4.518%,3:+1.662%,4:+0.428% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_self_cpu_ms | 0.000 | 0:+24.000%,3:+11.538% |
| fresh_paired_rust | no-live-subscriber | stream | measured_turn_cpu_ms | 6.250 | 0:+50.000%,3:+14.286%,4:+6.250% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_turn_rss_mib | 1.109 | 0:+1.561%,2:+0.858%,3:+1.109%,4:+1.124% |
| fresh_paired_rust | no-live-subscriber | stream | through_product_exit_wall_ms | 1.192 | 0:+5.277%,3:+2.356%,4:+1.192% |
| fresh_paired_rust | no-live-subscriber | stream | complete_harness_lifecycle_wall_ms | 0.316 | 0:+4.931%,3:+3.097%,4:+0.316% |
| fresh_paired_rust | no-live-subscriber | large | mean_turn_ms | -13.888 | 3:+2.339% |
| fresh_paired_rust | no-live-subscriber | large | turn_cpu_ms_per_turn | -25.000 | 4:+50.000% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_ms | -0.947 | 2:+0.083%,4:+3.358% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_anon_pss_mib | 0.246 | 0:+0.246%,1:+0.623%,2:+1.620%,4:+0.137% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_pss_mib | 1.376 | 0:+1.050%,1:+1.376%,2:+2.752%,3:+0.801%,4:+1.644% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_rss_mib | 1.364 | 0:+1.030%,1:+1.364%,2:+2.673%,3:+0.791%,4:+1.621% |
| fresh_paired_rust | no-live-subscriber | large | median_measured_turn_ms | -13.888 | 3:+2.339% |
| fresh_paired_rust | no-live-subscriber | large | measured_turn_cpu_ms | -25.000 | 4:+50.000% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_turn_rss_mib | 1.037 | 0:+1.037%,1:+0.819%,2:+2.563%,3:+0.870%,4:+1.281% |
| fresh_paired_rust | no-live-subscriber | large | through_product_exit_wall_ms | -0.301 | 2:+0.487%,4:+6.071% |
| fresh_paired_rust | no-live-subscriber | large | complete_harness_lifecycle_wall_ms | 0.359 | 2:+1.386%,3:+0.359%,4:+9.208% |
| fresh_paired_rust | no-live-subscriber | tool | mean_turn_ms | 8.517 | 1:+8.517%,2:+40.459%,3:+13.038%,4:+8.326% |
| fresh_paired_rust | no-live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 1:+20.000%,2:+25.000% |
| fresh_paired_rust | no-live-subscriber | tool | through_cleanup_self_cpu_ms | 0.000 | 0:+11.765%,2:+25.000% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_ms | -0.105 | 0:+3.545%,2:+16.485% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_anon_pss_mib | 0.083 | 1:+0.750%,3:+0.597%,4:+0.083% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_pss_mib | 0.902 | 0:+0.843%,1:+1.307%,2:+0.125%,3:+1.259%,4:+0.902% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_rss_mib | 0.864 | 0:+0.816%,1:+1.290%,2:+0.050%,3:+1.232%,4:+0.864% |
| fresh_paired_rust | no-live-subscriber | tool | median_measured_turn_ms | 8.517 | 1:+8.517%,2:+40.459%,3:+13.038%,4:+8.326% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_self_cpu_ms | 0.000 | 0:+11.765%,2:+25.000% |
| fresh_paired_rust | no-live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 1:+20.000%,2:+25.000% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_turn_rss_mib | 0.738 | 0:+0.738%,1:+0.676%,2:+0.571%,3:+1.085%,4:+1.124% |
| fresh_paired_rust | no-live-subscriber | tool | through_product_exit_wall_ms | -1.727 | 0:+3.089%,2:+17.690% |
| fresh_paired_rust | no-live-subscriber | tool | complete_harness_lifecycle_wall_ms | -0.898 | 0:+3.329%,2:+15.872% |
| fresh_paired_rust | no-live-subscriber | cancel | mean_turn_ms | -7.638 | 1:+6.459%,4:+8.320% |
| fresh_paired_rust | no-live-subscriber | cancel | turn_cpu_ms_per_turn | 0.000 | 4:+50.000% |
| fresh_paired_rust | no-live-subscriber | cancel | lifecycle_ms | 0.946 | 1:+3.197%,3:+4.790%,4:+0.946% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_anon_pss_mib | -0.232 | 1:+0.084% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_pss_mib | 0.899 | 0:+0.899%,1:+0.824%,2:+1.109%,3:+0.790%,4:+1.227% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_rss_mib | 0.891 | 0:+0.891%,1:+0.794%,2:+1.168%,3:+0.755%,4:+1.246% |
| fresh_paired_rust | no-live-subscriber | cancel | median_measured_turn_ms | -7.638 | 1:+6.459%,4:+8.320% |
| fresh_paired_rust | no-live-subscriber | cancel | measured_turn_cpu_ms | 0.000 | 4:+50.000% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_turn_rss_mib | 0.831 | 0:+0.826%,1:+0.831%,2:+1.067%,3:+0.591%,4:+1.490% |
| fresh_paired_rust | no-live-subscriber | cancel | through_product_exit_wall_ms | 2.340 | 1:+2.340%,2:+3.224%,3:+4.461%,4:+2.237% |
| fresh_paired_rust | no-live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 1.976 | 1:+0.166%,2:+4.899%,3:+4.698%,4:+1.976% |
| fresh_paired_rust | live-subscriber | short-small | mean_turn_ms | 2.230 | 0:+1.659%,1:+2.230%,2:+11.265%,3:+3.676% |
| fresh_paired_rust | live-subscriber | short-small | turn_cpu_ms_per_turn | 2.597 | 0:+1.282%,1:+3.750%,2:+8.451%,3:+2.597% |
| fresh_paired_rust | live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.974 | 0:+1.935%,1:+3.846%,2:+5.405%,3:+1.974% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_ms | 1.826 | 0:+0.608%,1:+2.766%,2:+4.888%,3:+1.826% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_anon_pss_mib | 0.024 | 1:+0.170%,2:+0.030%,4:+0.024% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_pss_mib | 0.762 | 0:+0.609%,1:+0.762%,2:+1.136%,3:+0.294%,4:+0.857% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_rss_mib | 0.774 | 0:+0.566%,1:+0.774%,2:+1.088%,3:+0.288%,4:+0.911% |
| fresh_paired_rust | live-subscriber | short-small | median_measured_turn_ms | -3.142 | 3:+4.485% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_self_cpu_ms | 1.974 | 0:+1.935%,1:+3.846%,2:+5.405%,3:+1.974% |
| fresh_paired_rust | live-subscriber | short-small | measured_turn_cpu_ms | 2.597 | 0:+1.282%,1:+3.750%,2:+8.451%,3:+2.597% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_turn_rss_mib | 0.724 | 0:+0.263%,1:+0.724%,2:+1.047%,3:+0.490%,4:+1.264% |
| fresh_paired_rust | live-subscriber | short-small | through_product_exit_wall_ms | 2.053 | 0:+0.280%,1:+2.418%,2:+4.460%,3:+2.053% |
| fresh_paired_rust | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 2.372 | 1:+2.372%,2:+4.558%,3:+3.226% |
| fresh_paired_rust | live-subscriber | short-large | mean_turn_ms | 26.020 | 0:+94.721%,2:+26.020%,3:+14.560%,4:+39.334% |
| fresh_paired_rust | live-subscriber | short-large | turn_cpu_ms_per_turn | 22.222 | 0:+82.000%,2:+22.222%,3:+16.667%,4:+37.705% |
| fresh_paired_rust | live-subscriber | short-large | through_cleanup_self_cpu_ms | -0.217 | 0:+10.329%,3:+5.430% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_ms | -0.430 | 0:+9.332%,3:+4.391% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_anon_pss_mib | -0.105 | 1:+0.284%,3:+1.307% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_pss_mib | 0.513 | 0:+0.240%,1:+0.980%,2:+0.513%,3:+1.697%,4:+0.405% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_rss_mib | 0.527 | 0:+0.241%,1:+0.984%,2:+0.527%,3:+1.694%,4:+0.396% |
| fresh_paired_rust | live-subscriber | short-large | median_measured_turn_ms | 31.506 | 0:+100.273%,2:+31.506%,3:+16.397%,4:+69.646% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_self_cpu_ms | -0.217 | 0:+10.329%,3:+5.430% |
| fresh_paired_rust | live-subscriber | short-large | measured_turn_cpu_ms | 22.222 | 0:+82.000%,2:+22.222%,3:+16.667%,4:+37.705% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_turn_rss_mib | 0.941 | 0:+0.236%,1:+1.271%,2:+0.941%,3:+2.193%,4:+0.557% |
| fresh_paired_rust | live-subscriber | short-large | through_product_exit_wall_ms | -0.618 | 0:+9.265%,3:+3.881% |
| fresh_paired_rust | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | -0.618 | 0:+8.754%,3:+3.625% |
| fresh_paired_rust | live-subscriber | stream | mean_turn_ms | -0.465 | 1:+0.508%,4:+0.819% |
| fresh_paired_rust | live-subscriber | stream | turn_cpu_ms_per_turn | 0.000 | 3:+5.556%,4:+14.286% |
| fresh_paired_rust | live-subscriber | stream | through_cleanup_self_cpu_ms | 0.000 | 1:+14.815%,4:+12.000% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_ms | 0.213 | 0:+0.213%,1:+1.744%,4:+1.601% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_anon_pss_mib | -0.597 | 3:+0.075%,4:+0.194% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_pss_mib | 0.353 | 0:+0.353%,2:+0.165%,3:+0.770%,4:+1.107% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_rss_mib | 0.353 | 0:+0.353%,2:+0.147%,3:+0.806%,4:+1.090% |
| fresh_paired_rust | live-subscriber | stream | median_measured_turn_ms | -0.465 | 1:+0.508%,4:+0.819% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_self_cpu_ms | 0.000 | 1:+14.815%,4:+12.000% |
| fresh_paired_rust | live-subscriber | stream | measured_turn_cpu_ms | 0.000 | 3:+5.556%,4:+14.286% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_turn_rss_mib | 0.384 | 0:+0.384%,3:+0.986%,4:+1.046% |
| fresh_paired_rust | live-subscriber | stream | through_product_exit_wall_ms | 0.272 | 0:+0.272%,1:+1.541%,4:+1.909% |
| fresh_paired_rust | live-subscriber | stream | complete_harness_lifecycle_wall_ms | 0.158 | 0:+0.158%,1:+2.083%,4:+2.509% |
| fresh_paired_rust | live-subscriber | large | mean_turn_ms | 12.147 | 1:+12.147%,2:+21.733%,3:+159.889% |
| fresh_paired_rust | live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 1:+25.000%,2:+20.000% |
| fresh_paired_rust | live-subscriber | large | through_cleanup_self_cpu_ms | -6.250 | 1:+6.250%,2:+12.500% |
| fresh_paired_rust | live-subscriber | large | lifecycle_ms | -1.413 | 1:+2.767%,2:+6.656% |
| fresh_paired_rust | live-subscriber | large | max_sampled_anon_pss_mib | -0.616 | 0:+0.585%,2:+0.530% |
| fresh_paired_rust | live-subscriber | large | max_sampled_pss_mib | 0.944 | 0:+1.129%,1:+0.944%,2:+1.571%,4:+0.600% |
| fresh_paired_rust | live-subscriber | large | max_sampled_rss_mib | 0.973 | 0:+1.032%,1:+0.973%,2:+1.558%,4:+0.617% |
| fresh_paired_rust | live-subscriber | large | median_measured_turn_ms | 12.147 | 1:+12.147%,2:+21.733%,3:+159.889% |
| fresh_paired_rust | live-subscriber | large | lifecycle_self_cpu_ms | -6.250 | 1:+6.250%,2:+12.500% |
| fresh_paired_rust | live-subscriber | large | measured_turn_cpu_ms | 0.000 | 1:+25.000%,2:+20.000% |
| fresh_paired_rust | live-subscriber | large | max_sampled_turn_rss_mib | 0.901 | 0:+0.901%,1:+1.094%,2:+1.015%,4:+0.699% |
| fresh_paired_rust | live-subscriber | large | through_product_exit_wall_ms | -1.314 | 1:+5.412%,2:+6.761% |
| fresh_paired_rust | live-subscriber | large | complete_harness_lifecycle_wall_ms | -1.777 | 1:+8.311%,2:+4.924% |
| fresh_paired_rust | live-subscriber | tool | mean_turn_ms | -3.176 | 3:+85.894%,4:+16.499% |
| fresh_paired_rust | live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 1:+25.000%,4:+25.000% |
| fresh_paired_rust | live-subscriber | tool | lifecycle_ms | -2.922 | 4:+4.886% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_anon_pss_mib | -0.013 | 2:+0.532%,4:+0.214% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_pss_mib | 0.832 | 1:+0.832%,2:+1.602%,3:+0.107%,4:+1.020% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_rss_mib | 0.775 | 1:+0.775%,2:+1.631%,3:+0.103%,4:+1.089% |
| fresh_paired_rust | live-subscriber | tool | median_measured_turn_ms | -3.176 | 3:+85.894%,4:+16.499% |
| fresh_paired_rust | live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 1:+25.000%,4:+25.000% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_turn_rss_mib | 0.521 | 1:+0.521%,2:+1.165%,4:+1.152% |
| fresh_paired_rust | live-subscriber | tool | through_product_exit_wall_ms | -3.282 | 4:+5.285% |
| fresh_paired_rust | live-subscriber | tool | complete_harness_lifecycle_wall_ms | -3.300 | 4:+1.687% |
| fresh_paired_rust | live-subscriber | cancel | mean_turn_ms | 16.667 | 0:+77.186%,1:+16.667%,3:+121.783% |
| fresh_paired_rust | live-subscriber | cancel | turn_cpu_ms_per_turn | 33.333 | 0:+33.333%,1:+33.333%,3:+50.000% |
| fresh_paired_rust | live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 0:+28.571%,3:+23.529% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_ms | -1.218 | 0:+13.078%,3:+39.990% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_anon_pss_mib | -0.708 | 0:+2.137%,1:+1.944% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_pss_mib | 0.692 | 0:+2.317%,1:+2.776%,2:+0.605%,4:+0.692% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_rss_mib | 0.756 | 0:+2.254%,1:+2.683%,2:+0.681%,4:+0.756% |
| fresh_paired_rust | live-subscriber | cancel | median_measured_turn_ms | 16.667 | 0:+77.186%,1:+16.667%,3:+121.783% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 0:+28.571%,3:+23.529% |
| fresh_paired_rust | live-subscriber | cancel | measured_turn_cpu_ms | 33.333 | 0:+33.333%,1:+33.333%,3:+50.000% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_turn_rss_mib | 0.791 | 0:+2.363%,1:+2.536%,2:+0.704%,4:+0.791% |
| fresh_paired_rust | live-subscriber | cancel | through_product_exit_wall_ms | -0.763 | 0:+12.452%,3:+38.897% |
| fresh_paired_rust | live-subscriber | cancel | complete_harness_lifecycle_wall_ms | -0.799 | 0:+11.366%,3:+36.967% |
| prior_pi | no-live-subscriber | short-small | closed_allocated_bytes | 40.881 | cross-batch |
| prior_pi | no-live-subscriber | short-small | closed_logical_bytes | 40.785 | cross-batch |
| prior_pi | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 1.846 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_ms | 2.804 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_self_cpu_ms | 2.353 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_anon_pss_mib | 1.125 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_pss_mib | 1.489 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_rss_mib | 1.087 | cross-batch |
| prior_pi | no-live-subscriber | short-small | mean_turn_ms | 3.706 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_cpu_ms | 2.130 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_write_bytes | 4.205 | cross-batch |
| prior_pi | no-live-subscriber | short-small | median_measured_turn_ms | 4.435 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_bytes | 4.145 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_mib | 4.145 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.277 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_product_exit_wall_ms | 1.837 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_cpu_ms_per_turn | 2.130 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_write_bytes_per_turn | 4.205 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_allocated_bytes | 41.350 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_logical_bytes | 41.270 | cross-batch |
| prior_pi | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.533 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_ms | 4.724 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_self_cpu_ms | 3.980 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_anon_pss_mib | 1.272 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_pss_mib | 1.518 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_rss_mib | 1.203 | cross-batch |
| prior_pi | no-live-subscriber | short-large | mean_turn_ms | 4.073 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_cpu_ms | 2.750 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_write_bytes | 5.045 | cross-batch |
| prior_pi | no-live-subscriber | short-large | median_measured_turn_ms | 4.650 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_bytes | 4.630 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_mib | 4.630 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 2.794 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_product_exit_wall_ms | 3.635 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_cpu_ms_per_turn | 2.750 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_write_bytes_per_turn | 5.045 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_allocated_bytes | 15.500 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_logical_bytes | 15.399 | cross-batch |
| prior_pi | no-live-subscriber | stream | max_sampled_pss_mib | 1.255 | cross-batch |
| prior_pi | no-live-subscriber | stream | measured_turn_write_bytes | 2.516 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_bytes | 3.718 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_mib | 3.718 | cross-batch |
| prior_pi | no-live-subscriber | stream | turn_write_bytes_per_turn | 2.516 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_allocated_bytes | 10.263 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_logical_bytes | 10.161 | cross-batch |
| prior_pi | no-live-subscriber | large | lifecycle_ms | 1.176 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_anon_pss_mib | 1.088 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_pss_mib | 1.561 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_rss_mib | 1.056 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_turn_rss_mib | 1.052 | cross-batch |
| prior_pi | no-live-subscriber | large | measured_turn_write_bytes | 2.931 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_bytes | 4.198 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_mib | 4.198 | cross-batch |
| prior_pi | no-live-subscriber | large | turn_write_bytes_per_turn | 2.931 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_allocated_bytes | 17.406 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_logical_bytes | 17.285 | cross-batch |
| prior_pi | no-live-subscriber | tool | lifecycle_ms | 1.214 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_pss_mib | 1.385 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_turn_rss_mib | 1.064 | cross-batch |
| prior_pi | no-live-subscriber | tool | measured_turn_write_bytes | 2.589 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_bytes | 3.737 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_mib | 3.737 | cross-batch |
| prior_pi | no-live-subscriber | tool | turn_write_bytes_per_turn | 2.589 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_allocated_bytes | 12.700 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_logical_bytes | 12.569 | cross-batch |
| prior_pi | no-live-subscriber | cancel | lifecycle_ms | 1.175 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_pss_mib | 1.406 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_turn_rss_mib | 1.042 | cross-batch |
| prior_pi | no-live-subscriber | cancel | measured_turn_write_bytes | 3.015 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_bytes | 4.403 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_mib | 4.403 | cross-batch |
| prior_pi | no-live-subscriber | cancel | turn_write_bytes_per_turn | 3.015 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_allocated_bytes | 40.881 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_logical_bytes | 40.785 | cross-batch |
| prior_pi | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 2.039 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_ms | 3.089 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_self_cpu_ms | 2.889 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_anon_pss_mib | 1.194 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_pss_mib | 1.559 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_rss_mib | 1.133 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_turn_rss_mib | 1.019 | cross-batch |
| prior_pi | live-subscriber | short-small | mean_turn_ms | 4.656 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_cpu_ms | 3.292 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_write_bytes | 4.206 | cross-batch |
| prior_pi | live-subscriber | short-small | median_measured_turn_ms | 4.930 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_bytes | 4.146 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_mib | 4.146 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.608 | cross-batch |
| prior_pi | live-subscriber | short-small | through_product_exit_wall_ms | 2.030 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_cpu_ms_per_turn | 3.292 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_write_bytes_per_turn | 4.206 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_allocated_bytes | 41.333 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_logical_bytes | 41.270 | cross-batch |
| prior_pi | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.574 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_ms | 4.758 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_self_cpu_ms | 4.330 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_anon_pss_mib | 1.279 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_pss_mib | 1.521 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_rss_mib | 1.209 | cross-batch |
| prior_pi | live-subscriber | short-large | mean_turn_ms | 4.804 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_cpu_ms | 3.818 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_write_bytes | 5.049 | cross-batch |
| prior_pi | live-subscriber | short-large | median_measured_turn_ms | 5.462 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_bytes | 4.630 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_mib | 4.630 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_self_cpu_ms | 3.060 | cross-batch |
| prior_pi | live-subscriber | short-large | through_product_exit_wall_ms | 3.662 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_cpu_ms_per_turn | 3.818 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_write_bytes_per_turn | 5.049 | cross-batch |
| prior_pi | live-subscriber | stream | closed_allocated_bytes | 15.531 | cross-batch |
| prior_pi | live-subscriber | stream | closed_logical_bytes | 15.430 | cross-batch |
| prior_pi | live-subscriber | stream | max_sampled_pss_mib | 1.262 | cross-batch |
| prior_pi | live-subscriber | stream | measured_turn_write_bytes | 2.524 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_bytes | 3.727 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_mib | 3.727 | cross-batch |
| prior_pi | live-subscriber | stream | turn_write_bytes_per_turn | 2.524 | cross-batch |
| prior_pi | live-subscriber | large | closed_allocated_bytes | 10.263 | cross-batch |
| prior_pi | live-subscriber | large | closed_logical_bytes | 10.161 | cross-batch |
| prior_pi | live-subscriber | large | lifecycle_ms | 1.249 | cross-batch |
| prior_pi | live-subscriber | large | lifecycle_self_cpu_ms | 1.062 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_anon_pss_mib | 1.090 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_pss_mib | 1.563 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_rss_mib | 1.056 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_turn_rss_mib | 1.056 | cross-batch |
| prior_pi | live-subscriber | large | measured_turn_write_bytes | 2.931 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_bytes | 4.198 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_mib | 4.198 | cross-batch |
| prior_pi | live-subscriber | large | turn_write_bytes_per_turn | 2.931 | cross-batch |
| prior_pi | live-subscriber | tool | closed_allocated_bytes | 17.406 | cross-batch |
| prior_pi | live-subscriber | tool | closed_logical_bytes | 17.285 | cross-batch |
| prior_pi | live-subscriber | tool | lifecycle_ms | 1.145 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_pss_mib | 1.374 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_turn_rss_mib | 1.061 | cross-batch |
| prior_pi | live-subscriber | tool | measured_turn_write_bytes | 2.589 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_bytes | 3.737 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_mib | 3.737 | cross-batch |
| prior_pi | live-subscriber | tool | turn_write_bytes_per_turn | 2.589 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_allocated_bytes | 12.700 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_logical_bytes | 12.569 | cross-batch |
| prior_pi | live-subscriber | cancel | lifecycle_ms | 1.159 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_pss_mib | 1.395 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_turn_rss_mib | 1.042 | cross-batch |
| prior_pi | live-subscriber | cancel | measured_turn_write_bytes | 3.015 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_bytes | 4.403 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_mib | 4.403 | cross-batch |
| prior_pi | live-subscriber | cancel | turn_write_bytes_per_turn | 3.015 | cross-batch |
| prior_deepseek | live-subscriber | short-small | mean_turn_ms | 1.179 | cross-batch |
| prior_deepseek | live-subscriber | short-small | median_measured_turn_ms | 1.141 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_cpu_ms_per_turn | 1.362 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_write_bytes_per_turn | 43.911 | cross-batch |
| prior_deepseek | live-subscriber | short-large | mean_turn_ms | 1.240 | cross-batch |
| prior_deepseek | live-subscriber | short-large | median_measured_turn_ms | 1.261 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_cpu_ms_per_turn | 1.500 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_write_bytes_per_turn | 53.000 | cross-batch |
| prior_deepseek | live-subscriber | stream | mean_turn_ms | 1.008 | cross-batch |
| prior_deepseek | live-subscriber | stream | median_measured_turn_ms | 1.008 | cross-batch |
| prior_deepseek | live-subscriber | stream | turn_write_bytes_per_turn | 45.429 | cross-batch |
| prior_deepseek | live-subscriber | large | turn_write_bytes_per_turn | 30.143 | cross-batch |

- Only the fresh old/new Rust products are interleaved pairs. Pi/DeepSeek use fixed previous batches and separate descriptive ratios of medians; no causal parity or paired confidence claim.
- /proc CPU counters use 100 ticks/s; each cumulative endpoint has 10 ms granularity. This is not profiler sampling.
- A product measured-turn median and a product measured-turn mean are different estimands. Across products we report median and range of those statistics, not pooled turns.
- DeepSeek SDK idle does not await flush/fsync. Native wait4 CPU covers exit/flush and verification separately; full process write_bytes is unavailable, not zero. No equivalent lifecycle ratio is constructed.
- All recorded adverse pairs and all unavailable ratios are retained, including reversals hidden by a favorable group median.

Unavailable ratio records, all per-product values, and reference hashes are retained in report-data.json.
