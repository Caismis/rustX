# Final cost closure performance evidence

Fresh Rust comparison: five interleaved pairs per cell. Historical Pi/DeepSeek rows are fixed, separate batches; their ratios are ratios of medians, not paired results.

| Mode | Case | Paired median wall % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -4.198 [-51.622,68.623] | -0.769 | -10.398 | 1.167 | 1.079 |
| no-live-subscriber | short-large | 0.993 [-48.274,34.775] | 0.000 | -9.495 | 0.846 | 0.930 |
| no-live-subscriber | stream | 0.179 [-2.704,1.481] | 0.000 | -4.308 | 0.540 | 0.588 |
| no-live-subscriber | large | -2.712 [-10.568,7.844] | -5.882 | -4.481 | 1.081 | 1.047 |
| no-live-subscriber | tool | 7.564 [-19.300,19.661] | 5.882 | -6.993 | 0.819 | 0.969 |
| no-live-subscriber | cancel | -1.411 [-18.295,24.641] | 0.000 | -4.485 | 1.228 | 0.769 |
| live-subscriber | short-small | -5.446 [-18.243,-3.643] | -7.692 | -10.382 | 0.871 | 0.783 |
| live-subscriber | short-large | -51.607 [-53.406,59.767] | -10.940 | -9.532 | 0.379 | 0.304 |
| live-subscriber | stream | -0.595 [-1.919,1.396] | -6.897 | -4.308 | 1.182 | 0.834 |
| live-subscriber | large | 3.428 [-12.517,68.124] | 6.250 | -4.481 | 0.521 | 0.728 |
| live-subscriber | tool | -2.625 [-28.555,11.085] | 0.000 | -6.993 | 0.488 | 0.813 |
| live-subscriber | cancel | -7.234 [-9.945,10.730] | -5.882 | -4.485 | 1.221 | 0.988 |

| Mode | Case | Mean wall / old Pi | Median wall / old Pi | Full CPU / old Pi | Full writes / old Pi |
|---|---|---:|---:|---:|---:|
| no-live-subscriber | short-small | 4.337 | 4.908 | 1.372 | 4.251 |
| no-live-subscriber | short-large | 4.311 | 4.992 | 2.865 | 4.733 |
| no-live-subscriber | stream | 0.916 | 0.916 | 0.245 | 3.836 |
| no-live-subscriber | large | 0.387 | 0.387 | 0.271 | 4.337 |
| no-live-subscriber | tool | 0.436 | 0.436 | 0.270 | 3.879 |
| no-live-subscriber | cancel | 0.356 | 0.356 | 0.246 | 4.553 |
| live-subscriber | short-small | 4.644 | 5.095 | 1.588 | 4.251 |
| live-subscriber | short-large | 3.104 | 2.819 | 2.933 | 4.730 |
| live-subscriber | stream | 0.909 | 0.909 | 0.243 | 3.836 |
| live-subscriber | large | 0.359 | 0.359 | 0.254 | 4.337 |
| live-subscriber | tool | 0.424 | 0.424 | 0.270 | 3.879 |
| live-subscriber | cancel | 0.325 | 0.325 | 0.250 | 4.553 |

Warm measured windows (per-product statistic, then median [minimum,maximum] across five Rust/Pi products; each has 32 measured turns):

| Mode | History | Engine/batch | Mean ms/turn | Median ms/turn | CPU ms/turn | Write bytes/turn |
|---|---:|---|---:|---:|---:|---:|
| no-live-subscriber | 8 | current Rust | 15.724 [9.634,17.008] | 15.386 [8.293,16.450] | 17.812 [11.250,19.062] | 965888.000 [965632.000,966400.000] |
| no-live-subscriber | 8 | fixed old Pi | 3.625 [3.524,4.546] | 3.135 [3.014,3.328] | 7.188 [6.875,9.688] | 224512.000 [224512.000,224512.000] |
| no-live-subscriber | 64 | current Rust | 15.937 [10.184,19.443] | 16.468 [9.557,18.617] | 18.438 [11.562,22.188] | 1152640.000 [1152256.000,1153152.000] |
| no-live-subscriber | 64 | fixed old Pi | 3.697 [3.667,3.911] | 3.299 [3.159,3.524] | 6.250 [5.938,6.875] | 224384.000 [224384.000,224384.000] |
| live-subscriber | 8 | current Rust | 18.268 [17.110,42.825] | 17.426 [16.761,17.552] | 24.688 [20.000,25.312] | 965888.000 [965632.000,966016.000] |
| live-subscriber | 8 | fixed old Pi | 3.934 [3.784,4.165] | 3.420 [3.309,3.699] | 7.500 [7.188,8.438] | 224512.000 [224512.000,224512.000] |
| live-subscriber | 64 | current Rust | 12.382 [10.640,19.469] | 9.979 [9.947,18.825] | 17.188 [15.000,26.875] | 1151744.000 [1151360.000,1155456.000] |
| live-subscriber | 64 | fixed old Pi | 3.989 [3.827,4.233] | 3.541 [3.482,3.679] | 6.875 [6.250,7.188] | 224384.000 [224384.000,224384.000] |

