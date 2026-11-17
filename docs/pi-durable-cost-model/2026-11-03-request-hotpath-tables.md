# Request hot-path full values

Raw recomputation: `final-audit.json` and `support-audit.json` in the evidence archive. Four descriptive pairs per formal cell; all reversals retained. No confidence interval or parity claim. A product value for warm wall is the median of its 32 warm turns; the displayed absolute value is the median of four product values. Changes/ratios are calculated pair by pair first.

## Final ordinary triads

### no-live-subscriber / short-small

| Metric | Before | Candidate | Pi | Paired change %, pairs 0–3 | Median change % | Candidate/Pi, pairs 0–3 | Median ratio |
|---|---:|---:|---:|---|---:|---|---:|
| Warm median wall, ms | 6.819750 | 6.647250 | 3.119250 | 1.108244, -5.686261, 4.958365, -6.044268 | -2.289009 | 2.123047, 2.139126, 2.265153, 2.089289 | 2.131087 |
| Warm mean wall, ms | 7.475062 | 7.242891 | 3.687000 | -0.304379, -5.764428, 6.846294, -7.621015 | -3.034404 | 1.938512, 1.987960, 2.156263, 1.932554 | 1.963236 |
| Warm CPU/turn, ms | 8.593750 | 8.281250 | 5.937500 | 0.000000, -10.000000, 16.000000, -13.793103 | -5.000000 | 1.181818, 1.500000, 1.611111, 1.250000 | 1.375000 |
| Warm process writes/turn, bytes | 839104.000000 | 839232.000000 | 224512.000000 | 0.030511, 0.000000, 0.000000, -0.015249 | 0.000000 | 3.738312, 3.737742, 3.736602, 3.738312 | 3.738027 |
| Lifecycle CPU, ms | 850.000000 | 840.000000 | 460.000000 | 1.204819, -4.545455, 3.614458, -4.597701 | -1.670318 | 1.714286, 1.866667, 1.911111, 1.765957 | 1.816312 |
| Through-cleanup process writes, MiB | 33.259766 | 33.263672 | 8.875000 | 0.023491, 0.000000, 0.000000, -0.011741 | 0.000000 | 3.748239, 3.747799, 3.746919, 3.748239 | 3.748019 |
| Lifecycle wall, ms | 960.443000 | 941.374500 | 453.619500 | -0.855473, -3.080279, 3.622814, -4.962298 | -1.967876 | 1.873667, 2.089753, 2.156237, 2.050804 | 2.070279 |
| Maximum sampled anonymous PSS, MiB | 122.498047 | 121.339844 | 111.683594 | -1.939260, -2.090791, 0.802517, -0.016060 | -0.977660 | 1.080939, 1.084494, 1.072986, 1.092016 | 1.082716 |
| Warm maximum sampled RSS, MiB | 155.800781 | 154.673828 | 161.507812 | -1.587578, -1.582666, 0.426380, -0.324847 | -0.953756 | 0.957088, 0.960414, 0.953916, 0.958286 | 0.957687 |
| Post-writer-close recorded file bytes | 6635238.000000 | 6637298.000000 | 172032.000000 | 0.000000, 0.000000, 0.062131, 0.062093 | 0.031046 | 38.593738, 38.569789, 38.569789, 38.593738 | 38.581764 |

### no-live-subscriber / short-large

