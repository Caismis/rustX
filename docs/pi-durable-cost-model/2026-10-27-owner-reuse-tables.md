# Owner reuse performance evidence

Fresh Rust comparison: five interleaved pairs per cell. Historical Pi/DeepSeek rows are fixed, separate batches; their ratios are ratios of medians, not paired results.

| Mode | Case | Paired median wall % [min,max] | Full CPU % | Full write bytes % | Sample anon PSS % | Sample RSS % |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | -5.318 [-8.944,5.942] | -0.699 | -1.119 | 0.173 | -0.026 |
| no-live-subscriber | short-large | -5.174 [-46.583,24.689] | -0.230 | -1.116 | -0.606 | -0.455 |
| no-live-subscriber | stream | -0.640 [-2.547,2.981] | -6.452 | -0.226 | -0.511 | -0.347 |
| no-live-subscriber | large | 3.883 [-6.562,40.595] | 0.000 | -0.255 | -0.562 | -0.220 |
| no-live-subscriber | tool | -0.145 [-9.819,17.800] | -5.882 | -0.398 | -0.535 | -0.120 |
| no-live-subscriber | cancel | 7.441 [-9.763,17.998] | 0.000 | -0.263 | 0.122 | -0.097 |
| live-subscriber | short-small | 1.159 [-8.605,8.729] | -0.556 | -1.128 | -2.309 | -1.525 |
| live-subscriber | short-large | 20.365 [-44.137,83.363] | 8.584 | -1.112 | -2.287 | -1.788 |
| live-subscriber | stream | 1.131 [-1.341,2.995] | 3.571 | -0.226 | 0.701 | 0.677 |
| live-subscriber | large | 1.345 [-2.417,36.582] | 0.000 | -0.255 | 0.287 | 0.243 |
| live-subscriber | tool | 2.044 [-4.297,44.141] | 5.882 | -0.398 | 0.497 | 0.411 |
| live-subscriber | cancel | -8.979 [-47.141,-4.143] | -6.250 | -0.263 | -0.187 | -0.038 |

| Mode | Case | Mean wall / old Pi | Median wall / old Pi | Full CPU / old Pi | Full writes / old Pi |
|---|---|---:|---:|---:|---:|
| no-live-subscriber | short-small | 4.914 | 5.729 | 1.457 | 4.744 |
| no-live-subscriber | short-large | 4.940 | 5.554 | 2.993 | 5.230 |
| no-live-subscriber | stream | 0.931 | 0.931 | 0.264 | 4.009 |
| no-live-subscriber | large | 0.443 | 0.443 | 0.288 | 4.541 |
| no-live-subscriber | tool | 0.427 | 0.427 | 0.254 | 4.171 |
| no-live-subscriber | cancel | 0.313 | 0.313 | 0.246 | 4.767 |
| live-subscriber | short-small | 5.506 | 6.166 | 1.845 | 4.744 |
| live-subscriber | short-large | 4.474 | 4.746 | 3.353 | 5.230 |
| live-subscriber | stream | 0.926 | 0.926 | 0.261 | 4.009 |
| live-subscriber | large | 0.395 | 0.395 | 0.271 | 4.541 |
| live-subscriber | tool | 0.484 | 0.484 | 0.302 | 4.171 |
| live-subscriber | cancel | 0.342 | 0.342 | 0.250 | 4.767 |

Warm measured windows (per-product statistic, then median [minimum,maximum] across five Rust/Pi products; each has 32 measured turns):

| Mode | History | Engine/batch | Mean ms/turn | Median ms/turn | CPU ms/turn | Write bytes/turn |
|---|---:|---|---:|---:|---:|---:|
| no-live-subscriber | 8 | current Rust | 17.815 [16.048,19.311] | 17.959 [15.921,19.355] | 20.000 [17.812,22.188] | 1085056.000 [1084672.000,1085312.000] |
| no-live-subscriber | 8 | fixed old Pi | 3.625 [3.524,4.546] | 3.135 [3.014,3.328] | 7.188 [6.875,9.688] | 224512.000 [224512.000,224512.000] |
| no-live-subscriber | 64 | current Rust | 18.264 [13.655,21.265] | 18.323 [11.288,21.323] | 20.312 [15.625,23.750] | 1241984.000 [1241216.000,1243648.000] |
| no-live-subscriber | 64 | fixed old Pi | 3.697 [3.667,3.911] | 3.299 [3.159,3.524] | 6.250 [5.938,6.875] | 224384.000 [224384.000,224384.000] |
| live-subscriber | 8 | current Rust | 21.659 [20.747,25.218] | 21.087 [20.069,21.666] | 30.312 [29.375,32.188] | 1084928.000 [1084544.000,1085184.000] |
| live-subscriber | 8 | fixed old Pi | 3.934 [3.784,4.165] | 3.420 [3.309,3.699] | 7.500 [7.188,8.438] | 224512.000 [224512.000,224512.000] |
| live-subscriber | 64 | current Rust | 17.847 [12.838,23.926] | 16.803 [11.925,23.921] | 25.312 [18.750,34.062] | 1243136.000 [1242240.000,1243264.000] |
| live-subscriber | 64 | fixed old Pi | 3.989 [3.827,4.233] | 3.541 [3.482,3.679] | 6.875 [6.250,7.188] | 224384.000 [224384.000,224384.000] |