| Live case | Current mean ms/turn | Fixed DS mean ms/turn | Wall ratio | CPU ratio | Measured write ratio |
|---|---:|---:|---:|---:|---:|
| short-small | 18.268 | 15.541 | 1.175 | 1.362 | 44.917 |
| short-large | 12.382 | 15.450 | 0.801 | 0.982 | 53.880 |
| stream | 1577.271 | 1583.218 | 0.996 | 0.500 | 47.143 |
| large | 32.316 | 165.613 | 0.195 | 0.150 | 31.857 |

DeepSeek exit/flush CPU remains separate from measured turns and from Rust full-lifecycle CPU; full DeepSeek process write bytes remain unavailable. The JSON retains the exact wait4 metrics and source-specific limits.

All adverse metrics (including individual fresh paired reversals):

| Comparison | Mode | Case | Metric | Median % or cross-batch ratio | Adverse pairs |
|---|---|---|---|---:|---|
| fresh_paired_rust | no-live-subscriber | short-small | mean_turn_ms | 0.755 | 0:+0.755%,2:+62.904%,3:+4.965% |
| fresh_paired_rust | no-live-subscriber | short-small | turn_cpu_ms_per_turn | 1.786 | 0:+1.786%,2:+59.459%,3:+10.811% |
| fresh_paired_rust | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | -0.769 | 2:+27.184%,3:+6.796% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_ms | -0.145 | 2:+23.189%,3:+6.097% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_anon_pss_mib | 1.167 | 0:+1.167%,1:+1.367%,2:+0.718%,3:+0.739%,4:+1.392% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_pss_mib | 1.038 | 0:+1.166%,1:+1.376%,2:+0.427%,3:+0.439%,4:+1.038% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_rss_mib | 1.079 | 0:+1.197%,1:+1.396%,2:+0.418%,3:+0.358%,4:+1.079% |
| fresh_paired_rust | no-live-subscriber | short-small | median_measured_turn_ms | -4.198 | 2:+68.623% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_self_cpu_ms | -0.769 | 2:+27.184%,3:+6.796% |
| fresh_paired_rust | no-live-subscriber | short-small | measured_turn_cpu_ms | 1.786 | 0:+1.786%,2:+59.459%,3:+10.811% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_turn_rss_mib | 1.088 | 0:+1.088%,1:+1.287%,2:+0.694%,3:+0.499%,4:+1.980% |
| fresh_paired_rust | no-live-subscriber | short-small | through_product_exit_wall_ms | -0.604 | 2:+22.098%,3:+6.330% |
| fresh_paired_rust | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -2.023 | 2:+20.371%,3:+5.918% |
| fresh_paired_rust | no-live-subscriber | short-large | mean_turn_ms | 5.469 | 0:+16.627%,1:+5.469%,3:+17.129% |
| fresh_paired_rust | no-live-subscriber | short-large | turn_cpu_ms_per_turn | 3.333 | 0:+18.333%,1:+3.333%,3:+15.686% |
| fresh_paired_rust | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 0.000 | 1:+0.246%,3:+14.448% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_ms | 0.105 | 0:+0.105%,1:+0.718%,3:+13.788% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_anon_pss_mib | 0.846 | 1:+0.479%,2:+1.430%,3:+0.846%,4:+0.945% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_pss_mib | 0.918 | 1:+0.770%,2:+1.178%,3:+1.085%,4:+0.918% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_rss_mib | 0.930 | 1:+0.759%,2:+1.125%,3:+1.118%,4:+0.930% |
| fresh_paired_rust | no-live-subscriber | short-large | median_measured_turn_ms | 0.993 | 0:+1.707%,1:+0.993%,3:+34.775% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_self_cpu_ms | 0.000 | 1:+0.246%,3:+14.448% |
| fresh_paired_rust | no-live-subscriber | short-large | measured_turn_cpu_ms | 3.333 | 0:+18.333%,1:+3.333%,3:+15.686% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_turn_rss_mib | 1.218 | 0:+0.379%,1:+1.218%,2:+1.681%,3:+1.633%,4:+1.118% |
| fresh_paired_rust | no-live-subscriber | short-large | through_product_exit_wall_ms | -0.367 | 1:+0.681%,3:+13.050% |
| fresh_paired_rust | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | -0.259 | 1:+0.610%,3:+12.141% |
| fresh_paired_rust | no-live-subscriber | short-large | closed_logical_bytes | 0.646 | 0:+0.418%,1:+1.145%,2:+0.646%,3:+0.380%,4:+0.993% |
| fresh_paired_rust | no-live-subscriber | short-large | closed_allocated_bytes | 0.604 | 0:+0.415%,1:+1.138%,2:+0.604%,3:+0.339%,4:+0.949% |
| fresh_paired_rust | no-live-subscriber | stream | mean_turn_ms | 0.179 | 2:+0.562%,3:+1.481%,4:+0.179% |
| fresh_paired_rust | no-live-subscriber | stream | turn_cpu_ms_per_turn | -6.667 | 2:+7.692%,3:+30.769% |
| fresh_paired_rust | no-live-subscriber | stream | through_cleanup_self_cpu_ms | 0.000 | 2:+12.500%,3:+7.407% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_ms | -0.815 | 2:+1.900%,3:+0.782% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_anon_pss_mib | 0.540 | 0:+1.425%,1:+0.681%,2:+0.540%,3:+0.248%,4:+0.147% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_pss_mib | 0.597 | 0:+1.420%,1:+0.334%,2:+0.468%,3:+0.597%,4:+0.758% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_rss_mib | 0.588 | 0:+1.417%,1:+0.324%,2:+0.450%,3:+0.588%,4:+0.844% |
| fresh_paired_rust | no-live-subscriber | stream | median_measured_turn_ms | 0.179 | 2:+0.562%,3:+1.481%,4:+0.179% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_self_cpu_ms | 0.000 | 2:+12.500%,3:+7.407% |
| fresh_paired_rust | no-live-subscriber | stream | measured_turn_cpu_ms | -6.667 | 2:+7.692%,3:+30.769% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_turn_rss_mib | 0.587 | 0:+1.240%,1:+0.198%,2:+0.453%,3:+0.587%,4:+0.822% |
| fresh_paired_rust | no-live-subscriber | stream | through_product_exit_wall_ms | -0.450 | 2:+2.116%,3:+0.925% |
| fresh_paired_rust | no-live-subscriber | stream | complete_harness_lifecycle_wall_ms | -0.516 | 2:+2.178%,3:+1.254% |
| fresh_paired_rust | no-live-subscriber | large | mean_turn_ms | -2.712 | 2:+2.682%,4:+7.844% |
| fresh_paired_rust | no-live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 1:+33.333%,4:+33.333% |
| fresh_paired_rust | no-live-subscriber | large | through_cleanup_self_cpu_ms | -5.882 | 1:+6.667%,4:+12.500% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_ms | -0.167 | 0:+4.571%,4:+12.347% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_anon_pss_mib | 1.081 | 0:+0.987%,1:+2.332%,2:+1.081%,3:+0.252%,4:+1.801% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_pss_mib | 1.156 | 0:+0.790%,1:+1.710%,2:+1.156%,3:+0.070%,4:+1.651% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_rss_mib | 1.047 | 0:+0.768%,1:+1.687%,2:+1.047%,3:+0.054%,4:+1.625% |
| fresh_paired_rust | no-live-subscriber | large | median_measured_turn_ms | -2.712 | 2:+2.682%,4:+7.844% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_self_cpu_ms | -5.882 | 1:+6.667%,4:+12.500% |
| fresh_paired_rust | no-live-subscriber | large | measured_turn_cpu_ms | 0.000 | 1:+33.333%,4:+33.333% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_turn_rss_mib | 0.855 | 0:+0.684%,1:+1.184%,2:+0.855%,3:+0.014%,4:+1.391% |
| fresh_paired_rust | no-live-subscriber | large | through_product_exit_wall_ms | 0.289 | 0:+3.814%,2:+0.289%,4:+12.364% |
| fresh_paired_rust | no-live-subscriber | large | complete_harness_lifecycle_wall_ms | 0.471 | 0:+4.820%,2:+0.471%,4:+11.898% |
| fresh_paired_rust | no-live-subscriber | tool | mean_turn_ms | 7.564 | 1:+7.564%,2:+18.008%,3:+19.661%,4:+5.415% |
| fresh_paired_rust | no-live-subscriber | tool | turn_cpu_ms_per_turn | 50.000 | 1:+50.000%,2:+100.000%,3:+50.000%,4:+25.000% |
| fresh_paired_rust | no-live-subscriber | tool | through_cleanup_self_cpu_ms | 5.882 | 1:+5.882%,2:+6.250%,4:+20.000% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_ms | 5.990 | 1:+5.990%,2:+7.570%,4:+12.788% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_anon_pss_mib | 0.819 | 0:+1.001%,1:+0.548%,2:+1.598%,3:+0.541%,4:+0.819% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_pss_mib | 0.986 | 0:+1.144%,1:+0.775%,2:+1.639%,3:+0.466%,4:+0.986% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_rss_mib | 0.969 | 0:+1.079%,1:+0.756%,2:+1.596%,3:+0.469%,4:+0.969% |
| fresh_paired_rust | no-live-subscriber | tool | median_measured_turn_ms | 7.564 | 1:+7.564%,2:+18.008%,3:+19.661%,4:+5.415% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_self_cpu_ms | 5.882 | 1:+5.882%,2:+6.250%,4:+20.000% |
| fresh_paired_rust | no-live-subscriber | tool | measured_turn_cpu_ms | 50.000 | 1:+50.000%,2:+100.000%,3:+50.000%,4:+25.000% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_turn_rss_mib | 0.750 | 0:+0.750%,1:+1.122%,2:+1.586%,3:+0.717%,4:+0.734% |
| fresh_paired_rust | no-live-subscriber | tool | through_product_exit_wall_ms | 5.897 | 1:+5.897%,2:+7.831%,4:+14.075% |
| fresh_paired_rust | no-live-subscriber | tool | complete_harness_lifecycle_wall_ms | 5.684 | 1:+6.649%,2:+5.684%,4:+14.568% |
| fresh_paired_rust | no-live-subscriber | cancel | mean_turn_ms | -1.411 | 1:+24.641%,3:+1.626% |
| fresh_paired_rust | no-live-subscriber | cancel | turn_cpu_ms_per_turn | -25.000 | 1:+50.000%,3:+33.333% |
| fresh_paired_rust | no-live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 1:+6.667%,3:+6.250% |
| fresh_paired_rust | no-live-subscriber | cancel | lifecycle_ms | 2.043 | 1:+2.655%,3:+2.043%,4:+2.462% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_anon_pss_mib | 1.228 | 0:+1.764%,1:+1.645%,2:+0.670%,3:+0.653%,4:+1.228% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_pss_mib | 0.773 | 0:+0.773%,1:+0.748%,2:+0.816%,3:+0.272%,4:+1.161% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_rss_mib | 0.769 | 0:+0.769%,1:+0.739%,2:+0.844%,3:+0.252%,4:+1.154% |
| fresh_paired_rust | no-live-subscriber | cancel | median_measured_turn_ms | -1.411 | 1:+24.641%,3:+1.626% |
| fresh_paired_rust | no-live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 1:+6.667%,3:+6.250% |
| fresh_paired_rust | no-live-subscriber | cancel | measured_turn_cpu_ms | -25.000 | 1:+50.000%,3:+33.333% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_turn_rss_mib | 0.698 | 0:+1.095%,1:+0.867%,2:+0.626%,3:+0.128%,4:+0.698% |
| fresh_paired_rust | no-live-subscriber | cancel | through_product_exit_wall_ms | -0.323 | 1:+3.597%,4:+2.430% |
| fresh_paired_rust | no-live-subscriber | cancel | complete_harness_lifecycle_wall_ms | -0.448 | 1:+2.353%,4:+2.481% |
| fresh_paired_rust | live-subscriber | short-small | mean_turn_ms | -3.564 | 1:+7.369% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_ms | -0.608 | 1:+3.267%,4:+52.295% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_anon_pss_mib | 0.871 | 0:+0.886%,1:+0.086%,3:+0.871%,4:+1.657% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_pss_mib | 0.795 | 0:+0.795%,2:+0.218%,3:+1.061%,4:+1.337% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_rss_mib | 0.783 | 0:+0.783%,2:+0.216%,3:+1.087%,4:+1.286% |
| fresh_paired_rust | live-subscriber | short-small | max_sampled_turn_rss_mib | 0.959 | 0:+0.959%,1:+0.029%,2:+0.621%,3:+1.538%,4:+1.056% |
| fresh_paired_rust | live-subscriber | short-small | through_product_exit_wall_ms | -0.638 | 1:+3.055%,4:+50.850% |
| fresh_paired_rust | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -1.251 | 1:+3.209%,4:+49.941% |
| fresh_paired_rust | live-subscriber | short-small | closed_logical_bytes | -0.492 | 4:+0.002% |
| fresh_paired_rust | live-subscriber | short-large | mean_turn_ms | -44.973 | 2:+54.390% |
| fresh_paired_rust | live-subscriber | short-large | turn_cpu_ms_per_turn | -48.113 | 2:+45.763% |
| fresh_paired_rust | live-subscriber | short-large | through_cleanup_self_cpu_ms | -10.940 | 2:+5.312% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_ms | -12.324 | 0:+2.390%,2:+7.251% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_anon_pss_mib | 0.379 | 2:+0.379%,3:+0.982%,4:+0.416% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_pss_mib | 0.301 | 1:+0.049%,2:+0.301%,3:+0.923%,4:+0.474% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_rss_mib | 0.304 | 1:+0.022%,2:+0.304%,3:+0.881%,4:+0.494% |
| fresh_paired_rust | live-subscriber | short-large | median_measured_turn_ms | -51.607 | 2:+59.767% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_self_cpu_ms | -10.940 | 2:+5.312% |
| fresh_paired_rust | live-subscriber | short-large | measured_turn_cpu_ms | -48.113 | 2:+45.763% |
| fresh_paired_rust | live-subscriber | short-large | max_sampled_turn_rss_mib | 0.687 | 0:+0.050%,2:+0.687%,3:+1.068%,4:+0.874% |
| fresh_paired_rust | live-subscriber | short-large | through_product_exit_wall_ms | -11.927 | 0:+2.275%,2:+6.359% |
| fresh_paired_rust | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | -11.669 | 0:+2.569%,2:+6.115% |
| fresh_paired_rust | live-subscriber | short-large | closed_logical_bytes | -0.038 | 1:+0.646%,3:+0.456% |
| fresh_paired_rust | live-subscriber | short-large | closed_allocated_bytes | -0.038 | 1:+0.642%,3:+0.415% |
| fresh_paired_rust | live-subscriber | stream | mean_turn_ms | -0.595 | 1:+1.396% |
| fresh_paired_rust | live-subscriber | stream | turn_cpu_ms_per_turn | 0.000 | 1:+7.143% |
| fresh_paired_rust | live-subscriber | stream | through_cleanup_self_cpu_ms | -6.897 | 1:+20.000% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_ms | -1.214 | 1:+1.653% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_anon_pss_mib | 1.182 | 0:+1.182%,1:+0.690%,2:+1.034%,3:+3.183%,4:+1.336% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_pss_mib | 0.904 | 0:+0.598%,1:+0.692%,2:+0.904%,3:+1.773%,4:+1.271% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_rss_mib | 0.834 | 0:+0.564%,1:+0.620%,2:+0.834%,3:+1.705%,4:+1.250% |
| fresh_paired_rust | live-subscriber | stream | median_measured_turn_ms | -0.595 | 1:+1.396% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_self_cpu_ms | -6.897 | 1:+20.000% |
| fresh_paired_rust | live-subscriber | stream | measured_turn_cpu_ms | 0.000 | 1:+7.143% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_turn_rss_mib | 1.061 | 0:+1.061%,1:+0.601%,2:+0.982%,3:+1.914%,4:+1.290% |
| fresh_paired_rust | live-subscriber | stream | through_product_exit_wall_ms | -1.010 | 1:+1.289% |
| fresh_paired_rust | live-subscriber | stream | complete_harness_lifecycle_wall_ms | -1.423 | 1:+1.369% |
| fresh_paired_rust | live-subscriber | large | mean_turn_ms | 3.428 | 2:+5.947%,3:+68.124%,4:+3.428% |
| fresh_paired_rust | live-subscriber | large | turn_cpu_ms_per_turn | -25.000 | 3:+100.000% |
| fresh_paired_rust | live-subscriber | large | through_cleanup_self_cpu_ms | 6.250 | 2:+7.143%,3:+13.333%,4:+6.250% |
| fresh_paired_rust | live-subscriber | large | lifecycle_ms | 1.091 | 2:+1.550%,3:+1.091%,4:+8.333% |
| fresh_paired_rust | live-subscriber | large | max_sampled_anon_pss_mib | 0.521 | 0:+0.584%,2:+2.630%,3:+0.521% |
| fresh_paired_rust | live-subscriber | large | max_sampled_pss_mib | 0.810 | 0:+0.889%,1:+0.430%,2:+2.110%,3:+0.810%,4:+0.220% |
| fresh_paired_rust | live-subscriber | large | max_sampled_rss_mib | 0.728 | 0:+0.820%,1:+0.423%,2:+2.099%,3:+0.728%,4:+0.228% |
| fresh_paired_rust | live-subscriber | large | median_measured_turn_ms | 3.428 | 2:+5.947%,3:+68.124%,4:+3.428% |
| fresh_paired_rust | live-subscriber | large | lifecycle_self_cpu_ms | 6.250 | 2:+7.143%,3:+13.333%,4:+6.250% |
| fresh_paired_rust | live-subscriber | large | measured_turn_cpu_ms | -25.000 | 3:+100.000% |
| fresh_paired_rust | live-subscriber | large | max_sampled_turn_rss_mib | 1.039 | 0:+1.072%,1:+0.864%,2:+2.309%,3:+1.039%,4:+0.476% |
| fresh_paired_rust | live-subscriber | large | through_product_exit_wall_ms | 0.209 | 0:+0.209%,2:+2.037%,4:+8.995% |
| fresh_paired_rust | live-subscriber | large | complete_harness_lifecycle_wall_ms | 0.211 | 0:+0.541%,2:+0.211%,4:+8.070% |
| fresh_paired_rust | live-subscriber | tool | mean_turn_ms | -2.625 | 2:+11.085% |
| fresh_paired_rust | live-subscriber | tool | lifecycle_ms | -0.934 | 0:+8.764%,2:+2.263% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_anon_pss_mib | 0.488 | 0:+2.521%,1:+0.488%,2:+3.020%,4:+0.148% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_pss_mib | 0.785 | 0:+1.854%,1:+0.692%,2:+2.249%,3:+0.386%,4:+0.785% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_rss_mib | 0.813 | 0:+1.788%,1:+0.679%,2:+2.308%,3:+0.423%,4:+0.813% |
| fresh_paired_rust | live-subscriber | tool | median_measured_turn_ms | -2.625 | 2:+11.085% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_turn_rss_mib | 1.060 | 0:+2.144%,1:+0.935%,2:+2.477%,4:+1.060% |
| fresh_paired_rust | live-subscriber | tool | through_product_exit_wall_ms | -0.819 | 0:+9.175%,2:+2.007% |
| fresh_paired_rust | live-subscriber | tool | complete_harness_lifecycle_wall_ms | -0.580 | 0:+9.420%,2:+1.259% |
| fresh_paired_rust | live-subscriber | cancel | mean_turn_ms | -7.234 | 0:+10.730% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_ms | -0.617 | 2:+1.125% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_anon_pss_mib | 1.221 | 0:+0.910%,1:+2.118%,2:+1.363%,3:+0.949%,4:+1.221% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_pss_mib | 0.987 | 0:+0.697%,1:+1.661%,2:+1.154%,3:+0.987%,4:+0.357% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_rss_mib | 0.988 | 0:+0.762%,1:+1.636%,2:+1.082%,3:+0.988%,4:+0.346% |
| fresh_paired_rust | live-subscriber | cancel | median_measured_turn_ms | -7.234 | 0:+10.730% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_turn_rss_mib | 1.020 | 0:+1.042%,1:+1.352%,2:+0.846%,3:+1.020%,4:+0.387% |
| fresh_paired_rust | live-subscriber | cancel | through_product_exit_wall_ms | -0.050 | 0:+1.923%,2:+1.438% |
| fresh_paired_rust | live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 0.294 | 0:+1.929%,2:+2.067%,4:+0.294% |
| prior_pi | no-live-subscriber | short-small | closed_allocated_bytes | 43.500 | cross-batch |
| prior_pi | no-live-subscriber | short-small | closed_logical_bytes | 43.407 | cross-batch |
| prior_pi | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 1.951 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_ms | 3.004 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_self_cpu_ms | 2.529 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_anon_pss_mib | 1.124 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_pss_mib | 1.474 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_rss_mib | 1.077 | cross-batch |
| prior_pi | no-live-subscriber | short-small | mean_turn_ms | 4.337 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_cpu_ms | 2.478 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_write_bytes | 4.302 | cross-batch |
| prior_pi | no-live-subscriber | short-small | median_measured_turn_ms | 4.908 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_bytes | 4.251 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_mib | 4.251 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.372 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_product_exit_wall_ms | 1.963 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_cpu_ms_per_turn | 2.478 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_write_bytes_per_turn | 4.302 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_allocated_bytes | 44.383 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_logical_bytes | 44.320 | cross-batch |
| prior_pi | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.606 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_ms | 4.847 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_self_cpu_ms | 4.081 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_anon_pss_mib | 1.279 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_pss_mib | 1.516 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_rss_mib | 1.201 | cross-batch |
| prior_pi | no-live-subscriber | short-large | mean_turn_ms | 4.311 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_cpu_ms | 2.950 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_write_bytes | 5.137 | cross-batch |
| prior_pi | no-live-subscriber | short-large | median_measured_turn_ms | 4.992 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_bytes | 4.733 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_mib | 4.733 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 2.865 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_product_exit_wall_ms | 3.723 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_cpu_ms_per_turn | 2.950 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_write_bytes_per_turn | 5.137 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_allocated_bytes | 15.906 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_logical_bytes | 15.807 | cross-batch |
| prior_pi | no-live-subscriber | stream | max_sampled_pss_mib | 1.241 | cross-batch |
| prior_pi | no-live-subscriber | stream | measured_turn_write_bytes | 2.619 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_bytes | 3.836 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_mib | 3.836 | cross-batch |
| prior_pi | no-live-subscriber | stream | turn_write_bytes_per_turn | 2.619 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_allocated_bytes | 10.579 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_logical_bytes | 10.479 | cross-batch |
| prior_pi | no-live-subscriber | large | lifecycle_ms | 1.219 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_anon_pss_mib | 1.086 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_pss_mib | 1.543 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_rss_mib | 1.043 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_turn_rss_mib | 1.041 | cross-batch |
| prior_pi | no-live-subscriber | large | measured_turn_write_bytes | 3.097 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_bytes | 4.337 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_mib | 4.337 | cross-batch |
| prior_pi | no-live-subscriber | large | turn_write_bytes_per_turn | 3.097 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_allocated_bytes | 17.969 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_logical_bytes | 17.850 | cross-batch |
| prior_pi | no-live-subscriber | tool | lifecycle_ms | 1.200 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_pss_mib | 1.372 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_turn_rss_mib | 1.053 | cross-batch |
| prior_pi | no-live-subscriber | tool | measured_turn_write_bytes | 2.712 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_bytes | 3.879 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_mib | 3.879 | cross-batch |
| prior_pi | no-live-subscriber | tool | turn_write_bytes_per_turn | 2.712 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_allocated_bytes | 13.100 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_logical_bytes | 12.972 | cross-batch |
| prior_pi | no-live-subscriber | cancel | lifecycle_ms | 1.157 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_pss_mib | 1.391 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_turn_rss_mib | 1.033 | cross-batch |
| prior_pi | no-live-subscriber | cancel | measured_turn_write_bytes | 3.194 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_bytes | 4.553 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_mib | 4.553 | cross-batch |
| prior_pi | no-live-subscriber | cancel | turn_write_bytes_per_turn | 3.194 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_allocated_bytes | 43.524 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_logical_bytes | 43.431 | cross-batch |
| prior_pi | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 2.040 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_ms | 3.089 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_self_cpu_ms | 2.852 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_anon_pss_mib | 1.192 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_pss_mib | 1.541 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_rss_mib | 1.120 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_turn_rss_mib | 1.007 | cross-batch |
| prior_pi | live-subscriber | short-small | mean_turn_ms | 4.644 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_cpu_ms | 3.292 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_write_bytes | 4.302 | cross-batch |
| prior_pi | live-subscriber | short-small | median_measured_turn_ms | 5.095 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_bytes | 4.251 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_mib | 4.251 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.588 | cross-batch |
| prior_pi | live-subscriber | short-small | through_product_exit_wall_ms | 2.031 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_cpu_ms_per_turn | 3.292 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_write_bytes_per_turn | 4.302 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_allocated_bytes | 44.400 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_logical_bytes | 44.337 | cross-batch |
| prior_pi | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.482 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_ms | 4.599 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_self_cpu_ms | 4.151 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_anon_pss_mib | 1.282 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_pss_mib | 1.514 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_rss_mib | 1.204 | cross-batch |
| prior_pi | live-subscriber | short-large | mean_turn_ms | 3.104 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_cpu_ms | 2.500 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_write_bytes | 5.133 | cross-batch |
| prior_pi | live-subscriber | short-large | median_measured_turn_ms | 2.819 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_bytes | 4.730 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_mib | 4.730 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_self_cpu_ms | 2.933 | cross-batch |
| prior_pi | live-subscriber | short-large | through_product_exit_wall_ms | 3.551 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_cpu_ms_per_turn | 2.500 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_write_bytes_per_turn | 5.133 | cross-batch |
| prior_pi | live-subscriber | stream | closed_allocated_bytes | 15.906 | cross-batch |
| prior_pi | live-subscriber | stream | closed_logical_bytes | 15.807 | cross-batch |
| prior_pi | live-subscriber | stream | max_sampled_pss_mib | 1.254 | cross-batch |
| prior_pi | live-subscriber | stream | measured_turn_write_bytes | 2.619 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_bytes | 3.836 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_mib | 3.836 | cross-batch |
| prior_pi | live-subscriber | stream | turn_write_bytes_per_turn | 2.619 | cross-batch |
| prior_pi | live-subscriber | large | closed_allocated_bytes | 10.579 | cross-batch |
| prior_pi | live-subscriber | large | closed_logical_bytes | 10.479 | cross-batch |
| prior_pi | live-subscriber | large | lifecycle_ms | 1.106 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_anon_pss_mib | 1.090 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_pss_mib | 1.545 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_rss_mib | 1.045 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_turn_rss_mib | 1.046 | cross-batch |
| prior_pi | live-subscriber | large | measured_turn_write_bytes | 3.097 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_bytes | 4.337 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_mib | 4.337 | cross-batch |
| prior_pi | live-subscriber | large | turn_write_bytes_per_turn | 3.097 | cross-batch |
| prior_pi | live-subscriber | tool | closed_allocated_bytes | 17.969 | cross-batch |
| prior_pi | live-subscriber | tool | closed_logical_bytes | 17.850 | cross-batch |
| prior_pi | live-subscriber | tool | lifecycle_ms | 1.116 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_pss_mib | 1.366 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_turn_rss_mib | 1.055 | cross-batch |
| prior_pi | live-subscriber | tool | measured_turn_write_bytes | 2.712 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_bytes | 3.879 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_mib | 3.879 | cross-batch |
| prior_pi | live-subscriber | tool | turn_write_bytes_per_turn | 2.712 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_allocated_bytes | 13.100 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_logical_bytes | 12.972 | cross-batch |
| prior_pi | live-subscriber | cancel | lifecycle_ms | 1.173 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_pss_mib | 1.385 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_turn_rss_mib | 1.031 | cross-batch |
| prior_pi | live-subscriber | cancel | measured_turn_write_bytes | 3.194 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_bytes | 4.553 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_mib | 4.553 | cross-batch |
| prior_pi | live-subscriber | cancel | turn_write_bytes_per_turn | 3.194 | cross-batch |
| prior_deepseek | live-subscriber | short-small | mean_turn_ms | 1.175 | cross-batch |
| prior_deepseek | live-subscriber | short-small | median_measured_turn_ms | 1.180 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_cpu_ms_per_turn | 1.362 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_write_bytes_per_turn | 44.917 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_write_bytes_per_turn | 53.880 | cross-batch |
| prior_deepseek | live-subscriber | stream | turn_write_bytes_per_turn | 47.143 | cross-batch |
| prior_deepseek | live-subscriber | large | turn_write_bytes_per_turn | 31.857 | cross-batch |

- Only the fresh old/new Rust products are interleaved pairs. Pi/DeepSeek use fixed previous batches and separate descriptive ratios of medians; no causal parity or paired confidence claim.
- /proc CPU counters use 100 ticks/s; each cumulative endpoint has 10 ms granularity. This is not profiler sampling.
- A product measured-turn median and a product measured-turn mean are different estimands. Across products we report median and range of those statistics, not pooled turns.
- DeepSeek SDK idle does not await flush/fsync. Native wait4 CPU covers exit/flush and verification separately; full process write_bytes is unavailable, not zero. No equivalent lifecycle ratio is constructed.
- All recorded adverse pairs and all unavailable ratios are retained, including reversals hidden by a favorable group median.

Unavailable ratio records, all per-product values, and reference hashes are retained in report-data.json.