| Metric | Before | Candidate | Pi | Paired change %, pairs 0–3 | Median change % | Candidate/Pi, pairs 0–3 | Median ratio |
|---|---:|---:|---:|---|---:|---|---:|
| Warm median wall, ms | 7.607750 | 7.315000 | 3.266750 | 4.649260, -8.695924, -1.903740, -6.903887 | -4.403814 | 2.325909, 2.006585, 2.344441, 2.233184 | 2.279546 |
| Warm mean wall, ms | 8.463719 | 8.016250 | 3.726516 | 4.591963, -9.386332, -2.568581, -7.881718 | -5.225149 | 2.291526, 1.938135, 2.202109, 2.147079 | 2.174594 |
| Warm CPU/turn, ms | 9.531250 | 9.062500 | 6.406250 | 0.000000, -12.121212, 0.000000, -9.375000 | -4.687500 | 1.380952, 1.260870, 1.450000, 1.450000 | 1.415476 |
| Warm process writes/turn, bytes | 990272.000000 | 989824.000000 | 224384.000000 | 0.000000, 0.116219, -0.077580, -0.103426 | -0.038790 | 4.414147, 4.422704, 4.408443, 4.407872 | 4.411295 |
| Lifecycle CPU, ms | 2815.000000 | 2795.000000 | 915.000000 | 0.719424, -3.793103, -1.071429, -0.706714 | -0.889071 | 3.043478, 2.968085, 3.077778, 3.087912 | 3.060628 |
| Through-cleanup process writes, MiB | 85.794922 | 85.775391 | 20.777344 | -0.013657, 0.050073, -0.004555, -0.040982 | -0.009106 | 4.129160, 4.132168, 4.127468, 4.127092 | 4.128314 |
| Lifecycle wall, ms | 2962.238000 | 2935.237000 | 879.427500 | 0.418566, -3.506023, -0.690635, -1.353102 | -1.021868 | 3.435419, 3.159549, 3.380750, 3.288502 | 3.334626 |
| Maximum sampled anonymous PSS, MiB | 202.351562 | 202.539062 | 156.136719 | 0.269328, -0.276992, 0.125502, 0.469003 | 0.197415 | 1.309107, 1.282862, 1.248243, 1.347179 | 1.295984 |
| Warm maximum sampled RSS, MiB | 172.128906 | 172.464844 | 184.037109 | -0.011356, -0.525853, 0.493081, 0.460317 | 0.224481 | 0.946835, 0.913454, 0.926587, 0.979678 | 0.936711 |
| Post-writer-close recorded file bytes | 9392614.000000 | 9380254.000000 | 245760.000000 | -0.000255, -0.482208, -0.087690, -0.175534 | -0.131612 | 38.252075, 38.017570, 38.201880, 38.134823 | 38.168351 |

### live-subscriber / short-small

| Metric | Before | Candidate | Pi | Paired change %, pairs 0–3 | Median change % | Candidate/Pi, pairs 0–3 | Median ratio |
|---|---:|---:|---:|---|---:|---|---:|
| Warm median wall, ms | 7.612750 | 7.513250 | 3.365250 | -5.438529, -0.544405, -0.461376, -1.670503 | -1.107454 | 2.148825, 2.308969, 2.348313, 2.125533 | 2.228897 |
| Warm mean wall, ms | 8.090266 | 8.048656 | 3.907781 | -3.285276, 0.402259, -0.612717, -0.838123 | -0.725420 | 2.047163, 2.072285, 2.141141, 1.991034 | 2.059724 |
| Warm CPU/turn, ms | 10.312500 | 10.312500 | 5.937500 | -2.941176, 0.000000, 0.000000, 0.000000 | 0.000000 | 1.736842, 1.736842, 1.736842, 1.600000 | 1.736842 |
| Warm process writes/turn, bytes | 839232.000000 | 839168.000000 | 224512.000000 | -0.015251, -0.015251, -0.030506, 0.015256 | -0.015251 | 3.737742, 3.737742, 3.736602, 3.737742 | 3.737742 |
| Lifecycle CPU, ms | 920.000000 | 920.000000 | 475.000000 | -1.075269, 1.098901, -1.075269, 0.000000 | -0.537634 | 1.916667, 1.957447, 2.000000, 1.895833 | 1.937057 |
| Through-cleanup process writes, MiB | 33.263672 | 33.261719 | 8.875000 | -0.011743, -0.011743, -0.023488, 0.011745 | -0.011743 | 3.747799, 3.747799, 3.746919, 3.747799 | 3.747799 |
| Lifecycle wall, ms | 982.521000 | 981.034000 | 477.879000 | -0.944537, 0.743143, -0.523788, 0.239196 | -0.142296 | 2.027975, 2.080178, 2.103419, 1.959515 | 2.054076 |
| Maximum sampled anonymous PSS, MiB | 126.501953 | 126.390625 | 112.029297 | -0.101754, -0.136063, -0.448100, 0.488230 | -0.118909 | 1.250212, 1.123660, 1.093189, 1.129110 | 1.126385 |
| Warm maximum sampled RSS, MiB | 160.042969 | 160.531250 | 162.798828 | 0.673351, 0.075697, -0.099461, 0.578786 | 0.327241 | 1.073994, 0.961975, 0.976895, 0.995461 | 0.986178 |
| Post-writer-close recorded file bytes | 6635235.000000 | 6637295.000000 | 172032.000000 | 0.062093, 0.062093, 0.000000, 0.000000 | 0.031046 | 38.593721, 38.593721, 38.569772, 38.569772 | 38.581746 |

