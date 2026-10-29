# Repeated work removal performance evidence

Fresh Rust comparison: five interleaved pairs per cell. Historical Pi/DeepSeek rows are fixed, separate batches; their ratios are ratios of medians, not paired results.

| Mode | Case | Paired median wall % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -0.107 [-14.197,80.414] | -3.053 | -2.176 | 0.539 | 0.190 |
| no-live-subscriber | short-large | 5.827 [-50.590,81.719] | 0.495 | -1.694 | -0.060 | 0.018 |
| no-live-subscriber | stream | 0.556 [0.133,4.058] | 7.143 | -0.732 | -0.352 | -0.377 |
| no-live-subscriber | large | 4.277 [-18.028,18.374] | 0.000 | -0.831 | -0.070 | -0.133 |
| no-live-subscriber | tool | -4.636 [-21.339,25.538] | 0.000 | -1.449 | 0.100 | 0.111 |
| no-live-subscriber | cancel | -4.619 [-18.993,8.161] | 0.000 | -0.857 | -0.027 | 0.147 |
| live-subscriber | short-small | 4.643 [-48.043,85.280] | 8.511 | -2.187 | -0.003 | -0.143 |
| live-subscriber | short-large | -2.324 [-5.029,5.791] | -0.833 | -1.669 | 0.476 | 0.169 |
| live-subscriber | stream | 0.096 [-0.112,2.861] | 3.571 | -0.732 | 0.251 | -0.181 |
| live-subscriber | large | 9.117 [-2.906,34.343] | 0.000 | -0.831 | 0.106 | -0.003 |
| live-subscriber | tool | 4.676 [-1.749,23.238] | 5.000 | -1.449 | -0.256 | 0.000 |
| live-subscriber | cancel | 7.077 [-9.336,19.320] | 7.143 | -0.857 | 0.421 | -0.157 |

| Mode | Case | Mean wall / old Pi | Median wall / old Pi | Full CPU / old Pi | Full writes / old Pi |
|---|---|---:|---:|---:|---:|
| no-live-subscriber | short-small | 4.199 | 4.879 | 1.351 | 4.056 |
| no-live-subscriber | short-large | 4.638 | 5.251 | 2.879 | 4.551 |
| no-live-subscriber | stream | 0.922 | 0.922 | 0.264 | 3.700 |
| no-live-subscriber | large | 0.465 | 0.465 | 0.271 | 4.163 |
| no-live-subscriber | tool | 0.408 | 0.408 | 0.270 | 3.683 |
| no-live-subscriber | cancel | 0.322 | 0.322 | 0.246 | 4.365 |
| live-subscriber | short-small | 4.444 | 5.038 | 1.567 | 4.055 |
| live-subscriber | short-large | 5.173 | 5.538 | 3.107 | 4.551 |
| live-subscriber | stream | 0.917 | 0.917 | 0.261 | 3.700 |
| live-subscriber | large | 0.441 | 0.441 | 0.305 | 4.163 |
| live-subscriber | tool | 0.542 | 0.542 | 0.317 | 3.683 |
| live-subscriber | cancel | 0.325 | 0.325 | 0.250 | 4.365 |

Warm measured windows (per-product statistic, then median [minimum,maximum] across five Rust/Pi products; each has 32 measured turns):

| Mode | History | Engine/batch | Mean ms/turn | Median ms/turn | CPU ms/turn | Write bytes/turn |
|---|---:|---|---:|---:|---:|---:|
| no-live-subscriber | 8 | current Rust | 15.225 [12.303,19.665] | 15.296 [12.490,16.158] | 17.188 [14.062,19.688] | 923392.000 [923008.000,923520.000] |
| no-live-subscriber | 8 | fixed old Pi | 3.625 [3.524,4.546] | 3.135 [3.014,3.328] | 7.188 [6.875,9.688] | 224512.000 [224512.000,224512.000] |
| no-live-subscriber | 64 | current Rust | 17.146 [9.647,19.142] | 17.322 [8.873,18.562] | 19.062 [11.250,21.562] | 1109888.000 [1108480.000,1110016.000] |
| no-live-subscriber | 64 | fixed old Pi | 3.697 [3.667,3.911] | 3.299 [3.159,3.524] | 6.250 [5.938,6.875] | 224384.000 [224384.000,224384.000] |
| live-subscriber | 8 | current Rust | 17.479 [9.791,18.020] | 17.230 [9.046,17.936] | 23.438 [14.062,24.688] | 923392.000 [923008.000,923904.000] |
| live-subscriber | 8 | fixed old Pi | 3.934 [3.784,4.165] | 3.420 [3.309,3.699] | 7.500 [7.188,8.438] | 224512.000 [224512.000,224512.000] |
| live-subscriber | 64 | current Rust | 20.634 [18.521,21.450] | 19.607 [18.974,20.424] | 27.812 [25.000,29.375] | 1109120.000 [1076736.000,1110272.000] |
| live-subscriber | 64 | fixed old Pi | 3.989 [3.827,4.233] | 3.541 [3.482,3.679] | 6.875 [6.250,7.188] | 224384.000 [224384.000,224384.000] |