| Live case | Current mean ms/turn | Fixed DS mean ms/turn | Wall ratio | CPU ratio | Measured write ratio |
|---|---:|---:|---:|---:|---:|
| short-small | 21.659 | 15.541 | 1.394 | 1.672 | 50.452 |
| short-large | 17.847 | 15.450 | 1.155 | 1.446 | 58.156 |
| stream | 1606.907 | 1583.218 | 1.015 | 0.567 | 50.714 |
| large | 35.492 | 165.613 | 0.214 | 0.200 | 35.143 |

DeepSeek exit/flush CPU remains separate from measured turns and from Rust full-lifecycle CPU; full DeepSeek process write bytes remain unavailable. The JSON retains the exact wait4 metrics and source-specific limits.

All adverse metrics (including individual fresh paired reversals):

| Comparison | Mode | Case | Metric | Median % or cross-batch ratio | Adverse pairs |
|---|---|---|---|---:|---|
| fresh_paired_rust | no-live-subscriber | short-small | mean_turn_ms | -4.304 | 1:+5.641%,2:+1.564% |
| fresh_paired_rust | no-live-subscriber | short-small | turn_cpu_ms_per_turn | -1.389 | 1:+6.452%,2:+1.724% |
| fresh_paired_rust | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | -0.699 | 1:+0.730%,2:+4.724% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_ms | -1.534 | 2:+5.176% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_anon_pss_mib | 0.173 | 1:+0.173%,3:+0.223%,4:+0.546% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_pss_mib | -0.031 | 1:+0.040%,4:+0.298% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_rss_mib | -0.026 | 1:+0.061%,4:+0.312% |
| fresh_paired_rust | no-live-subscriber | short-small | median_measured_turn_ms | -5.318 | 1:+5.942%,3:+1.081% |
| fresh_paired_rust | no-live-subscriber | short-small | lifecycle_self_cpu_ms | -0.699 | 1:+0.730%,2:+4.724% |
| fresh_paired_rust | no-live-subscriber | short-small | measured_turn_cpu_ms | -1.389 | 1:+6.452%,2:+1.724% |
| fresh_paired_rust | no-live-subscriber | short-small | max_sampled_turn_rss_mib | -0.063 | 1:+0.086%,4:+0.653% |
| fresh_paired_rust | no-live-subscriber | short-small | through_product_exit_wall_ms | -1.686 | 2:+5.218% |
| fresh_paired_rust | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -1.791 | 2:+5.006% |
| fresh_paired_rust | no-live-subscriber | short-large | mean_turn_ms | -6.713 | 0:+11.347%,4:+22.430% |
| fresh_paired_rust | no-live-subscriber | short-large | turn_cpu_ms_per_turn | -8.451 | 0:+13.333%,4:+20.635% |
| fresh_paired_rust | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | -0.230 | 0:+0.476%,4:+3.103% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_ms | 0.023 | 0:+0.969%,2:+0.023%,4:+3.699% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_anon_pss_mib | -0.606 | 4:+0.695% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_pss_mib | -0.454 | 4:+0.349% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_rss_mib | -0.455 | 4:+0.378% |
| fresh_paired_rust | no-live-subscriber | short-large | median_measured_turn_ms | -5.174 | 0:+19.014%,4:+24.689% |
| fresh_paired_rust | no-live-subscriber | short-large | lifecycle_self_cpu_ms | -0.230 | 0:+0.476%,4:+3.103% |
| fresh_paired_rust | no-live-subscriber | short-large | measured_turn_cpu_ms | -8.451 | 0:+13.333%,4:+20.635% |
| fresh_paired_rust | no-live-subscriber | short-large | max_sampled_turn_rss_mib | -0.818 | 4:+0.487% |
| fresh_paired_rust | no-live-subscriber | short-large | through_product_exit_wall_ms | 0.036 | 0:+0.904%,2:+0.036%,4:+3.790% |
| fresh_paired_rust | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 0.178 | 0:+0.714%,2:+0.178%,4:+3.717% |
| fresh_paired_rust | no-live-subscriber | stream | mean_turn_ms | -0.640 | 2:+0.139%,4:+2.981% |
| fresh_paired_rust | no-live-subscriber | stream | turn_cpu_ms_per_turn | 0.000 | 0:+5.882%,4:+25.000% |
| fresh_paired_rust | no-live-subscriber | stream | through_cleanup_self_cpu_ms | -6.452 | 4:+10.345% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_ms | -0.397 | 4:+2.642% |
| fresh_paired_rust | no-live-subscriber | stream | max_sampled_anon_pss_mib | -0.511 | 3:+0.165%,4:+0.134% |
| fresh_paired_rust | no-live-subscriber | stream | median_measured_turn_ms | -0.640 | 2:+0.139%,4:+2.981% |
| fresh_paired_rust | no-live-subscriber | stream | lifecycle_self_cpu_ms | -6.452 | 4:+10.345% |
| fresh_paired_rust | no-live-subscriber | stream | measured_turn_cpu_ms | 0.000 | 0:+5.882%,4:+25.000% |
| fresh_paired_rust | no-live-subscriber | stream | through_product_exit_wall_ms | -0.329 | 4:+2.682% |
| fresh_paired_rust | no-live-subscriber | stream | complete_harness_lifecycle_wall_ms | -0.680 | 4:+3.285% |
| fresh_paired_rust | no-live-subscriber | large | mean_turn_ms | 3.883 | 2:+3.883%,3:+7.141%,4:+40.595% |
| fresh_paired_rust | no-live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 4:+25.000% |
| fresh_paired_rust | no-live-subscriber | large | through_cleanup_self_cpu_ms | 0.000 | 2:+13.333% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_ms | 2.108 | 0:+2.108%,2:+5.659%,4:+6.938% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_anon_pss_mib | -0.562 | 2:+0.741%,4:+0.071% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_pss_mib | -0.281 | 2:+0.718% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_rss_mib | -0.220 | 2:+0.786% |
| fresh_paired_rust | no-live-subscriber | large | median_measured_turn_ms | 3.883 | 2:+3.883%,3:+7.141%,4:+40.595% |
| fresh_paired_rust | no-live-subscriber | large | lifecycle_self_cpu_ms | 0.000 | 2:+13.333% |
| fresh_paired_rust | no-live-subscriber | large | measured_turn_cpu_ms | 0.000 | 4:+25.000% |
| fresh_paired_rust | no-live-subscriber | large | max_sampled_turn_rss_mib | -0.318 | 2:+0.830%,4:+0.169% |
| fresh_paired_rust | no-live-subscriber | large | through_product_exit_wall_ms | 3.051 | 0:+3.051%,2:+8.002%,4:+5.597% |
| fresh_paired_rust | no-live-subscriber | large | complete_harness_lifecycle_wall_ms | 3.124 | 0:+3.124%,2:+10.187%,4:+3.560% |
| fresh_paired_rust | no-live-subscriber | tool | mean_turn_ms | -0.145 | 0:+5.702%,1:+17.800% |
| fresh_paired_rust | no-live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 1:+50.000% |
| fresh_paired_rust | no-live-subscriber | tool | through_cleanup_self_cpu_ms | -5.882 | 1:+5.882% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_ms | -4.584 | 3:+0.302% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_anon_pss_mib | -0.535 | 1:+0.865%,2:+0.013% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_pss_mib | -0.138 | 1:+0.612%,4:+0.044% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_rss_mib | -0.120 | 1:+0.646%,4:+0.057% |
| fresh_paired_rust | no-live-subscriber | tool | median_measured_turn_ms | -0.145 | 0:+5.702%,1:+17.800% |
| fresh_paired_rust | no-live-subscriber | tool | lifecycle_self_cpu_ms | -5.882 | 1:+5.882% |
| fresh_paired_rust | no-live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 1:+50.000% |
| fresh_paired_rust | no-live-subscriber | tool | max_sampled_turn_rss_mib | 0.143 | 1:+1.221%,2:+0.143%,4:+0.213% |
| fresh_paired_rust | no-live-subscriber | tool | complete_harness_lifecycle_wall_ms | -3.858 | 1:+0.384% |
| fresh_paired_rust | no-live-subscriber | cancel | mean_turn_ms | 7.441 | 0:+3.296%,1:+10.133%,2:+7.441%,4:+17.998% |
| fresh_paired_rust | no-live-subscriber | cancel | turn_cpu_ms_per_turn | 0.000 | 1:+33.333%,2:+100.000% |
| fresh_paired_rust | no-live-subscriber | cancel | through_cleanup_self_cpu_ms | 0.000 | 0:+7.143% |
| fresh_paired_rust | no-live-subscriber | cancel | lifecycle_ms | 1.027 | 0:+4.909%,3:+1.066%,4:+1.027% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_anon_pss_mib | 0.122 | 0:+0.122%,1:+0.022%,2:+0.424%,4:+0.153% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_pss_mib | -0.098 | 1:+0.133%,2:+0.747% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_rss_mib | -0.097 | 1:+0.077%,2:+0.735% |
| fresh_paired_rust | no-live-subscriber | cancel | median_measured_turn_ms | 7.441 | 0:+3.296%,1:+10.133%,2:+7.441%,4:+17.998% |
| fresh_paired_rust | no-live-subscriber | cancel | lifecycle_self_cpu_ms | 0.000 | 0:+7.143% |
| fresh_paired_rust | no-live-subscriber | cancel | measured_turn_cpu_ms | 0.000 | 1:+33.333%,2:+100.000% |
| fresh_paired_rust | no-live-subscriber | cancel | max_sampled_turn_rss_mib | -0.121 | 1:+0.209%,2:+0.939% |
| fresh_paired_rust | no-live-subscriber | cancel | through_product_exit_wall_ms | 1.476 | 0:+4.611%,3:+2.213%,4:+1.476% |
| fresh_paired_rust | no-live-subscriber | cancel | complete_harness_lifecycle_wall_ms | 1.615 | 0:+4.223%,3:+1.777%,4:+1.615% |
| fresh_paired_rust | live-subscriber | short-small | mean_turn_ms | 1.790 | 1:+30.955%,2:+4.257%,3:+1.790% |
| fresh_paired_rust | live-subscriber | short-small | turn_cpu_ms_per_turn | -0.990 | 1:+15.730%,2:+3.297% |
| fresh_paired_rust | live-subscriber | short-small | through_cleanup_self_cpu_ms | -0.556 | 1:+4.624%,2:+0.571% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_ms | -1.249 | 1:+47.669%,2:+0.452% |
| fresh_paired_rust | live-subscriber | short-small | median_measured_turn_ms | 1.159 | 1:+8.729%,2:+2.113%,3:+1.159% |
| fresh_paired_rust | live-subscriber | short-small | lifecycle_self_cpu_ms | -0.556 | 1:+4.624%,2:+0.571% |
| fresh_paired_rust | live-subscriber | short-small | measured_turn_cpu_ms | -0.990 | 1:+15.730%,2:+3.297% |
| fresh_paired_rust | live-subscriber | short-small | through_product_exit_wall_ms | -1.088 | 1:+45.494%,2:+0.368% |
| fresh_paired_rust | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | -0.860 | 1:+43.832%,2:+0.304% |
| fresh_paired_rust | live-subscriber | short-large | mean_turn_ms | 12.501 | 0:+72.852%,2:+66.396%,4:+12.501% |
| fresh_paired_rust | live-subscriber | short-large | turn_cpu_ms_per_turn | 8.000 | 0:+66.667%,2:+67.692%,4:+8.000% |
| fresh_paired_rust | live-subscriber | short-large | through_cleanup_self_cpu_ms | 8.584 | 0:+15.570%,2:+8.584%,4:+16.166% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_ms | 7.102 | 0:+13.706%,2:+7.102%,4:+13.599% |
| fresh_paired_rust | live-subscriber | short-large | median_measured_turn_ms | 20.365 | 0:+83.363%,2:+76.067%,4:+20.365% |
| fresh_paired_rust | live-subscriber | short-large | lifecycle_self_cpu_ms | 8.584 | 0:+15.570%,2:+8.584%,4:+16.166% |
| fresh_paired_rust | live-subscriber | short-large | measured_turn_cpu_ms | 8.000 | 0:+66.667%,2:+67.692%,4:+8.000% |
| fresh_paired_rust | live-subscriber | short-large | through_product_exit_wall_ms | 7.074 | 0:+12.877%,2:+7.074%,4:+13.253% |
| fresh_paired_rust | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 7.021 | 0:+12.429%,2:+7.021%,4:+12.612% |
| fresh_paired_rust | live-subscriber | stream | mean_turn_ms | 1.131 | 0:+2.995%,1:+0.262%,2:+1.811%,3:+1.131% |
| fresh_paired_rust | live-subscriber | stream | turn_cpu_ms_per_turn | 6.250 | 0:+6.250%,1:+6.250%,2:+12.500%,3:+13.333% |
| fresh_paired_rust | live-subscriber | stream | through_cleanup_self_cpu_ms | 3.571 | 0:+3.226%,1:+3.571%,2:+3.571%,3:+11.111% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_ms | 1.274 | 0:+22.710%,2:+1.305%,3:+1.274% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_anon_pss_mib | 0.701 | 0:+0.725%,1:+0.701%,2:+0.508%,4:+1.774% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_pss_mib | 0.735 | 0:+0.528%,1:+0.735%,2:+0.975%,3:+0.205%,4:+1.542% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_rss_mib | 0.677 | 0:+0.504%,1:+0.677%,2:+0.981%,3:+0.221%,4:+1.484% |
| fresh_paired_rust | live-subscriber | stream | median_measured_turn_ms | 1.131 | 0:+2.995%,1:+0.262%,2:+1.811%,3:+1.131% |
| fresh_paired_rust | live-subscriber | stream | lifecycle_self_cpu_ms | 3.571 | 0:+3.226%,1:+3.571%,2:+3.571%,3:+11.111% |
| fresh_paired_rust | live-subscriber | stream | measured_turn_cpu_ms | 6.250 | 0:+6.250%,1:+6.250%,2:+12.500%,3:+13.333% |
| fresh_paired_rust | live-subscriber | stream | max_sampled_turn_rss_mib | 0.573 | 0:+0.573%,1:+0.790%,2:+0.437%,4:+1.606% |
| fresh_paired_rust | live-subscriber | stream | through_product_exit_wall_ms | 1.157 | 0:+22.509%,2:+1.230%,3:+1.157% |
| fresh_paired_rust | live-subscriber | stream | complete_harness_lifecycle_wall_ms | 1.180 | 0:+21.856%,2:+1.180%,3:+1.361% |
| fresh_paired_rust | live-subscriber | large | mean_turn_ms | 1.345 | 1:+36.582%,3:+1.345%,4:+8.005% |
| fresh_paired_rust | live-subscriber | large | turn_cpu_ms_per_turn | 0.000 | 1:+25.000% |
| fresh_paired_rust | live-subscriber | large | through_cleanup_self_cpu_ms | 0.000 | 1:+13.333%,2:+7.143% |
| fresh_paired_rust | live-subscriber | large | lifecycle_ms | 0.300 | 1:+13.558%,2:+0.966%,3:+0.300% |
| fresh_paired_rust | live-subscriber | large | max_sampled_anon_pss_mib | 0.287 | 0:+1.052%,1:+0.287%,3:+0.679% |
| fresh_paired_rust | live-subscriber | large | max_sampled_pss_mib | 0.255 | 0:+0.553%,1:+0.255%,3:+0.321%,4:+0.066% |
| fresh_paired_rust | live-subscriber | large | max_sampled_rss_mib | 0.243 | 0:+0.540%,1:+0.292%,3:+0.243%,4:+0.057% |
| fresh_paired_rust | live-subscriber | large | median_measured_turn_ms | 1.345 | 1:+36.582%,3:+1.345%,4:+8.005% |
| fresh_paired_rust | live-subscriber | large | lifecycle_self_cpu_ms | 0.000 | 1:+13.333%,2:+7.143% |
| fresh_paired_rust | live-subscriber | large | measured_turn_cpu_ms | 0.000 | 1:+25.000% |
| fresh_paired_rust | live-subscriber | large | max_sampled_turn_rss_mib | 0.229 | 0:+0.291%,1:+0.299%,3:+0.050%,4:+0.229% |
| fresh_paired_rust | live-subscriber | large | through_product_exit_wall_ms | 0.305 | 1:+12.791%,2:+1.225%,3:+0.305% |
| fresh_paired_rust | live-subscriber | large | complete_harness_lifecycle_wall_ms | 0.597 | 1:+11.754%,2:+1.217%,3:+0.597% |
| fresh_paired_rust | live-subscriber | tool | mean_turn_ms | 2.044 | 1:+2.044%,2:+3.972%,4:+44.141% |
| fresh_paired_rust | live-subscriber | tool | turn_cpu_ms_per_turn | 0.000 | 0:+83.333%,4:+16.667% |
| fresh_paired_rust | live-subscriber | tool | through_cleanup_self_cpu_ms | 5.882 | 0:+12.500%,1:+5.882%,2:+5.556%,4:+35.294% |
| fresh_paired_rust | live-subscriber | tool | lifecycle_ms | 0.904 | 0:+53.711%,2:+0.904%,4:+23.214% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_anon_pss_mib | 0.497 | 0:+1.739%,2:+0.684%,4:+0.497% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_pss_mib | 0.428 | 0:+1.154%,2:+0.634%,4:+0.428% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_rss_mib | 0.411 | 0:+1.109%,2:+0.624%,4:+0.411% |
| fresh_paired_rust | live-subscriber | tool | median_measured_turn_ms | 2.044 | 1:+2.044%,2:+3.972%,4:+44.141% |
| fresh_paired_rust | live-subscriber | tool | lifecycle_self_cpu_ms | 5.882 | 0:+12.500%,1:+5.882%,2:+5.556%,4:+35.294% |
| fresh_paired_rust | live-subscriber | tool | measured_turn_cpu_ms | 0.000 | 0:+83.333%,4:+16.667% |
| fresh_paired_rust | live-subscriber | tool | max_sampled_turn_rss_mib | 0.326 | 0:+1.021%,2:+0.509%,4:+0.326% |
| fresh_paired_rust | live-subscriber | tool | through_product_exit_wall_ms | 0.631 | 0:+53.135%,2:+0.631%,4:+24.375% |
| fresh_paired_rust | live-subscriber | tool | complete_harness_lifecycle_wall_ms | 1.202 | 0:+51.022%,2:+1.202%,4:+28.868% |
| fresh_paired_rust | live-subscriber | cancel | turn_cpu_ms_per_turn | 0.000 | 4:+33.333% |
| fresh_paired_rust | live-subscriber | cancel | through_cleanup_self_cpu_ms | -6.250 | 2:+6.250% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_ms | -6.672 | 2:+1.851%,4:+0.497% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_anon_pss_mib | -0.187 | 0:+0.054%,4:+0.108% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_pss_mib | -0.113 | 2:+0.048%,4:+0.052% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_rss_mib | -0.038 | 2:+0.088%,4:+0.047% |
| fresh_paired_rust | live-subscriber | cancel | lifecycle_self_cpu_ms | -6.250 | 2:+6.250% |
| fresh_paired_rust | live-subscriber | cancel | measured_turn_cpu_ms | 0.000 | 4:+33.333% |
| fresh_paired_rust | live-subscriber | cancel | max_sampled_turn_rss_mib | -0.165 | 1:+0.042%,4:+0.177% |
| fresh_paired_rust | live-subscriber | cancel | through_product_exit_wall_ms | -8.004 | 2:+3.062%,4:+0.493% |
| fresh_paired_rust | live-subscriber | cancel | complete_harness_lifecycle_wall_ms | -9.970 | 2:+0.929% |
| prior_pi | no-live-subscriber | short-small | closed_allocated_bytes | 43.714 | cross-batch |
| prior_pi | no-live-subscriber | short-small | closed_logical_bytes | 43.621 | cross-batch |
| prior_pi | no-live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 2.049 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_ms | 3.151 | cross-batch |
| prior_pi | no-live-subscriber | short-small | lifecycle_self_cpu_ms | 2.686 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_anon_pss_mib | 1.127 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_pss_mib | 1.462 | cross-batch |
| prior_pi | no-live-subscriber | short-small | max_sampled_rss_mib | 1.068 | cross-batch |
| prior_pi | no-live-subscriber | short-small | mean_turn_ms | 4.914 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_cpu_ms | 2.783 | cross-batch |
| prior_pi | no-live-subscriber | short-small | measured_turn_write_bytes | 4.833 | cross-batch |
| prior_pi | no-live-subscriber | short-small | median_measured_turn_ms | 5.729 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_bytes | 4.744 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_process_write_mib | 4.744 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.457 | cross-batch |
| prior_pi | no-live-subscriber | short-small | through_product_exit_wall_ms | 2.055 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_cpu_ms_per_turn | 2.783 | cross-batch |
| prior_pi | no-live-subscriber | short-small | turn_write_bytes_per_turn | 4.833 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_allocated_bytes | 44.167 | cross-batch |
| prior_pi | no-live-subscriber | short-large | closed_logical_bytes | 44.102 | cross-batch |
| prior_pi | no-live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.758 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_ms | 5.059 | cross-batch |
| prior_pi | no-live-subscriber | short-large | lifecycle_self_cpu_ms | 4.263 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_anon_pss_mib | 1.273 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_pss_mib | 1.500 | cross-batch |
| prior_pi | no-live-subscriber | short-large | max_sampled_rss_mib | 1.188 | cross-batch |
| prior_pi | no-live-subscriber | short-large | mean_turn_ms | 4.940 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_cpu_ms | 3.250 | cross-batch |
| prior_pi | no-live-subscriber | short-large | measured_turn_write_bytes | 5.535 | cross-batch |
| prior_pi | no-live-subscriber | short-large | median_measured_turn_ms | 5.554 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_bytes | 5.230 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_process_write_mib | 5.230 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_cleanup_self_cpu_ms | 2.993 | cross-batch |
| prior_pi | no-live-subscriber | short-large | through_product_exit_wall_ms | 3.877 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_cpu_ms_per_turn | 3.250 | cross-batch |
| prior_pi | no-live-subscriber | short-large | turn_write_bytes_per_turn | 5.535 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_allocated_bytes | 16.750 | cross-batch |
| prior_pi | no-live-subscriber | stream | closed_logical_bytes | 16.624 | cross-batch |
| prior_pi | no-live-subscriber | stream | max_sampled_pss_mib | 1.218 | cross-batch |
| prior_pi | no-live-subscriber | stream | measured_turn_write_bytes | 2.817 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_bytes | 4.009 | cross-batch |
| prior_pi | no-live-subscriber | stream | through_cleanup_process_write_mib | 4.009 | cross-batch |
| prior_pi | no-live-subscriber | stream | turn_write_bytes_per_turn | 2.817 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_allocated_bytes | 11.237 | cross-batch |
| prior_pi | no-live-subscriber | large | closed_logical_bytes | 11.140 | cross-batch |
| prior_pi | no-live-subscriber | large | lifecycle_ms | 1.278 | cross-batch |
| prior_pi | no-live-subscriber | large | lifecycle_self_cpu_ms | 1.062 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_anon_pss_mib | 1.070 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_pss_mib | 1.503 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_rss_mib | 1.018 | cross-batch |
| prior_pi | no-live-subscriber | large | max_sampled_turn_rss_mib | 1.017 | cross-batch |
| prior_pi | no-live-subscriber | large | measured_turn_write_bytes | 3.417 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_bytes | 4.541 | cross-batch |
| prior_pi | no-live-subscriber | large | through_cleanup_process_write_mib | 4.541 | cross-batch |
| prior_pi | no-live-subscriber | large | turn_write_bytes_per_turn | 3.417 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_allocated_bytes | 19.844 | cross-batch |
| prior_pi | no-live-subscriber | tool | closed_logical_bytes | 19.736 | cross-batch |
| prior_pi | no-live-subscriber | tool | lifecycle_ms | 1.114 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_pss_mib | 1.336 | cross-batch |
| prior_pi | no-live-subscriber | tool | max_sampled_turn_rss_mib | 1.031 | cross-batch |
| prior_pi | no-live-subscriber | tool | measured_turn_write_bytes | 3.110 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_bytes | 4.171 | cross-batch |
| prior_pi | no-live-subscriber | tool | through_cleanup_process_write_mib | 4.171 | cross-batch |
| prior_pi | no-live-subscriber | tool | turn_write_bytes_per_turn | 3.110 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_allocated_bytes | 13.900 | cross-batch |
| prior_pi | no-live-subscriber | cancel | closed_logical_bytes | 13.776 | cross-batch |
| prior_pi | no-live-subscriber | cancel | lifecycle_ms | 1.088 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_pss_mib | 1.363 | cross-batch |
| prior_pi | no-live-subscriber | cancel | max_sampled_turn_rss_mib | 1.013 | cross-batch |
| prior_pi | no-live-subscriber | cancel | measured_turn_write_bytes | 3.522 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_bytes | 4.767 | cross-batch |
| prior_pi | no-live-subscriber | cancel | through_cleanup_process_write_mib | 4.767 | cross-batch |
| prior_pi | no-live-subscriber | cancel | turn_write_bytes_per_turn | 3.522 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_allocated_bytes | 43.714 | cross-batch |
| prior_pi | live-subscriber | short-small | closed_logical_bytes | 43.621 | cross-batch |
| prior_pi | live-subscriber | short-small | complete_harness_lifecycle_wall_ms | 2.182 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_ms | 3.352 | cross-batch |
| prior_pi | live-subscriber | short-small | lifecycle_self_cpu_ms | 3.315 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_anon_pss_mib | 1.187 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_pss_mib | 1.521 | cross-batch |
| prior_pi | live-subscriber | short-small | max_sampled_rss_mib | 1.105 | cross-batch |
| prior_pi | live-subscriber | short-small | mean_turn_ms | 5.506 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_cpu_ms | 4.042 | cross-batch |
| prior_pi | live-subscriber | short-small | measured_turn_write_bytes | 4.832 | cross-batch |
| prior_pi | live-subscriber | short-small | median_measured_turn_ms | 6.166 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_bytes | 4.744 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_process_write_mib | 4.744 | cross-batch |
| prior_pi | live-subscriber | short-small | through_cleanup_self_cpu_ms | 1.845 | cross-batch |
| prior_pi | live-subscriber | short-small | through_product_exit_wall_ms | 2.198 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_cpu_ms_per_turn | 4.042 | cross-batch |
| prior_pi | live-subscriber | short-small | turn_write_bytes_per_turn | 4.832 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_allocated_bytes | 44.133 | cross-batch |
| prior_pi | live-subscriber | short-large | closed_logical_bytes | 44.069 | cross-batch |
| prior_pi | live-subscriber | short-large | complete_harness_lifecycle_wall_ms | 3.785 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_ms | 5.050 | cross-batch |
| prior_pi | live-subscriber | short-large | lifecycle_self_cpu_ms | 4.745 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_anon_pss_mib | 1.279 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_pss_mib | 1.502 | cross-batch |
| prior_pi | live-subscriber | short-large | max_sampled_rss_mib | 1.195 | cross-batch |
| prior_pi | live-subscriber | short-large | mean_turn_ms | 4.474 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_cpu_ms | 3.682 | cross-batch |
| prior_pi | live-subscriber | short-large | measured_turn_write_bytes | 5.540 | cross-batch |
| prior_pi | live-subscriber | short-large | median_measured_turn_ms | 4.746 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_bytes | 5.230 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_process_write_mib | 5.230 | cross-batch |
| prior_pi | live-subscriber | short-large | through_cleanup_self_cpu_ms | 3.353 | cross-batch |
| prior_pi | live-subscriber | short-large | through_product_exit_wall_ms | 3.892 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_cpu_ms_per_turn | 3.682 | cross-batch |
| prior_pi | live-subscriber | short-large | turn_write_bytes_per_turn | 5.540 | cross-batch |
| prior_pi | live-subscriber | stream | closed_allocated_bytes | 16.750 | cross-batch |
| prior_pi | live-subscriber | stream | closed_logical_bytes | 16.624 | cross-batch |
| prior_pi | live-subscriber | stream | max_sampled_pss_mib | 1.235 | cross-batch |
| prior_pi | live-subscriber | stream | measured_turn_write_bytes | 2.817 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_bytes | 4.009 | cross-batch |
| prior_pi | live-subscriber | stream | through_cleanup_process_write_mib | 4.009 | cross-batch |
| prior_pi | live-subscriber | stream | turn_write_bytes_per_turn | 2.817 | cross-batch |
| prior_pi | live-subscriber | large | closed_allocated_bytes | 11.237 | cross-batch |
| prior_pi | live-subscriber | large | closed_logical_bytes | 11.140 | cross-batch |
| prior_pi | live-subscriber | large | lifecycle_ms | 1.136 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_anon_pss_mib | 1.085 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_pss_mib | 1.522 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_rss_mib | 1.028 | cross-batch |
| prior_pi | live-subscriber | large | max_sampled_turn_rss_mib | 1.030 | cross-batch |
| prior_pi | live-subscriber | large | measured_turn_write_bytes | 3.417 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_bytes | 4.541 | cross-batch |
| prior_pi | live-subscriber | large | through_cleanup_process_write_mib | 4.541 | cross-batch |
| prior_pi | live-subscriber | large | turn_write_bytes_per_turn | 3.417 | cross-batch |
| prior_pi | live-subscriber | tool | closed_allocated_bytes | 19.844 | cross-batch |
| prior_pi | live-subscriber | tool | closed_logical_bytes | 19.736 | cross-batch |
| prior_pi | live-subscriber | tool | lifecycle_ms | 1.145 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_pss_mib | 1.332 | cross-batch |
| prior_pi | live-subscriber | tool | max_sampled_turn_rss_mib | 1.030 | cross-batch |
| prior_pi | live-subscriber | tool | measured_turn_write_bytes | 3.110 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_bytes | 4.171 | cross-batch |
| prior_pi | live-subscriber | tool | through_cleanup_process_write_mib | 4.171 | cross-batch |
| prior_pi | live-subscriber | tool | turn_write_bytes_per_turn | 3.110 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_allocated_bytes | 13.900 | cross-batch |
| prior_pi | live-subscriber | cancel | closed_logical_bytes | 13.776 | cross-batch |
| prior_pi | live-subscriber | cancel | lifecycle_ms | 1.201 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_pss_mib | 1.358 | cross-batch |
| prior_pi | live-subscriber | cancel | max_sampled_turn_rss_mib | 1.013 | cross-batch |
| prior_pi | live-subscriber | cancel | measured_turn_write_bytes | 3.522 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_bytes | 4.767 | cross-batch |
| prior_pi | live-subscriber | cancel | through_cleanup_process_write_mib | 4.767 | cross-batch |
| prior_pi | live-subscriber | cancel | turn_write_bytes_per_turn | 3.522 | cross-batch |
| prior_deepseek | live-subscriber | short-small | mean_turn_ms | 1.394 | cross-batch |
| prior_deepseek | live-subscriber | short-small | median_measured_turn_ms | 1.427 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_cpu_ms_per_turn | 1.672 | cross-batch |
| prior_deepseek | live-subscriber | short-small | turn_write_bytes_per_turn | 50.452 | cross-batch |
| prior_deepseek | live-subscriber | short-large | mean_turn_ms | 1.155 | cross-batch |
| prior_deepseek | live-subscriber | short-large | median_measured_turn_ms | 1.096 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_cpu_ms_per_turn | 1.446 | cross-batch |
| prior_deepseek | live-subscriber | short-large | turn_write_bytes_per_turn | 58.156 | cross-batch |
| prior_deepseek | live-subscriber | stream | mean_turn_ms | 1.015 | cross-batch |
| prior_deepseek | live-subscriber | stream | median_measured_turn_ms | 1.015 | cross-batch |
| prior_deepseek | live-subscriber | stream | turn_write_bytes_per_turn | 50.714 | cross-batch |
| prior_deepseek | live-subscriber | large | turn_write_bytes_per_turn | 35.143 | cross-batch |

- Only the fresh old/new Rust products are interleaved pairs. Pi/DeepSeek use fixed previous batches and separate descriptive ratios of medians; no causal parity or paired confidence claim.
- /proc CPU counters use 100 ticks/s; each cumulative endpoint has 10 ms granularity. This is not profiler sampling.
- A product measured-turn median and a product measured-turn mean are different estimands. Across products we report median and range of those statistics, not pooled turns.
- DeepSeek SDK idle does not await flush/fsync. Native wait4 CPU covers exit/flush and verification separately; full process write_bytes is unavailable, not zero. No equivalent lifecycle ratio is constructed.
- All recorded adverse pairs and all unavailable ratios are retained, including reversals hidden by a favorable group median.

Unavailable ratio records, all per-product values, and reference hashes are retained in report-data.json.