### live-subscriber / short-large

| Metric | Before | Candidate | Pi | Paired change %, pairs 0–3 | Median change % | Candidate/Pi, pairs 0–3 | Median ratio |
|---|---:|---:|---:|---|---:|---|---:|
| Warm median wall, ms | 8.656500 | 8.611000 | 3.593000 | -5.982048, 16.007766, -1.249779, -0.272294 | -0.761037 | 2.386640, 2.699015, 2.272246, 2.517182 | 2.451911 |
| Warm mean wall, ms | 9.417484 | 9.898578 | 4.074359 | -81.638006, 13.793351, -1.300209, -1.273419 | -1.286814 | 2.831723, 2.259257, 2.241974, 2.321876 | 2.290566 |
| Warm CPU/turn, ms | 11.875000 | 11.718750 | 7.031250 | -26.000000, 11.111111, -5.263158, 0.000000 | -2.631579 | 1.850000, 1.481481, 1.565217, 1.727273 | 1.646245 |
| Warm process writes/turn, bytes | 990720.000000 | 990912.000000 | 224384.000000 | 0.051693, -0.232048, 0.129349, 0.012917 | 0.032305 | 4.416429, 4.414718, 4.415859, 4.416999 | 4.416144 |
| Lifecycle CPU, ms | 3015.000000 | 3065.000000 | 985.000000 | -5.588235, 1.993355, 1.661130, 0.662252 | 1.161691 | 3.378947, 3.039604, 3.000000, 3.166667 | 3.103135 |
| Through-cleanup process writes, MiB | 85.796875 | 85.800781 | 20.777344 | 0.004554, -0.027305, 0.036440, 0.018209 | 0.011381 | 4.128972, 4.130100, 4.128972, 4.130664 | 4.129536 |
| Lifecycle wall, ms | 3062.775500 | 3131.320000 | 948.905000 | -34.642715, 2.581017, 2.734428, 1.670856 | 2.125936 | 4.864284, 3.215777, 3.120023, 3.383183 | 3.299480 |
| Maximum sampled anonymous PSS, MiB | 207.171875 | 207.466797 | 154.173828 | 0.638802, 0.304279, -0.054552, -0.550486 | 0.124863 | 1.353490, 1.455251, 1.337942, 1.333117 | 1.345716 |
| Warm maximum sampled RSS, MiB | 178.048828 | 178.681641 | 184.908203 | 1.024577, 0.224403, 0.059072, -0.513877 | 0.141737 | 0.948404, 1.017852, 0.939907, 0.988155 | 0.968279 |
| Post-writer-close recorded file bytes | 9386419.000000 | 9382323.000000 | 245760.000000 | 0.044453, -0.043884, 0.351479, -0.482251 | 0.000285 | 38.168437, 38.185103, 38.319218, 38.034322 | 38.176770 |

The post-writer-close footprint sums each engine's recorded files. Rust still records WAL/SHM and small session/configuration files at this endpoint, whereas Pi records its database/workspace. It is not a comparison of compacted main-database sizes or proof that all SQLite handles have closed. Complete file lists remain in the raw products.

## Earlier same-turn cohorts, separately

These are not pooled with final results. `current-pi` is the fresh predevelopment c9/Pi baseline; `regression-repeat` compares the two older Statistics binaries. Ratios are rust/Pi for the former and c9/pre-Statistics for the latter.