| Live case | Current mean ms/turn | Fixed DS mean ms/turn | Wall ratio | CPU ratio | Measured write ratio |
|---|---:|---:|---:|---:|---:|
| short-small | 17.479 | 15.541 | 1.125 | 1.293 | 42.940 |
| short-large | 20.634 | 15.450 | 1.336 | 1.589 | 51.886 |
| stream | 1591.249 | 1583.218 | 1.005 | 0.533 | 45.000 |
| large | 39.678 | 165.613 | 0.240 | 0.250 | 29.714 |

DeepSeek exit/flush CPU remains separate from measured turns and from Rust full-lifecycle CPU; full DeepSeek process write bytes remain unavailable. The JSON retains the exact wait4 metrics and source-specific limits.

All adverse metrics (including individual fresh paired reversals):

| Comparison | Mode | Case | Metric | Median % or cross-batch ratio | Adverse pairs |
|---|---|---|---|---:|---|
| fresh_paired_rust | no-live-subscriber | short-small | mean_turn_ms | -3.826 | 1:+78.083%,3:+11.284% |
| fresh_paired_rust | no-live-subscriber | short-small | turn_cpu_ms_per_turn | -4.545 | 1:+67.647%,3:+7.143% |
| fresh_paired_rust | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | -3.053 | 1:+31.313%,3:+13.889% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_ms | -2.724 | 1:+28.166%,3:+14.208% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_anon_pss_mib | 0.539 | 0:+0.539%,1:+0.108%,2:+1.173%,3:+0.806% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_pss_mib | 0.171 | 0:+0.171%,2:+0.430%,3:+0.175% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_rss_mib | 0.190 | 0:+0.190%,2:+0.426%,3:+0.207% |
| fresh_paired_rust | no-live-subscriber | short-small | median_measured_turn_ms | -0.107 | 1:+80.414%,3:+41.290% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_self_cpu_ms | -3.053 | 1:+31.313%,3:+13.889% |
| fresh_paired_rust | no-live-subscriber | short-small | measured_turn_cpu_ms | -4.545 | 1:+67.647%,3:+7.143% |
| fresh_paired_rust | no-live-subscriber | short-small | through_product_exit_wall_ms | -2.511 | 1:+27.066%,3:+14.420% |
| fresh_paired_rust | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -2.575 | 1:+26.225%,3:+14.639% |
| fresh_paired_rust | no-live-subscriber | short-small | closed_logical_bytes | 0.763 | 0:+0.822%,1:+0.763%,2:+0.763%,3:+0.763%,4:+0.822% |
| fresh_paired_rust | no-live-subscriber | short-small | closed_allocated_bytes | 0.757 | 0:+0.815%,1:+0.757%,2:+0.757%,3:+0.757%,4:+0.815% |
| fresh_paired_rust | no-live-subscriber | short-large | mean_turn_ms | 10.943 | 0:+10.943%,1:+41.562%,4:+23.398% |
| fresh_paired_rust | no-live-subscriber | short-large | turn_cpu_ms_per_turn | 9.677 | 0:+9.677%,1:+38.636%,4:+23.214% |
| fresh_paired_rust | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 0.495 | 0:+0.495%,1:+20.117%,4:+3.518% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_ms | -0.386 | 1:+19.419%,4:+3.517% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_anon_pss_mib | -0.060 | 2:+0.429%,3:+0.044% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_pss_mib | 0.009 | 0:+0.009%,2:+0.127%,3:+0.042% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_rss_mib | 0.018 | 0:+0.018%,2:+0.124%,3:+0.054% |
| fresh_paired_rust | no-live-subscriber | short-large | median_measured_turn_ms | 5.827 | 0:+5.827%,1:+81.719%,4:+18.659% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_self_cpu_ms | 0.495 | 0:+0.495%,1:+20.117%,4:+3.518% |
| fresh_paired_rust | no-live-subscriber | short-large | measured_turn_cpu_ms | 9.677 | 0:+9.677%,1:+38.636%,4:+23.214% |
| fresh_paired_rust | no-live-subscriber | short-large | through_product_exit_wall_ms | -0.431 | 1:+18.326%,4:+3.266% |
| fresh_paired_rust | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | -0.586 | 1:+17.430%,4:+3.397% |
| fresh_paired_rust | no-live-subscriber | stream | mean_turn_ms | 0.556 | 0:+0.593%,1:+4.058%,2:+0.556%,3:+0.133%,4:+0.265% |
| fresh_paired_rust | no-live-subscriber | stream | turn_cpu_ms_per_turn | 0.000 | 0:+15.385%,1:+33.333% |
| fresh_paired_rust | no-live-subscriber | stream | through_cleanup_self_cpu_ms | 7.143 | 0:+12.000%,1:+20.833%,3:+7.143% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_ms | 1.242 | 0:+1.736%,1:+3.035%,2:+0.809%,3:+1.242%,4:+0.011% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_anon_pss_mib | -0.352 | 0:+0.748%,4:+0.347% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_pss_mib | -0.378 | 0:+0.342% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_rss_mib | -0.377 | 0:+0.329% |
| fresh_paired_rust | no-live-subscriber | stream | median_measured_turn_ms | 0.556 | 0:+0.593%,1:+4.058%,2:+0.556%,3:+0.133%,4:+0.265% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_self_cpu_ms | 7.143 | 0:+12.000%,1:+20.833%,3:+7.143% |
| fresh_paired_rust | no-live-subscriber | stream | measured_turn_cpu_ms | 0.000 | 0:+15.385%,1:+33.333% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_turn_rss_mib | -0.465 | 0:+0.022% |
| fresh_paired_rust | no-live-subscriber | stream | through_product_exit_wall_ms | 1.276 | 0:+1.730%,1:+2.993%,2:+0.853%,3:+1.276% |
| fresh_paired_rust | no-live-subscriber | stream | complete_harness_lifecycle_wall_ms | 1.595 | 0:+1.724%,1:+3.353%,2:+0.771%,3:+1.595%,4:+0.001% |
| fresh_paired_rust | no-live-subscriber | large | mean_turn_ms | 4.277 | 2:+8.540%,3:+4.277%,4:+18.374% |
| fresh_paired_rust | no-live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 2:+100.000%,4:+25.000% |
| fresh_paired_rust | no-live-subscriber | large | through_cleanup_self_cpu_ms | 0.000 | 2:+7.143% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_ms | 2.236 | 0:+3.946%,2:+2.236%,3:+0.417%,4:+3.979% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_anon_pss_mib | -0.070 | 1:+0.330%,4:+0.408% |
| fresh_paired_rust | no-live-subscriber | large | median_measured_turn_ms | 4.277 | 2:+8.540%,3:+4.277%,4:+18.374% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_self_cpu_ms | 0.000 | 2:+7.143% |
| fresh_paired_rust | no-live-subscriber | large | measured_turn_cpu_ms | 0.000 | 2:+100.000%,4:+25.000% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_turn_rss_mib | -0.534 | 4:+0.179% |
| fresh_paired_rust | no-live-subscriber | large | through_product_exit_wall_ms | 1.415 | 0:+3.779%,2:+1.415%,4:+4.049% |
| fresh_paired_rust | no-live-subscriber | large | complete_harness_lifecycle_wall_ms | 1.171 | 0:+6.180%,2:+1.171%,4:+2.667% |
| fresh_paired_rust | no-live-subscriber | tool | mean_turn_ms | -4.636 | 0:+25.538%,2:+15.549% |
| fresh_paired_rust | no-live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 0:+25.000% |
| fresh_paired_rust | no-live-subscriber | tool | through_cleanup_self_cpu_ms | 0.000 | 0:+12.500%,2:+6.250% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_ms | -1.568 | 0:+10.939%,2:+8.644% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_anon_pss_mib | 0.100 | 0:+0.206%,1:+0.100%,2:+1.220%,4:+0.088% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_pss_mib | 0.107 | 0:+0.397%,2:+0.626%,3:+0.107% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_rss_mib | 0.111 | 0:+0.380%,2:+0.628%,3:+0.111% |
| fresh_paired_rust | no-live-subscriber | tool | median_measured_turn_ms | -4.636 | 0:+25.538%,2:+15.549% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_self_cpu_ms | 0.000 | 0:+12.500%,2:+6.250% |
| fresh_paired_rust | no-live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 0:+25.000% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_turn_rss_mib | 0.003 | 1:+0.003%,2:+0.106%,4:+0.263% |
| fresh_paired_rust | no-live-subscriber | tool | through_product_exit_wall_ms | -1.690 | 0:+11.567%,2:+10.246% |
| fresh_paired_rust | no-live-subscriber | tool | complete_harness_lifecycle_wall_ms | -1.514 | 0:+11.763%,2:+11.415% |
| fresh_paired_rust | no-live-subscriber | cancel | mean_turn_ms | -4.619 | 3:+8.161% |
| fresh_paired_rust | no-live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 3:+7.143% |
| fresh_paired_rust | no-live-subscriber | cancel | lifecycle_ms | -2.880 | 0:+0.621%,3:+1.027% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_anon_pss_mib | -0.027 | 1:+0.529%,2:+1.488% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_pss_mib | 0.127 | 1:+0.338%,2:+0.567%,3:+0.127% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_rss_mib | 0.147 | 1:+0.343%,2:+0.556%,3:+0.147% |
| fresh_paired_rust | no-live-subscriber | cancel | median_measured_turn_ms | -4.619 | 3:+8.161% |
| fresh_paired_rust | no-live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 3:+7.143% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_turn_rss_mib | 0.151 | 1:+0.374%,2:+0.671%,3:+0.151% |
| fresh_paired_rust | no-live-subscriber | cancel | through_product_exit_wall_ms | -1.482 | 0:+0.636%,3:+0.746% |
| fresh_paired_rust | no-live-subscriber | cancel | complete_harness_lifecycle_wall_ms | -1.705 | 3:+1.403% |
| fresh_paired_rust | live-subscriber | short-small | mean_turn_ms | 7.509 | 0:+7.509%,2:+65.110%,4:+72.711% |
| fresh_paired_rust | live-subscriber | short-small | turn_cpu_ms_per_turn | 7.042 | 0:+7.042%,2:+61.224%,4:+63.043% |
| fresh_paired_rust | live-subscriber | short-small | through_cleanup_self_cpu_ms | 8.511 | 0:+8.511%,2:+32.203%,4:+33.333% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_ms | 7.161 | 0:+7.161%,2:+25.844%,4:+26.605% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_anon_pss_mib | -0.003 | 0:+0.012%,3:+0.131% |
| fresh_paired_rust | live-subscriber | short-small | median_measured_turn_ms | 4.643 | 0:+4.643%,2:+78.365%,4:+85.280% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_self_cpu_ms | 8.511 | 0:+8.511%,2:+32.203%,4:+33.333% |
| fresh_paired_rust | live-subscriber | short-small | measured_turn_cpu_ms | 7.042 | 0:+7.042%,2:+61.224%,4:+63.043% |
| fresh_paired_rust | live-subscriber | short-small | through_product_exit_wall_ms | 6.443 | 0:+6.443%,2:+24.720%,4:+24.659% |
| fresh_paired_rust | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 4.937 | 0:+4.937%,2:+23.631%,4:+22.511% |
| fresh_paired_rust | live-subscriber | short-small | closed_logical_bytes | 0.822 | 0:+0.822%,1:+2.582%,2:+0.704%,3:+0.822%,4:+0.822% |
| fresh_paired_rust | live-subscriber | short-small | closed_allocated_bytes | 0.815 | 0:+0.815%,1:+2.563%,2:+0.699%,3:+0.815%,4:+0.815% |
| fresh_paired_rust | live-subscriber | short-large | mean_turn_ms | -2.039 | 2:+12.038%,3:+6.141% |
| fresh_paired_rust | live-subscriber | short-large | turn_cpu_ms_per_turn | -3.261 | 2:+13.253%,3:+4.651% |
| fresh_paired_rust | live-subscriber | short-large | through_cleanup_self_cpu_ms | -0.833 | 0:+0.649%,2:+4.338% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_ms | -1.052 | 2:+3.431% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_anon_pss_mib | 0.476 | 0:+0.024%,2:+0.716%,3:+0.511%,4:+0.476% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_pss_mib | 0.172 | 2:+0.476%,3:+0.172%,4:+0.216% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_rss_mib | 0.169 | 2:+0.488%,3:+0.169%,4:+0.214% |
| fresh_paired_rust | live-subscriber | short-large | median_measured_turn_ms | -2.324 | 2:+5.791%,4:+0.471% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_self_cpu_ms | -0.833 | 0:+0.649%,2:+4.338% |
| fresh_paired_rust | live-subscriber | short-large | measured_turn_cpu_ms | -3.261 | 2:+13.253%,3:+4.651% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_turn_rss_mib | -0.004 | 2:+0.693%,4:+0.072% |
| fresh_paired_rust | live-subscriber | short-large | through_product_exit_wall_ms | -0.969 | 2:+3.569% |
| fresh_paired_rust | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | -0.942 | 2:+3.668% |
| fresh_paired_rust | live-subscriber | stream | mean_turn_ms | 0.096 | 1:+2.861%,2:+0.096%,4:+0.883% |
| fresh_paired_rust | live-subscriber | stream | turn_cpu_ms_per_turn | 0.000 | 1:+13.333%,4:+6.667% |
| fresh_paired_rust | live-subscriber | stream | through_cleanup_self_cpu_ms | 3.571 | 0:+3.571%,1:+10.345%,2:+3.571%,3:+3.704% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_ms | 1.450 | 0:+0.589%,1:+2.505%,2:+1.450%,3:+0.058%,4:+1.578% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_anon_pss_mib | 0.251 | 0:+0.251%,2:+0.128%,3:+2.450%,4:+0.611% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_pss_mib | -0.136 | 3:+1.328%,4:+0.171% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_rss_mib | -0.181 | 3:+1.266%,4:+0.176% |
| fresh_paired_rust | live-subscriber | stream | median_measured_turn_ms | 0.096 | 1:+2.861%,2:+0.096%,4:+0.883% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_self_cpu_ms | 3.571 | 0:+3.571%,1:+10.345%,2:+3.571%,3:+3.704% |
| fresh_paired_rust | live-subscriber | stream | measured_turn_cpu_ms | 0.000 | 1:+13.333%,4:+6.667% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_turn_rss_mib | -0.245 | 3:+1.105%,4:+0.107% |
| fresh_paired_rust | live-subscriber | stream | through_product_exit_wall_ms | 1.418 | 0:+0.629%,1:+2.090%,2:+1.660%,3:+0.184%,4:+1.418% |
| fresh_paired_rust | live-subscriber | stream | complete_harness_lifecycle_wall_ms | 0.977 | 0:+0.111%,1:+1.294%,2:+2.068%,3:+0.771%,4:+0.977% |
| fresh_paired_rust | live-subscriber | large | mean_turn_ms | 9.117 | 0:+9.117%,1:+17.121%,2:+34.343%,4:+6.241% |
| fresh_paired_rust | live-subscriber | large | turn_cpu_ms_per_turn | 25.000 | 0:+25.000%,2:+75.000%,3:+25.000%,4:+25.000% |
| fresh_paired_rust | live-subscriber | large | through_cleanup_self_cpu_ms | 0.000 | 2:+26.667%,4:+20.000% |
| fresh_paired_rust | live-subscriber | large | lifecycle_ms | -1.136 | 2:+9.732%,4:+9.533% |
| fresh_paired_rust | live-subscriber | large | max_sampled_anon_pss_mib | 0.106 | 0:+0.540%,2:+0.232%,3:+0.106% |
| fresh_paired_rust | live-subscriber | large | max_sampled_pss_mib | 0.000 | 0:+0.174%,2:+0.027% |
| fresh_paired_rust | live-subscriber | large | max_sampled_rss_mib | -0.003 | 0:+0.215%,2:+0.042% |
| fresh_paired_rust | live-subscriber | large | median_measured_turn_ms | 9.117 | 0:+9.117%,1:+17.121%,2:+34.343%,4:+6.241% |
| fresh_paired_rust | live-subscriber | large | lifecycle_self_cpu_ms | 0.000 | 2:+26.667%,4:+20.000% |
| fresh_paired_rust | live-subscriber | large | measured_turn_cpu_ms | 25.000 | 0:+25.000%,2:+75.000%,3:+25.000%,4:+25.000% |
| fresh_paired_rust | live-subscriber | large | max_sampled_turn_rss_mib | -0.234 | 0:+0.204% |
| fresh_paired_rust | live-subscriber | large | through_product_exit_wall_ms | 1.466 | 0:+1.466%,2:+9.321%,4:+9.465% |
| fresh_paired_rust | live-subscriber | large | complete_harness_lifecycle_wall_ms | 2.734 | 0:+2.734%,2:+6.977%,4:+8.250% |
| fresh_paired_rust | live-subscriber | tool | mean_turn_ms | 4.676 | 0:+4.676%,1:+3.351%,3:+10.502%,4:+23.238% |
| fresh_paired_rust | live-subscriber | tool | turn_cpu_ms_per_turn | 14.286 | 2:+14.286%,3:+40.000%,4:+40.000% |
| fresh_paired_rust | live-subscriber | tool | through_cleanup_self_cpu_ms | 5.000 | 2:+5.000%,3:+11.111%,4:+18.750% |
| fresh_paired_rust | live-subscriber | tool | lifecycle_ms | 1.402 | 0:+1.402%,2:+4.843%,3:+1.212%,4:+7.561% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_anon_pss_mib | -0.256 | 4:+0.948% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_pss_mib | -0.034 | 4:+0.677% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_rss_mib | 0.000 | 2:+0.016%,4:+0.654% |
| fresh_paired_rust | live-subscriber | tool | median_measured_turn_ms | 4.676 | 0:+4.676%,1:+3.351%,3:+10.502%,4:+23.238% |
| fresh_paired_rust | live-subscriber | tool | lifecycle_self_cpu_ms | 5.000 | 2:+5.000%,3:+11.111%,4:+18.750% |
| fresh_paired_rust | live-subscriber | tool | measured_turn_cpu_ms | 14.286 | 2:+14.286%,3:+40.000%,4:+40.000% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_turn_rss_mib | 0.159 | 0:+0.083%,2:+0.159%,3:+0.255%,4:+0.439% |
| fresh_paired_rust | live-subscriber | tool | through_product_exit_wall_ms | 2.844 | 0:+2.844%,2:+4.600%,4:+8.504% |
| fresh_paired_rust | live-subscriber | tool | complete_harness_lifecycle_wall_ms | 2.198 | 0:+2.198%,1:+1.536%,2:+4.001%,3:+0.444%,4:+9.609% |
| fresh_paired_rust | live-subscriber | cancel | mean_turn_ms | 7.077 | 1:+5.837%,2:+7.077%,3:+19.320%,4:+8.051% |
| fresh_paired_rust | live-subscriber | cancel | turn_cpu_ms_per_turn | 33.333 | 2:+33.333%,3:+33.333%,4:+33.333% |
| fresh_paired_rust | live-subscriber | cancel | through_cleanup_self_cpu_ms | 7.143 | 2:+7.143%,3:+13.333%,4:+13.333% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_ms | 1.476 | 1:+1.476%,3:+6.841%,4:+10.930% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_anon_pss_mib | 0.421 | 1:+0.004%,2:+0.421%,3:+0.590%,4:+1.637% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_pss_mib | -0.157 | 3:+0.376%,4:+0.615% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_rss_mib | -0.157 | 3:+0.399%,4:+0.616% |
| fresh_paired_rust | live-subscriber | cancel | median_measured_turn_ms | 7.077 | 1:+5.837%,2:+7.077%,3:+19.320%,4:+8.051% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_self_cpu_ms | 7.143 | 2:+7.143%,3:+13.333%,4:+13.333% |
| fresh_paired_rust | live-subscriber | cancel | measured_turn_cpu_ms | 33.333 | 2:+33.333%,3:+33.333%,4:+33.333% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_turn_rss_mib | -0.223 | 3:+0.079%,4:+0.495% |
| fresh_paired_rust | live-subscriber | cancel | through_product_exit_wall_ms | 0.562 | 0:+0.562%,1:+0.475%,3:+7.131%,4:+10.095% |
| fresh_paired_rust | live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 1.405 | 0:+1.405%,1:+0.370%,3:+7.573%,4:+10.090% |
| prior_pi | no-live-subscriber | short-small | closed_allocated_bytes | 41.190 | cross-batch |
| prior_pi | no-live-subscriber | short-small | closed_logical_bytes | 41.096 | cross-batch |
| prior_pi | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 1.909 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_ms | 2.937 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_self_cpu_ms | 2.490 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_anon_pss_mib | 1.125 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_pss_mib | 1.481 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_rss_mib | 1.082 | cross-batch |
| prior_pi | no-live-subscriber | short-small | mean_turn_ms | 4.199 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_cpu_ms | 2.391 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_write_bytes | 4.113 | cross-batch |
| prior_pi | no-live-subscriber | short-small | median_measured_turn_ms | 4.879 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_bytes | 4.056 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_mib | 4.056 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.351 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_product_exit_wall_ms | 1.920 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_cpu_ms_per_turn | 2.391 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_write_bytes_per_turn | 4.113 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_allocated_bytes | 41.133 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_logical_bytes | 41.069 | cross-batch |
| prior_pi | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.599 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_ms | 4.840 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_self_cpu_ms | 4.101 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_anon_pss_mib | 1.274 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_pss_mib | 1.518 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_rss_mib | 1.203 | cross-batch |
| prior_pi | no-live-subscriber | short-large | mean_turn_ms | 4.638 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_cpu_ms | 3.050 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_write_bytes | 4.946 | cross-batch |
| prior_pi | no-live-subscriber | short-large | median_measured_turn_ms | 5.251 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_bytes | 4.551 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_mib | 4.551 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 2.879 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_product_exit_wall_ms | 3.712 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_cpu_ms_per_turn | 3.050 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_write_bytes_per_turn | 4.946 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_allocated_bytes | 15.406 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_logical_bytes | 15.304 | cross-batch |
| prior_pi | no-live-subscriber | stream | max_sampled_pss_mib | 1.255 | cross-batch |
| prior_pi | no-live-subscriber | stream | measured_turn_write_bytes | 2.500 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_bytes | 3.700 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_mib | 3.700 | cross-batch |
| prior_pi | no-live-subscriber | stream | turn_write_bytes_per_turn | 2.500 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_allocated_bytes | 10.158 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_logical_bytes | 10.056 | cross-batch |
| prior_pi | no-live-subscriber | large | lifecycle_ms | 1.302 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_anon_pss_mib | 1.088 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_pss_mib | 1.555 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_rss_mib | 1.051 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_turn_rss_mib | 1.046 | cross-batch |
| prior_pi | no-live-subscriber | large | measured_turn_write_bytes | 2.889 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_bytes | 4.163 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_mib | 4.163 | cross-batch |
| prior_pi | no-live-subscriber | large | turn_write_bytes_per_turn | 2.889 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_allocated_bytes | 17.062 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_logical_bytes | 16.939 | cross-batch |
| prior_pi | no-live-subscriber | tool | lifecycle_ms | 1.160 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_pss_mib | 1.384 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_turn_rss_mib | 1.063 | cross-batch |
| prior_pi | no-live-subscriber | tool | measured_turn_write_bytes | 2.521 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_bytes | 3.683 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_mib | 3.683 | cross-batch |
| prior_pi | no-live-subscriber | tool | turn_write_bytes_per_turn | 2.521 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_allocated_bytes | 12.567 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_logical_bytes | 12.435 | cross-batch |
| prior_pi | no-live-subscriber | cancel | lifecycle_ms | 1.119 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_pss_mib | 1.403 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_turn_rss_mib | 1.039 | cross-batch |
| prior_pi | no-live-subscriber | cancel | measured_turn_write_bytes | 2.970 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_bytes | 4.365 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_mib | 4.365 | cross-batch |
| prior_pi | no-live-subscriber | cancel | turn_write_bytes_per_turn | 2.970 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_allocated_bytes | 41.214 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_logical_bytes | 41.120 | cross-batch |
| prior_pi | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 1.991 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_ms | 3.050 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_self_cpu_ms | 2.815 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_anon_pss_mib | 1.192 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_pss_mib | 1.550 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_rss_mib | 1.126 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_turn_rss_mib | 1.013 | cross-batch |
| prior_pi | live-subscriber | short-small | mean_turn_ms | 4.444 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_cpu_ms | 3.125 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_write_bytes | 4.113 | cross-batch |
| prior_pi | live-subscriber | short-small | median_measured_turn_ms | 5.038 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_bytes | 4.055 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_mib | 4.055 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.567 | cross-batch |
| prior_pi | live-subscriber | short-small | through_product_exit_wall_ms | 2.004 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_cpu_ms_per_turn | 3.125 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_write_bytes_per_turn | 4.113 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_allocated_bytes | 41.150 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_logical_bytes | 41.085 | cross-batch |
| prior_pi | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.617 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_ms | 4.826 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_self_cpu_ms | 4.396 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_anon_pss_mib | 1.282 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_pss_mib | 1.520 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_rss_mib | 1.208 | cross-batch |
| prior_pi | live-subscriber | short-large | mean_turn_ms | 5.173 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_cpu_ms | 4.045 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_write_bytes | 4.943 | cross-batch |
| prior_pi | live-subscriber | short-large | median_measured_turn_ms | 5.538 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_bytes | 4.551 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_mib | 4.551 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_self_cpu_ms | 3.107 | cross-batch |
| prior_pi | live-subscriber | short-large | through_product_exit_wall_ms | 3.714 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_cpu_ms_per_turn | 4.045 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_write_bytes_per_turn | 4.943 | cross-batch |
| prior_pi | live-subscriber | stream | closed_allocated_bytes | 15.406 | cross-batch |
| prior_pi | live-subscriber | stream | closed_logical_bytes | 15.304 | cross-batch |
| prior_pi | live-subscriber | stream | max_sampled_pss_mib | 1.263 | cross-batch |
| prior_pi | live-subscriber | stream | measured_turn_write_bytes | 2.500 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_bytes | 3.700 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_mib | 3.700 | cross-batch |
| prior_pi | live-subscriber | stream | turn_write_bytes_per_turn | 2.500 | cross-batch |
| prior_pi | live-subscriber | large | closed_allocated_bytes | 10.158 | cross-batch |
| prior_pi | live-subscriber | large | closed_logical_bytes | 10.056 | cross-batch |
| prior_pi | live-subscriber | large | lifecycle_ms | 1.216 | cross-batch |
| prior_pi | live-subscriber | large | lifecycle_self_cpu_ms | 1.125 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_anon_pss_mib | 1.094 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_pss_mib | 1.564 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_rss_mib | 1.056 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_turn_rss_mib | 1.054 | cross-batch |
| prior_pi | live-subscriber | large | measured_turn_write_bytes | 2.889 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_bytes | 4.163 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_mib | 4.163 | cross-batch |
| prior_pi | live-subscriber | large | turn_write_bytes_per_turn | 2.889 | cross-batch |
| prior_pi | live-subscriber | tool | closed_allocated_bytes | 17.062 | cross-batch |
| prior_pi | live-subscriber | tool | closed_logical_bytes | 16.939 | cross-batch |
| prior_pi | live-subscriber | tool | lifecycle_ms | 1.226 | cross-batch |
| prior_pi | live-subscriber | tool | lifecycle_self_cpu_ms | 1.053 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_pss_mib | 1.376 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_turn_rss_mib | 1.061 | cross-batch |
| prior_pi | live-subscriber | tool | measured_turn_write_bytes | 2.521 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_bytes | 3.683 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_mib | 3.683 | cross-batch |
| prior_pi | live-subscriber | tool | turn_write_bytes_per_turn | 2.521 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_allocated_bytes | 12.567 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_logical_bytes | 12.435 | cross-batch |
| prior_pi | live-subscriber | cancel | lifecycle_ms | 1.111 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_pss_mib | 1.397 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_turn_rss_mib | 1.039 | cross-batch |
| prior_pi | live-subscriber | cancel | measured_turn_write_bytes | 2.970 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_bytes | 4.365 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_mib | 4.365 | cross-batch |
| prior_pi | live-subscriber | cancel | turn_write_bytes_per_turn | 2.970 | cross-batch |
| prior_deepseek | live-subscriber | short-small | mean_turn_ms | 1.125 | cross-batch |
| prior_deepseek | live-subscriber | short-small | median_measured_turn_ms | 1.166 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_cpu_ms_per_turn | 1.293 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_write_bytes_per_turn | 42.940 | cross-batch |
| prior_deepseek | live-subscriber | short-large | mean_turn_ms | 1.336 | cross-batch |
| prior_deepseek | live-subscriber | short-large | median_measured_turn_ms | 1.278 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_cpu_ms_per_turn | 1.589 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_write_bytes_per_turn | 51.886 | cross-batch |
| prior_deepseek | live-subscriber | stream | mean_turn_ms | 1.005 | cross-batch |
| prior_deepseek | live-subscriber | stream | median_measured_turn_ms | 1.005 | cross-batch |
| prior_deepseek | live-subscriber | stream | turn_write_bytes_per_turn | 45.000 | cross-batch |
| prior_deepseek | live-subscriber | large | turn_write_bytes_per_turn | 29.714 | cross-batch |

- Only the fresh old/new Rust products are interleaved pairs. Pi/DeepSeek use fixed previous batches and separate descriptive ratios of medians; no causal parity or paired confidence claim.
- /proc CPU counters use 100 ticks/s; each cumulative endpoint has 10 ms granularity. This is not profiler sampling.
- A product measured-turn median and a product measured-turn mean are different estimands. Across products we report median and range of those statistics, not pooled turns.
- DeepSeek SDK idle does not await flush/fsync. Native wait4 CPU covers exit/flush and verification separately; full process write_bytes is unavailable, not zero. No equivalent lifecycle ratio is constructed.
- All recorded adverse pairs and all unavailable ratios are retained, including reversals hidden by a favorable group median.

Unavailable ratio records, all per-product values, and reference hashes are retained in report-data.json.