| Cohort | Mode | Case | Metric | All four paired ratios | Median |
|---|---|---|---|---|---:|
| current-pi | no-live-subscriber | short-small | Warm median wall, ms | 2.298961, 2.020411, 2.224963, 2.157677 | 2.191320 |
| current-pi | no-live-subscriber | short-small | Warm mean wall, ms | 2.063031, 1.865298, 2.063020, 1.968854 | 2.015937 |
| current-pi | no-live-subscriber | short-small | Warm CPU/turn, ms | 1.473684, 1.315789, 1.444444, 1.333333 | 1.388889 |
| current-pi | no-live-subscriber | short-small | Warm process writes/turn, bytes | 3.734892, 3.738312, 3.738312, 3.735462 | 3.736887 |
| current-pi | no-live-subscriber | short-small | Lifecycle CPU, ms | 1.888889, 1.765957, 1.822222, 1.863636 | 1.842929 |
| current-pi | no-live-subscriber | short-small | Through-cleanup process writes, MiB | 3.745599, 3.748239, 3.748239, 3.746039 | 3.747139 |
| current-pi | no-live-subscriber | short-large | Warm median wall, ms | 2.273488, 2.193328, 2.221286, 2.246302 | 2.233794 |
| current-pi | no-live-subscriber | short-large | Warm mean wall, ms | 2.117764, 2.128724, 2.083058, 2.042973 | 2.100411 |
| current-pi | no-live-subscriber | short-large | Warm CPU/turn, ms | 1.526316, 1.473684, 1.428571, 1.318182 | 1.451128 |
| current-pi | no-live-subscriber | short-large | Warm process writes/turn, bytes | 4.413577, 4.410154, 4.417570, 4.415288 | 4.414432 |
| current-pi | no-live-subscriber | short-large | Lifecycle CPU, ms | 3.088889, 3.000000, 2.836735, 3.021739 | 3.010870 |
| current-pi | no-live-subscriber | short-large | Through-cleanup process writes, MiB | 4.128032, 4.126904, 4.130100, 4.129724 | 4.128878 |
| current-pi | live-subscriber | short-small | Warm median wall, ms | 2.345229, 2.280921, 2.328960, 2.249223 | 2.304940 |
| current-pi | live-subscriber | short-small | Warm mean wall, ms | 2.088288, 2.159433, 2.163737, 2.105046 | 2.132239 |
| current-pi | live-subscriber | short-small | Warm CPU/turn, ms | 1.684211, 1.888889, 1.600000, 1.684211 | 1.684211 |
| current-pi | live-subscriber | short-small | Warm process writes/turn, bytes | 3.736602, 3.737172, 3.737742, 3.738883 | 3.737457 |
| current-pi | live-subscriber | short-small | Lifecycle CPU, ms | 2.000000, 1.916667, 1.978261, 1.836735 | 1.947464 |
| current-pi | live-subscriber | short-small | Through-cleanup process writes, MiB | 3.746919, 3.747359, 3.747799, 3.748680 | 3.747579 |
| current-pi | live-subscriber | short-large | Warm median wall, ms | 2.409313, 2.368797, 2.693688, 2.367071 | 2.389055 |
| current-pi | live-subscriber | short-large | Warm mean wall, ms | 2.403084, 2.313564, 2.593666, 2.301320 | 2.358324 |
| current-pi | live-subscriber | short-large | Warm CPU/turn, ms | 1.761905, 1.666667, 2.000000, 1.800000 | 1.780952 |
| current-pi | live-subscriber | short-large | Warm process writes/turn, bytes | 4.418711, 4.412436, 4.407872, 4.417570 | 4.415003 |
| current-pi | live-subscriber | short-large | Lifecycle CPU, ms | 3.180851, 3.304348, 3.255319, 3.217391 | 3.236355 |
| current-pi | live-subscriber | short-large | Through-cleanup process writes, MiB | 4.129724, 4.129160, 4.126904, 4.130100 | 4.129442 |
| regression-repeat | no-live-subscriber | short-large | Warm median wall, ms | 0.952768, 1.071842, 0.990597, 1.006759 | 0.998678 |
| regression-repeat | no-live-subscriber | short-large | Warm mean wall, ms | 0.969443, 1.067708, 0.992231, 0.996412 | 0.994321 |
| regression-repeat | no-live-subscriber | short-large | Warm CPU/turn, ms | 1.000000, 1.107143, 1.000000, 0.964286 | 1.000000 |
| regression-repeat | no-live-subscriber | short-large | Warm process writes/turn, bytes | 1.000129, 0.998709, 1.000258, 0.999096 | 0.999612 |
| regression-repeat | no-live-subscriber | short-large | Lifecycle CPU, ms | 1.007273, 1.021978, 0.992780, 0.989051 | 1.000026 |
| regression-repeat | no-live-subscriber | short-large | Through-cleanup process writes, MiB | 0.998453, 0.998000, 0.998772, 0.998181 | 0.998317 |
| regression-repeat | live-subscriber | short-large | Warm median wall, ms | 1.094464, 0.866445, 0.985561, 0.941563 | 0.963562 |
| regression-repeat | live-subscriber | short-large | Warm mean wall, ms | 1.072679, 0.225616, 0.768512, 0.959173 | 0.863842 |
| regression-repeat | live-subscriber | short-large | Warm CPU/turn, ms | 1.088235, 0.734694, 0.947368, 0.921053 | 0.934211 |
| regression-repeat | live-subscriber | short-large | Warm process writes/turn, bytes | 1.000129, 0.999224, 1.003366, 1.000517 | 1.000323 |
| regression-repeat | live-subscriber | short-large | Lifecycle CPU, ms | 1.006780, 0.905199, 0.954545, 0.976821 | 0.965683 |
| regression-repeat | live-subscriber | short-large | Through-cleanup process writes, MiB | 0.998999, 0.998817, 1.000000, 0.998909 | 0.998954 |

## Synchronized storage diagnostics

One diagnostic product per revision/scenario per diagnostic kind. Logical columns are not WAL or physical device bytes. WAL ownership uses endpoint dbstat snapshots; transient page reuse is not excluded. Five warm write commits/turn remain.

| Case | Revision | Logical column bytes / 32 turns | Statistical facts | Full statistical checkpoints | Statistical bytes | WAL frames | WAL frame bytes | Write commits |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| short-small | baseline | 1011550 | 192 | 32 | 18656 | 5655 | 23298600 | 160 |
| short-small | complete | 1011526 | 192 | 32 | 18656 | 5655 | 23298600 | 160 |
| short-large | baseline | 1037478 | 192 | 32 | 19109 | 6384 | 26302080 | 160 |
| short-large | complete | 1037444 | 192 | 32 | 19090 | 6393 | 26339160 | 160 |

The complete logical-byte-by-table and WAL page-owner breakdowns are in `support-audit.json`; selected largest WAL owners follow.

| Case | Revision | WAL owners and frames (largest 12) |
|---|---|---|
| short-small | baseline | `content_frontiers` 498; `events` 280; `events_attempt_kind_idx` 208; `sqlite_autoindex_events_1` 202; `events_trace_kind_idx` 176; `events_kind_idx` 176; `rustx_store` 160; `events_step_kind_idx` 144; `events_trace_owner_idx` 143; `observation_block_headers` 143; `events_aggregate_heads_idx` 128; `observation_headers` 127 |
| short-small | complete | `content_frontiers` 498; `events` 280; `events_attempt_kind_idx` 208; `sqlite_autoindex_events_1` 202; `events_trace_kind_idx` 176; `events_kind_idx` 176; `rustx_store` 160; `events_step_kind_idx` 144; `events_trace_owner_idx` 143; `observation_block_headers` 143; `events_aggregate_heads_idx` 128; `observation_headers` 127 |
| short-large | baseline | `content_frontiers` 607; `events` 288; `events_kind_idx` 268; `events_trace_kind_idx` 268; `events_attempt_kind_idx` 206; `sqlite_autoindex_events_1` 199; `sqlite_autoindex_request_source_pages_1` 173; `events_trace_owner_idx` 169; `rustx_store` 160; `events_aggregate_heads_idx` 158; `observation_block_headers` 148; `events_step_kind_idx` 148 |
| short-large | complete | `content_frontiers` 610; `events` 288; `events_kind_idx` 268; `events_trace_kind_idx` 268; `events_attempt_kind_idx` 206; `sqlite_autoindex_events_1` 199; `sqlite_autoindex_request_source_pages_1` 179; `events_trace_owner_idx` 169; `rustx_store` 160; `events_aggregate_heads_idx` 158; `observation_block_headers` 148; `events_step_kind_idx` 148 |

## Intrusive diagnostic counters

No timing here is a formal product latency or a CPU percentage. Timers are nested and must not be added. Two independently executed products per mode/revision. A missing old scalar-encode counter stays unavailable.

| Product | Fold hits | Fold misses | History distance: count | Actual read-domain calls | Source median µs | Scalar median µs | Nodes loaded/request median | Warm stable encodes |
|---|---:|---:|---|---:|---:|---:|---:|---:|
| counts-0-no-live-subscriber | 160 | 32 | {'2': 32} | 0 | 853.000000 | 89.000000 | 63.000000 | unavailable |
| counts-0-live-subscriber | 154 | 38 | {'0': 75, '2': 59, '3': 32} | 134 | 863.500000 | 89.500000 | 63.000000 | unavailable |
| counts-1-no-live-subscriber | 160 | 32 | {'2': 32} | 0 | 846.500000 | 88.000000 | 63.000000 | unavailable |
| counts-1-live-subscriber | 150 | 42 | {'0': 77, '2': 60, '3': 32} | 137 | 860.500000 | 88.000000 | 63.000000 | unavailable |
| candidate-counts-0-no-live-subscriber | 192 | 0 | {} | 0 | 752.000000 | 72.000000 | 41.000000 | 0 |
| candidate-counts-0-live-subscriber | 192 | 0 | {'0': 71, '2': 27, '3': 32} | 141 | 762.500000 | 72.000000 | 41.000000 | 0 |
| candidate-counts-1-no-live-subscriber | 192 | 0 | {} | 0 | 752.500000 | 71.000000 | 41.000000 | 0 |
| candidate-counts-1-live-subscriber | 192 | 0 | {'0': 67, '2': 24, '3': 32} | 134 | 794.000000 | 75.000000 | 41.000000 | 0 |

## Existing ordinary diagnostic CPU profiles

Each warm stack is assigned once within the operation partition. Cumulative areas overlap that partition and each other. Samples are diagnostic 250 Hz signals, not elapsed phase measurements. Inherited owner rules group some `runtime_client::trace` producer work under `other_publication`; do not equate that label with publication-only cost. The Source cumulative stack predicate is more direct.

| Profile | Total samples | SQL execute | SQL prepare | Structural encoding | Source navigation | Other | Request Source cumulative samples |
|---|---:|---:|---:|---:|---:|---:|---:|
| no-live-subscriber-0-before-cpu | 72 | 23 | 0 | 4 | 2 | 43 | 5 |
| no-live-subscriber-0-current-cpu | 73 | 17 | 2 | 3 | 2 | 49 | 6 |
| no-live-subscriber-1-current-cpu | 75 | 21 | 0 | 4 | 4 | 46 | 13 |
| no-live-subscriber-1-before-cpu | 78 | 19 | 0 | 3 | 1 | 55 | 6 |
| live-subscriber-0-before-cpu | 91 | 20 | 1 | 3 | 3 | 64 | 7 |
| live-subscriber-0-current-cpu | 95 | 32 | 0 | 4 | 0 | 59 | 7 |

## Retained exclusions and checks

`triads-overlap-excluded`: all 48 trial products excluded from formal performance, not cherry-picked. `extra-products`: 16 correctness-only Rust products, including the overlap; 10 checked before/after requests. `pi-smoke`: one setup product. Exact build/check/command failures remain in the archive. Source/build identities and all 970 input hashes accompany the final 2,378-test, Fmt and Clippy results. The 48 Python harness tests and 24 Node oracle tests pass.
