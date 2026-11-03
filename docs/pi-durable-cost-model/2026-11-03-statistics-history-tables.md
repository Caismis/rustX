# Statistics 历史重构完整配对表

每个数值来自保留原始计数的独立复算。旧→新；百分比先逐对计算。四对为描述样本，不提供置信区间，不计算新 Pi/DSH 倍数。第一候选与最终候选分开，不合并样本。

## 初始候选 ecf0efa00

Fresh Rust only; 32 formal + 12 Statistics + 16 boundary + 4 traced boundary + 8 columns/WAL products, phases separate. Prior explanation stage 40 diagnostic products remains separate. No Pi/DeepSeek runs or new ratios.

### no-live-subscriber / short-small

| 指标 | 旧侧中位数 | 新侧中位数 | 配对变化中位数 % | 配对范围 % |
|---|---:|---:|---:|---:|
| mean_turn_ms | 7.262797 | 7.302453 | -0.025856 | -4.312622, 1.729554 |
| turn_cpu_ms_per_turn | 8.281250 | 8.281250 | 0.000000 | 0.000000, 3.703704 |
| through_cleanup_self_cpu_ms | 835.000000 | 840.000000 | 0.602410 | -4.545455, 2.409639 |
| through_cleanup_process_write_mib | 33.371094 | 33.259766 | -0.333607 | -0.351206, -0.327715 |
| lifecycle_ms | 942.779500 | 940.085500 | -0.281546 | -32.604863, 0.546629 |
| max_sampled_anon_pss_mib | 122.156250 | 121.958984 | -0.089316 | -0.401940, 0.358228 |
| max_sampled_pss_mib | 177.972656 | 177.687500 | -0.113728 | -0.212843, -0.071884 |
| max_sampled_rss_mib | 180.138672 | 179.847656 | -0.087823 | -0.221119, -0.030459 |
| median_measured_turn_ms | 6.693250 | 6.775750 | 1.167657 | 0.095406, 2.496955 |
| measured_turn_write_bytes | 26918912.000000 | 26851328.000000 | -0.251066 | -0.273931, -0.243420 |
| turn_write_bytes_per_turn | 841216.000000 | 839104.000000 | -0.251066 | -0.273931, -0.243420 |
| through_cleanup_process_write_bytes | 34992128.000000 | 34875392.000000 | -0.333607 | -0.351206, -0.327715 |
| lifecycle_self_cpu_ms | 835.000000 | 840.000000 | 0.602410 | -4.545455, 2.409639 |
| measured_turn_cpu_ms | 265.000000 | 265.000000 | 0.000000 | 0.000000, 3.703704 |
| max_sampled_turn_rss_mib | 155.226562 | 155.228516 | -0.116890 | -0.264424, 0.188907 |
| through_product_exit_wall_ms | 1000.653612 | 998.387121 | -0.485835 | -31.433304, 0.714096 |
| complete_harness_lifecycle_wall_ms | 1048.261604 | 1046.812259 | -0.160168 | -31.088952, 0.579607 |
| closed_logical_bytes | 6680359.000000 | 6639351.000000 | -0.613859 | -0.675533, -0.613859 |
| closed_allocated_bytes | 6696960.000000 | 6656000.000000 | -0.611621 | -0.672783, -0.611621 |

| 指标 | pair | 旧 | 新 | 变化 % | 比例不可用原因 |
|---|---:|---:|---:|---:|---|
| mean_turn_ms | 0 | 8.980906 | 8.593594 | -4.312622 |  |
| mean_turn_ms | 1 | 7.155594 | 7.196562 | 0.572542 |  |
| mean_turn_ms | 2 | 7.303719 | 7.258125 | -0.624254 |  |
| mean_turn_ms | 3 | 7.221875 | 7.346781 | 1.729554 |  |
| turn_cpu_ms_per_turn | 0 | 8.437500 | 8.750000 | 3.703704 |  |
| turn_cpu_ms_per_turn | 1 | 8.125000 | 8.125000 | 0.000000 |  |
| turn_cpu_ms_per_turn | 2 | 8.125000 | 8.125000 | 0.000000 |  |
| turn_cpu_ms_per_turn | 3 | 8.437500 | 8.437500 | 0.000000 |  |
| through_cleanup_self_cpu_ms | 0 | 880.000000 | 840.000000 | -4.545455 |  |
| through_cleanup_self_cpu_ms | 1 | 830.000000 | 840.000000 | 1.204819 |  |
| through_cleanup_self_cpu_ms | 2 | 830.000000 | 850.000000 | 2.409639 |  |
| through_cleanup_self_cpu_ms | 3 | 840.000000 | 840.000000 | 0.000000 |  |
| through_cleanup_process_write_mib | 0 | 33.367188 | 33.250000 | -0.351206 |  |
| through_cleanup_process_write_mib | 1 | 33.375000 | 33.265625 | -0.327715 |  |
| through_cleanup_process_write_mib | 2 | 33.375000 | 33.265625 | -0.327715 |  |
| through_cleanup_process_write_mib | 3 | 33.367188 | 33.253906 | -0.339499 |  |
| lifecycle_ms | 0 | 1501.503000 | 1011.940000 | -32.604863 |  |
| lifecycle_ms | 1 | 928.234000 | 933.308000 | 0.546629 |  |
| lifecycle_ms | 2 | 937.605000 | 942.149000 | 0.484639 |  |
| lifecycle_ms | 3 | 947.954000 | 938.022000 | -1.047730 |  |
| max_sampled_anon_pss_mib | 0 | 122.183594 | 121.808594 | -0.306915 |  |
| max_sampled_anon_pss_mib | 1 | 122.453125 | 121.960938 | -0.401940 |  |
| max_sampled_anon_pss_mib | 2 | 121.800781 | 121.957031 | 0.128283 |  |
| max_sampled_anon_pss_mib | 3 | 122.128906 | 122.566406 | 0.358228 |  |
| max_sampled_pss_mib | 0 | 178.021484 | 177.642578 | -0.212843 |  |
| max_sampled_pss_mib | 1 | 177.978516 | 177.732422 | -0.138272 |  |
| max_sampled_pss_mib | 2 | 177.388672 | 177.230469 | -0.089184 |  |
| max_sampled_pss_mib | 3 | 177.966797 | 177.838867 | -0.071884 |  |
| max_sampled_rss_mib | 0 | 180.191406 | 179.792969 | -0.221119 |  |
| max_sampled_rss_mib | 1 | 180.136719 | 179.902344 | -0.130110 |  |
| max_sampled_rss_mib | 2 | 179.546875 | 179.492188 | -0.030459 |  |
| max_sampled_rss_mib | 3 | 180.140625 | 180.058594 | -0.045537 |  |
| median_measured_turn_ms | 0 | 6.803500 | 6.858000 | 0.801058 |  |
| median_measured_turn_ms | 1 | 6.583000 | 6.684000 | 1.534255 |  |
| median_measured_turn_ms | 2 | 6.813000 | 6.819500 | 0.095406 |  |
| median_measured_turn_ms | 3 | 6.568000 | 6.732000 | 2.496955 |  |
| measured_turn_write_bytes | 0 | 26914816.000000 | 26841088.000000 | -0.273931 |  |
| measured_turn_write_bytes | 1 | 26923008.000000 | 26857472.000000 | -0.243420 |  |
| measured_turn_write_bytes | 2 | 26923008.000000 | 26857472.000000 | -0.243420 |  |
| measured_turn_write_bytes | 3 | 26914816.000000 | 26845184.000000 | -0.258713 |  |
| turn_write_bytes_per_turn | 0 | 841088.000000 | 838784.000000 | -0.273931 |  |
| turn_write_bytes_per_turn | 1 | 841344.000000 | 839296.000000 | -0.243420 |  |
| turn_write_bytes_per_turn | 2 | 841344.000000 | 839296.000000 | -0.243420 |  |
| turn_write_bytes_per_turn | 3 | 841088.000000 | 838912.000000 | -0.258713 |  |
| through_cleanup_process_write_bytes | 0 | 34988032.000000 | 34865152.000000 | -0.351206 |  |
| through_cleanup_process_write_bytes | 1 | 34996224.000000 | 34881536.000000 | -0.327715 |  |
| through_cleanup_process_write_bytes | 2 | 34996224.000000 | 34881536.000000 | -0.327715 |  |
| through_cleanup_process_write_bytes | 3 | 34988032.000000 | 34869248.000000 | -0.339499 |  |
| lifecycle_self_cpu_ms | 0 | 880.000000 | 840.000000 | -4.545455 |  |
| lifecycle_self_cpu_ms | 1 | 830.000000 | 840.000000 | 1.204819 |  |
| lifecycle_self_cpu_ms | 2 | 830.000000 | 850.000000 | 2.409639 |  |
| lifecycle_self_cpu_ms | 3 | 840.000000 | 840.000000 | 0.000000 |  |
| measured_turn_cpu_ms | 0 | 270.000000 | 280.000000 | 3.703704 |  |
| measured_turn_cpu_ms | 1 | 260.000000 | 260.000000 | 0.000000 |  |
| measured_turn_cpu_ms | 2 | 260.000000 | 260.000000 | 0.000000 |  |
| measured_turn_cpu_ms | 3 | 270.000000 | 270.000000 | 0.000000 |  |
| max_sampled_turn_rss_mib | 0 | 155.085938 | 155.378906 | 0.188907 |  |
| max_sampled_turn_rss_mib | 1 | 155.339844 | 155.117188 | -0.143335 |  |
| max_sampled_turn_rss_mib | 2 | 155.113281 | 154.703125 | -0.264424 |  |
| max_sampled_turn_rss_mib | 3 | 155.480469 | 155.339844 | -0.090445 |  |
| through_product_exit_wall_ms | 0 | 1563.234828 | 1071.858469 | -31.433304 |  |
| through_product_exit_wall_ms | 1 | 986.745690 | 988.571838 | 0.185068 |  |
| through_product_exit_wall_ms | 2 | 995.112417 | 1002.218477 | 0.714096 |  |
| through_product_exit_wall_ms | 3 | 1006.194806 | 994.555765 | -1.156738 |  |
| complete_harness_lifecycle_wall_ms | 0 | 1633.175683 | 1125.438474 | -31.088952 |  |
| complete_harness_lifecycle_wall_ms | 1 | 1033.812590 | 1039.266911 | 0.527593 |  |
| complete_harness_lifecycle_wall_ms | 2 | 1042.238397 | 1048.279288 | 0.579607 |  |
| complete_harness_lifecycle_wall_ms | 3 | 1054.284810 | 1045.345230 | -0.847928 |  |
| closed_logical_bytes | 0 | 6680359.000000 | 6635231.000000 | -0.675533 |  |
| closed_logical_bytes | 1 | 6680359.000000 | 6639351.000000 | -0.613859 |  |
| closed_logical_bytes | 2 | 6680359.000000 | 6639351.000000 | -0.613859 |  |
| closed_logical_bytes | 3 | 6680359.000000 | 6639351.000000 | -0.613859 |  |
| closed_allocated_bytes | 0 | 6696960.000000 | 6651904.000000 | -0.672783 |  |
| closed_allocated_bytes | 1 | 6696960.000000 | 6656000.000000 | -0.611621 |  |
| closed_allocated_bytes | 2 | 6696960.000000 | 6656000.000000 | -0.611621 |  |
| closed_allocated_bytes | 3 | 6696960.000000 | 6656000.000000 | -0.611621 |  |

### no-live-subscriber / short-large

| 指标 | 旧侧中位数 | 新侧中位数 | 配对变化中位数 % | 配对范围 % |
|---|---:|---:|---:|---:|
| mean_turn_ms | 8.131391 | 8.196500 | 1.085449 | -1.089365, 2.214981 |
| turn_cpu_ms_per_turn | 9.062500 | 9.218750 | 1.724138 | 0.000000, 7.142857 |
| through_cleanup_self_cpu_ms | 2845.000000 | 2795.000000 | -1.399906 | -2.787456, -0.357143 |
| through_cleanup_process_write_mib | 85.902344 | 85.798828 | -0.095495 | -0.150041, -0.077336 |
| lifecycle_ms | 2993.946500 | 2937.702000 | -1.715088 | -3.003915, -0.226184 |
| max_sampled_anon_pss_mib | 201.888672 | 202.482422 | 0.256311 | 0.232113, 0.890334 |
| max_sampled_pss_mib | 257.744141 | 258.353516 | 0.236427 | 0.189512, 0.380539 |
| max_sampled_rss_mib | 259.912109 | 260.599609 | 0.264513 | 0.166850, 0.408562 |
| median_measured_turn_ms | 7.358000 | 7.468750 | 2.970847 | -1.338280, 3.423759 |
| measured_turn_write_bytes | 31696896.000000 | 31690752.000000 | 0.025871 | -0.038760, 0.090416 |
| turn_write_bytes_per_turn | 990528.000000 | 990336.000000 | 0.025871 | -0.038760, 0.090416 |
| through_cleanup_process_write_bytes | 90075136.000000 | 89966592.000000 | -0.095495 | -0.150041, -0.077336 |
| lifecycle_self_cpu_ms | 2845.000000 | 2795.000000 | -1.399906 | -2.787456, -0.357143 |
| measured_turn_cpu_ms | 290.000000 | 295.000000 | 1.724138 | 0.000000, 7.142857 |
| max_sampled_turn_rss_mib | 172.138672 | 172.591797 | 0.345394 | -0.149355, 0.574321 |
| through_product_exit_wall_ms | 3201.557803 | 3149.959729 | -1.684507 | -2.947952, 0.264563 |
| complete_harness_lifecycle_wall_ms | 3364.139853 | 3310.667931 | -1.545619 | -3.074477, 0.314795 |
| closed_logical_bytes | 9410979.000000 | 9388475.000000 | -0.260811 | -0.698265, -0.173113 |
| closed_allocated_bytes | 9428992.000000 | 9406464.000000 | -0.260558 | -0.694746, -0.173837 |

| 指标 | pair | 旧 | 新 | 变化 % | 比例不可用原因 |
|---|---:|---:|---:|---:|---|
| mean_turn_ms | 0 | 8.221531 | 8.131969 | -1.089365 |  |
| mean_turn_ms | 1 | 8.130656 | 8.184812 | 0.666075 |  |
| mean_turn_ms | 2 | 8.132125 | 8.312250 | 2.214981 |  |
| mean_turn_ms | 3 | 8.086500 | 8.208187 | 1.504823 |  |
| turn_cpu_ms_per_turn | 0 | 9.062500 | 9.062500 | 0.000000 |  |
| turn_cpu_ms_per_turn | 1 | 9.062500 | 9.062500 | 0.000000 |  |
| turn_cpu_ms_per_turn | 2 | 9.062500 | 9.375000 | 3.448276 |  |
| turn_cpu_ms_per_turn | 3 | 8.750000 | 9.375000 | 7.142857 |  |
| through_cleanup_self_cpu_ms | 0 | 2870.000000 | 2790.000000 | -2.787456 |  |
| through_cleanup_self_cpu_ms | 1 | 2820.000000 | 2800.000000 | -0.709220 |  |
| through_cleanup_self_cpu_ms | 2 | 2870.000000 | 2810.000000 | -2.090592 |  |
| through_cleanup_self_cpu_ms | 3 | 2800.000000 | 2790.000000 | -0.357143 |  |
| through_cleanup_process_write_mib | 0 | 85.867188 | 85.800781 | -0.077336 |  |
| through_cleanup_process_write_mib | 1 | 85.914062 | 85.785156 | -0.150041 |  |
| through_cleanup_process_write_mib | 2 | 85.914062 | 85.843750 | -0.081841 |  |
| through_cleanup_process_write_mib | 3 | 85.890625 | 85.796875 | -0.109150 |  |
| lifecycle_ms | 0 | 3016.796000 | 2926.174000 | -3.003915 |  |
| lifecycle_ms | 1 | 2971.097000 | 2941.091000 | -1.009930 |  |
| lifecycle_ms | 2 | 3024.652000 | 2951.448000 | -2.420245 |  |
| lifecycle_ms | 3 | 2940.965000 | 2934.313000 | -0.226184 |  |
| max_sampled_anon_pss_mib | 0 | 201.949219 | 202.417969 | 0.232113 |  |
| max_sampled_anon_pss_mib | 1 | 201.828125 | 202.363281 | 0.265154 |  |
| max_sampled_anon_pss_mib | 2 | 201.820312 | 203.617188 | 0.890334 |  |
| max_sampled_anon_pss_mib | 3 | 202.046875 | 202.546875 | 0.247467 |  |
| max_sampled_pss_mib | 0 | 257.651367 | 258.139648 | 0.189512 |  |
| max_sampled_pss_mib | 1 | 257.728516 | 258.386719 | 0.255386 |  |
| max_sampled_pss_mib | 2 | 257.909180 | 258.890625 | 0.380539 |  |
| max_sampled_pss_mib | 3 | 257.759766 | 258.320312 | 0.217469 |  |
| max_sampled_rss_mib | 0 | 259.871094 | 260.304688 | 0.166850 |  |
| max_sampled_rss_mib | 1 | 259.902344 | 260.648438 | 0.287067 |  |
| max_sampled_rss_mib | 2 | 260.058594 | 261.121094 | 0.408562 |  |
| max_sampled_rss_mib | 3 | 259.921875 | 260.550781 | 0.241960 |  |
| median_measured_turn_ms | 0 | 7.547000 | 7.446000 | -1.338280 |  |
| median_measured_turn_ms | 1 | 7.226000 | 7.431000 | 2.836978 |  |
| median_measured_turn_ms | 2 | 7.472500 | 7.704500 | 3.104717 |  |
| median_measured_turn_ms | 3 | 7.243500 | 7.491500 | 3.423759 |  |
| measured_turn_write_bytes | 0 | 31670272.000000 | 31690752.000000 | 0.064666 |  |
| measured_turn_write_bytes | 1 | 31703040.000000 | 31690752.000000 | -0.038760 |  |
| measured_turn_write_bytes | 2 | 31711232.000000 | 31739904.000000 | 0.090416 |  |
| measured_turn_write_bytes | 3 | 31690752.000000 | 31686656.000000 | -0.012925 |  |
| turn_write_bytes_per_turn | 0 | 989696.000000 | 990336.000000 | 0.064666 |  |
| turn_write_bytes_per_turn | 1 | 990720.000000 | 990336.000000 | -0.038760 |  |
| turn_write_bytes_per_turn | 2 | 990976.000000 | 991872.000000 | 0.090416 |  |
| turn_write_bytes_per_turn | 3 | 990336.000000 | 990208.000000 | -0.012925 |  |
| through_cleanup_process_write_bytes | 0 | 90038272.000000 | 89968640.000000 | -0.077336 |  |
| through_cleanup_process_write_bytes | 1 | 90087424.000000 | 89952256.000000 | -0.150041 |  |
| through_cleanup_process_write_bytes | 2 | 90087424.000000 | 90013696.000000 | -0.081841 |  |
| through_cleanup_process_write_bytes | 3 | 90062848.000000 | 89964544.000000 | -0.109150 |  |
| lifecycle_self_cpu_ms | 0 | 2870.000000 | 2790.000000 | -2.787456 |  |
| lifecycle_self_cpu_ms | 1 | 2820.000000 | 2800.000000 | -0.709220 |  |
| lifecycle_self_cpu_ms | 2 | 2870.000000 | 2810.000000 | -2.090592 |  |
| lifecycle_self_cpu_ms | 3 | 2800.000000 | 2790.000000 | -0.357143 |  |
| measured_turn_cpu_ms | 0 | 290.000000 | 290.000000 | 0.000000 |  |
| measured_turn_cpu_ms | 1 | 290.000000 | 290.000000 | 0.000000 |  |
| measured_turn_cpu_ms | 2 | 290.000000 | 300.000000 | 3.448276 |  |
| measured_turn_cpu_ms | 3 | 280.000000 | 300.000000 | 7.142857 |  |
| max_sampled_turn_rss_mib | 0 | 172.617188 | 172.359375 | -0.149355 |  |
| max_sampled_turn_rss_mib | 1 | 171.796875 | 172.664062 | 0.504775 |  |
| max_sampled_turn_rss_mib | 2 | 172.078125 | 173.066406 | 0.574321 |  |
| max_sampled_turn_rss_mib | 3 | 172.199219 | 172.519531 | 0.186013 |  |
| through_product_exit_wall_ms | 0 | 3226.378619 | 3131.266521 | -2.947952 |  |
| through_product_exit_wall_ms | 1 | 3176.736988 | 3148.259703 | -0.896432 |  |
| through_product_exit_wall_ms | 2 | 3231.562816 | 3151.659755 | -2.472583 |  |
| through_product_exit_wall_ms | 3 | 3145.586923 | 3153.908995 | 0.264563 |  |
| complete_harness_lifecycle_wall_ms | 0 | 3400.997541 | 3296.434644 | -3.074477 |  |
| complete_harness_lifecycle_wall_ms | 1 | 3339.423297 | 3307.903282 | -0.943876 |  |
| complete_harness_lifecycle_wall_ms | 2 | 3388.856409 | 3316.085393 | -2.147362 |  |
| complete_harness_lifecycle_wall_ms | 3 | 3303.034802 | 3313.432580 | 0.314795 |  |
| closed_logical_bytes | 0 | 9408895.000000 | 9392607.000000 | -0.173113 |  |
| closed_logical_bytes | 1 | 9408919.000000 | 9388487.000000 | -0.217156 |  |
| closed_logical_bytes | 2 | 9413039.000000 | 9347311.000000 | -0.698265 |  |
| closed_logical_bytes | 3 | 9417135.000000 | 9388463.000000 | -0.304466 |  |
| closed_allocated_bytes | 0 | 9424896.000000 | 9408512.000000 | -0.173837 |  |
| closed_allocated_bytes | 1 | 9424896.000000 | 9404416.000000 | -0.217297 |  |
| closed_allocated_bytes | 2 | 9433088.000000 | 9367552.000000 | -0.694746 |  |
| closed_allocated_bytes | 3 | 9437184.000000 | 9408512.000000 | -0.303819 |  |

### live-subscriber / short-small

| 指标 | 旧侧中位数 | 新侧中位数 | 配对变化中位数 % | 配对范围 % |
|---|---:|---:|---:|---:|
| mean_turn_ms | 8.068891 | 8.602672 | 6.826358 | -2.262674, 21.405007 |
| turn_cpu_ms_per_turn | 10.156250 | 10.937500 | 7.670455 | 0.000000, 15.625000 |
| through_cleanup_self_cpu_ms | 920.000000 | 940.000000 | 2.162354 | -1.075269, 6.593407 |
| through_cleanup_process_write_mib | 33.363281 | 33.259766 | -0.316104 | -0.327830, -0.280998 |
| lifecycle_ms | 985.558500 | 1011.733000 | 2.921068 | -2.082734, 7.643250 |
| max_sampled_anon_pss_mib | 125.998047 | 126.666016 | 0.530487 | 0.040199, 1.218866 |
| max_sampled_pss_mib | 181.850098 | 182.056641 | 0.121402 | -0.070727, 0.392195 |
| max_sampled_rss_mib | 184.007812 | 184.222656 | 0.126354 | -0.067781, 0.387556 |
| median_measured_turn_ms | 7.548250 | 8.125750 | 7.034162 | 0.137877, 22.102660 |
| measured_turn_write_bytes | 26910720.000000 | 26851328.000000 | -0.228294 | -0.243531, -0.182648 |
| turn_write_bytes_per_turn | 840960.000000 | 839104.000000 | -0.228294 | -0.243531, -0.182648 |
| through_cleanup_process_write_bytes | 34983936.000000 | 34875392.000000 | -0.316104 | -0.327830, -0.280998 |
| lifecycle_self_cpu_ms | 920.000000 | 940.000000 | 2.162354 | -1.075269, 6.593407 |
| measured_turn_cpu_ms | 325.000000 | 350.000000 | 7.670455 | 0.000000, 15.625000 |
| max_sampled_turn_rss_mib | 159.968750 | 160.070312 | 0.063496 | -0.114316, 0.102863 |
| through_product_exit_wall_ms | 1043.357938 | 1072.067435 | 2.813476 | -1.656260, 7.430941 |
| complete_harness_lifecycle_wall_ms | 1103.716395 | 1122.895769 | 2.721327 | -0.747039, 5.656399 |
| closed_logical_bytes | 6680356.000000 | 6637288.000000 | -0.644696 | -0.737206, -0.613860 |
| closed_allocated_bytes | 6696960.000000 | 6653952.000000 | -0.642202 | -0.733945, -0.611621 |

| 指标 | pair | 旧 | 新 | 变化 % | 比例不可用原因 |
|---|---:|---:|---:|---:|---|
| mean_turn_ms | 0 | 7.985281 | 9.694531 | 21.405007 |  |
| mean_turn_ms | 1 | 7.942719 | 8.053000 | 1.388457 |  |
| mean_turn_ms | 2 | 8.152500 | 9.152344 | 12.264259 |  |
| mean_turn_ms | 3 | 8.209312 | 8.023563 | -2.262674 |  |
| turn_cpu_ms_per_turn | 0 | 10.000000 | 11.562500 | 15.625000 |  |
| turn_cpu_ms_per_turn | 1 | 10.000000 | 10.625000 | 6.250000 |  |
| turn_cpu_ms_per_turn | 2 | 10.312500 | 11.250000 | 9.090909 |  |
| turn_cpu_ms_per_turn | 3 | 10.312500 | 10.312500 | 0.000000 |  |
| through_cleanup_self_cpu_ms | 0 | 910.000000 | 970.000000 | 6.593407 |  |
| through_cleanup_self_cpu_ms | 1 | 910.000000 | 920.000000 | 1.098901 |  |
| through_cleanup_self_cpu_ms | 2 | 930.000000 | 960.000000 | 3.225806 |  |
| through_cleanup_self_cpu_ms | 3 | 930.000000 | 920.000000 | -1.075269 |  |
| through_cleanup_process_write_mib | 0 | 33.363281 | 33.253906 | -0.327830 |  |
| through_cleanup_process_write_mib | 1 | 33.367188 | 33.265625 | -0.304378 |  |
| through_cleanup_process_write_mib | 2 | 33.363281 | 33.253906 | -0.327830 |  |
| through_cleanup_process_write_mib | 3 | 33.363281 | 33.269531 | -0.280998 |  |
| lifecycle_ms | 0 | 979.230000 | 1054.075000 | 7.643250 |  |
| lifecycle_ms | 1 | 973.297000 | 980.333000 | 0.722904 |  |
| lifecycle_ms | 2 | 991.887000 | 1042.664000 | 5.119232 |  |
| lifecycle_ms | 3 | 1001.664000 | 980.802000 | -2.082734 |  |
| max_sampled_anon_pss_mib | 0 | 125.761719 | 126.660156 | 0.714397 |  |
| max_sampled_anon_pss_mib | 1 | 126.324219 | 126.375000 | 0.040199 |  |
| max_sampled_anon_pss_mib | 2 | 126.234375 | 126.671875 | 0.346578 |  |
| max_sampled_anon_pss_mib | 3 | 125.308594 | 126.835938 | 1.218866 |  |
| max_sampled_pss_mib | 0 | 181.600586 | 181.870117 | 0.148420 |  |
| max_sampled_pss_mib | 1 | 182.099609 | 182.271484 | 0.094385 |  |
| max_sampled_pss_mib | 2 | 182.259766 | 182.130859 | -0.070727 |  |
| max_sampled_pss_mib | 3 | 181.271484 | 181.982422 | 0.392195 |  |
| max_sampled_rss_mib | 0 | 183.750000 | 184.039062 | 0.157313 |  |
| max_sampled_rss_mib | 1 | 184.265625 | 184.441406 | 0.095396 |  |
| max_sampled_rss_mib | 2 | 184.417969 | 184.292969 | -0.067781 |  |
| max_sampled_rss_mib | 3 | 183.441406 | 184.152344 | 0.387556 |  |
| median_measured_turn_ms | 0 | 7.481000 | 9.134500 | 22.102660 |  |
| median_measured_turn_ms | 1 | 7.429500 | 7.585500 | 2.099738 |  |
| median_measured_turn_ms | 2 | 7.703500 | 8.625500 | 11.968586 |  |
| median_measured_turn_ms | 3 | 7.615500 | 7.626000 | 0.137877 |  |
| measured_turn_write_bytes | 0 | 26910720.000000 | 26845184.000000 | -0.243531 |  |
| measured_turn_write_bytes | 1 | 26914816.000000 | 26857472.000000 | -0.213057 |  |
| measured_turn_write_bytes | 2 | 26910720.000000 | 26845184.000000 | -0.243531 |  |
| measured_turn_write_bytes | 3 | 26910720.000000 | 26861568.000000 | -0.182648 |  |
| turn_write_bytes_per_turn | 0 | 840960.000000 | 838912.000000 | -0.243531 |  |
| turn_write_bytes_per_turn | 1 | 841088.000000 | 839296.000000 | -0.213057 |  |
| turn_write_bytes_per_turn | 2 | 840960.000000 | 838912.000000 | -0.243531 |  |
| turn_write_bytes_per_turn | 3 | 840960.000000 | 839424.000000 | -0.182648 |  |
| through_cleanup_process_write_bytes | 0 | 34983936.000000 | 34869248.000000 | -0.327830 |  |
| through_cleanup_process_write_bytes | 1 | 34988032.000000 | 34881536.000000 | -0.304378 |  |
| through_cleanup_process_write_bytes | 2 | 34983936.000000 | 34869248.000000 | -0.327830 |  |
| through_cleanup_process_write_bytes | 3 | 34983936.000000 | 34885632.000000 | -0.280998 |  |
| lifecycle_self_cpu_ms | 0 | 910.000000 | 970.000000 | 6.593407 |  |
| lifecycle_self_cpu_ms | 1 | 910.000000 | 920.000000 | 1.098901 |  |
| lifecycle_self_cpu_ms | 2 | 930.000000 | 960.000000 | 3.225806 |  |
| lifecycle_self_cpu_ms | 3 | 930.000000 | 920.000000 | -1.075269 |  |
| measured_turn_cpu_ms | 0 | 320.000000 | 370.000000 | 15.625000 |  |
| measured_turn_cpu_ms | 1 | 320.000000 | 340.000000 | 6.250000 |  |
| measured_turn_cpu_ms | 2 | 330.000000 | 360.000000 | 9.090909 |  |
| measured_turn_cpu_ms | 3 | 330.000000 | 330.000000 | 0.000000 |  |
| max_sampled_turn_rss_mib | 0 | 159.496094 | 159.660156 | 0.102863 |  |
| max_sampled_turn_rss_mib | 1 | 160.183594 | 160.277344 | 0.058527 |  |
| max_sampled_turn_rss_mib | 2 | 160.601562 | 160.417969 | -0.114316 |  |
| max_sampled_turn_rss_mib | 3 | 159.753906 | 159.863281 | 0.068465 |  |
| through_product_exit_wall_ms | 0 | 1036.855654 | 1113.903782 | 7.430941 |  |
| through_product_exit_wall_ms | 1 | 1031.917037 | 1038.758261 | 0.662963 |  |
| through_product_exit_wall_ms | 2 | 1049.860223 | 1101.975169 | 4.963989 |  |
| through_product_exit_wall_ms | 3 | 1059.711276 | 1042.159701 | -1.656260 |  |
| complete_harness_lifecycle_wall_ms | 0 | 1106.501373 | 1169.089505 | 5.656399 |  |
| complete_harness_lifecycle_wall_ms | 1 | 1076.655746 | 1089.756831 | 1.216831 |  |
| complete_harness_lifecycle_wall_ms | 2 | 1102.969187 | 1149.578707 | 4.225822 |  |
| complete_harness_lifecycle_wall_ms | 3 | 1104.463603 | 1096.212831 | -0.747039 |  |
| closed_logical_bytes | 0 | 6680356.000000 | 6635228.000000 | -0.675533 |  |
| closed_logical_bytes | 1 | 6680356.000000 | 6639348.000000 | -0.613860 |  |
| closed_logical_bytes | 2 | 6680356.000000 | 6631108.000000 | -0.737206 |  |
| closed_logical_bytes | 3 | 6680356.000000 | 6639348.000000 | -0.613860 |  |
| closed_allocated_bytes | 0 | 6696960.000000 | 6651904.000000 | -0.672783 |  |
| closed_allocated_bytes | 1 | 6696960.000000 | 6656000.000000 | -0.611621 |  |
| closed_allocated_bytes | 2 | 6696960.000000 | 6647808.000000 | -0.733945 |  |
| closed_allocated_bytes | 3 | 6696960.000000 | 6656000.000000 | -0.611621 |  |

### live-subscriber / short-large

| 指标 | 旧侧中位数 | 新侧中位数 | 配对变化中位数 % | 配对范围 % |
|---|---:|---:|---:|---:|
| mean_turn_ms | 9.156672 | 9.110750 | -0.007062 | -1.255288, 1.759698 |
| turn_cpu_ms_per_turn | 11.406250 | 11.406250 | 0.000000 | -2.702703, 2.777778 |
| through_cleanup_self_cpu_ms | 3005.000000 | 3010.000000 | 0.333887 | -0.662252, 1.683502 |
| through_cleanup_process_write_mib | 85.878906 | 85.824219 | -0.052308 | -0.181826, -0.027299 |
| lifecycle_ms | 3052.288000 | 3055.438000 | 0.087841 | -0.945882, 1.605433 |
| max_sampled_anon_pss_mib | 206.437500 | 207.757812 | 0.690155 | 0.211153, 1.308623 |
| max_sampled_pss_mib | 262.423828 | 263.467285 | 0.350908 | 0.046343, 0.742488 |
| max_sampled_rss_mib | 264.583984 | 265.656250 | 0.358199 | 0.067661, 0.739374 |
| median_measured_turn_ms | 8.426750 | 8.569500 | 1.978058 | 0.537185, 3.765234 |
| measured_turn_write_bytes | 31682560.000000 | 31721472.000000 | 0.135754 | -0.142027, 0.168132 |
| turn_write_bytes_per_turn | 990080.000000 | 991296.000000 | 0.135754 | -0.142027, 0.168132 |
| through_cleanup_process_write_bytes | 90050560.000000 | 89993216.000000 | -0.052308 | -0.181826, -0.027299 |
| lifecycle_self_cpu_ms | 3005.000000 | 3010.000000 | 0.333887 | -0.662252, 1.683502 |
| measured_turn_cpu_ms | 365.000000 | 365.000000 | 0.000000 | -2.702703, 2.777778 |
| max_sampled_turn_rss_mib | 177.261719 | 178.269531 | 0.435169 | 0.070179, 1.316952 |
| through_product_exit_wall_ms | 3270.912124 | 3271.360509 | -0.222183 | -0.976718, 2.209108 |
| complete_harness_lifecycle_wall_ms | 3432.069317 | 3434.082642 | -0.096663 | -1.124804, 1.884814 |
| closed_logical_bytes | 9408892.000000 | 9378208.000000 | -0.434794 | -0.610741, -0.216902 |
| closed_allocated_bytes | 9428992.000000 | 9396224.000000 | -0.433877 | -0.608167, -0.217202 |

| 指标 | pair | 旧 | 新 | 变化 % | 比例不可用原因 |
|---|---:|---:|---:|---:|---|
| mean_turn_ms | 0 | 8.854500 | 9.010312 | 1.759698 |  |
| mean_turn_ms | 1 | 9.204219 | 9.126094 | -0.848796 |  |
| mean_turn_ms | 2 | 9.211031 | 9.095406 | -1.255288 |  |
| mean_turn_ms | 3 | 9.109125 | 9.185156 | 0.834671 |  |
| turn_cpu_ms_per_turn | 0 | 11.250000 | 11.250000 | 0.000000 |  |
| turn_cpu_ms_per_turn | 1 | 11.562500 | 11.562500 | 0.000000 |  |
| turn_cpu_ms_per_turn | 2 | 11.250000 | 11.562500 | 2.777778 |  |
| turn_cpu_ms_per_turn | 3 | 11.562500 | 11.250000 | -2.702703 |  |
| through_cleanup_self_cpu_ms | 0 | 2970.000000 | 3020.000000 | 1.683502 |  |
| through_cleanup_self_cpu_ms | 1 | 3010.000000 | 3000.000000 | -0.332226 |  |
| through_cleanup_self_cpu_ms | 2 | 3020.000000 | 3000.000000 | -0.662252 |  |
| through_cleanup_self_cpu_ms | 3 | 3000.000000 | 3030.000000 | 1.000000 |  |
| through_cleanup_process_write_mib | 0 | 85.933594 | 85.777344 | -0.181826 |  |
| through_cleanup_process_write_mib | 1 | 85.898438 | 85.851562 | -0.054570 |  |
| through_cleanup_process_write_mib | 2 | 85.859375 | 85.816406 | -0.050045 |  |
| through_cleanup_process_write_mib | 3 | 85.855469 | 85.832031 | -0.027299 |  |
| lifecycle_ms | 0 | 3017.691000 | 3066.138000 | 1.605433 |  |
| lifecycle_ms | 1 | 3058.943000 | 3030.009000 | -0.945882 |  |
| lifecycle_ms | 2 | 3060.307000 | 3044.738000 | -0.508740 |  |
| lifecycle_ms | 3 | 3045.633000 | 3066.478000 | 0.684423 |  |
| max_sampled_anon_pss_mib | 0 | 206.312500 | 207.882812 | 0.761133 |  |
| max_sampled_anon_pss_mib | 1 | 206.562500 | 209.265625 | 1.308623 |  |
| max_sampled_anon_pss_mib | 2 | 207.195312 | 207.632812 | 0.211153 |  |
| max_sampled_anon_pss_mib | 3 | 206.296875 | 207.574219 | 0.619177 |  |
| max_sampled_pss_mib | 0 | 262.275391 | 263.404297 | 0.430428 |  |
| max_sampled_pss_mib | 1 | 262.525391 | 264.474609 | 0.742488 |  |
| max_sampled_pss_mib | 2 | 263.408203 | 263.530273 | 0.046343 |  |
| max_sampled_pss_mib | 3 | 262.322266 | 263.034180 | 0.271389 |  |
| max_sampled_rss_mib | 0 | 264.433594 | 265.562500 | 0.426915 |  |
| max_sampled_rss_mib | 1 | 264.687500 | 266.644531 | 0.739374 |  |
| max_sampled_rss_mib | 2 | 265.570312 | 265.750000 | 0.067661 |  |
| max_sampled_rss_mib | 3 | 264.480469 | 265.246094 | 0.289483 |  |
| median_measured_turn_ms | 0 | 8.377000 | 8.422000 | 0.537185 |  |
| median_measured_turn_ms | 1 | 8.476500 | 8.721500 | 2.890344 |  |
| median_measured_turn_ms | 2 | 8.246500 | 8.557000 | 3.765234 |  |
| median_measured_turn_ms | 3 | 8.491500 | 8.582000 | 1.065772 |  |
| measured_turn_write_bytes | 0 | 31723520.000000 | 31678464.000000 | -0.142027 |  |
| measured_turn_write_bytes | 1 | 31694848.000000 | 31731712.000000 | 0.116309 |  |
| measured_turn_write_bytes | 2 | 31670272.000000 | 31723520.000000 | 0.168132 |  |
| measured_turn_write_bytes | 3 | 31670272.000000 | 31719424.000000 | 0.155199 |  |
| turn_write_bytes_per_turn | 0 | 991360.000000 | 989952.000000 | -0.142027 |  |
| turn_write_bytes_per_turn | 1 | 990464.000000 | 991616.000000 | 0.116309 |  |
| turn_write_bytes_per_turn | 2 | 989696.000000 | 991360.000000 | 0.168132 |  |
| turn_write_bytes_per_turn | 3 | 989696.000000 | 991232.000000 | 0.155199 |  |
| through_cleanup_process_write_bytes | 0 | 90107904.000000 | 89944064.000000 | -0.181826 |  |
| through_cleanup_process_write_bytes | 1 | 90071040.000000 | 90021888.000000 | -0.054570 |  |
| through_cleanup_process_write_bytes | 2 | 90030080.000000 | 89985024.000000 | -0.050045 |  |
| through_cleanup_process_write_bytes | 3 | 90025984.000000 | 90001408.000000 | -0.027299 |  |
| lifecycle_self_cpu_ms | 0 | 2970.000000 | 3020.000000 | 1.683502 |  |
| lifecycle_self_cpu_ms | 1 | 3010.000000 | 3000.000000 | -0.332226 |  |
| lifecycle_self_cpu_ms | 2 | 3020.000000 | 3000.000000 | -0.662252 |  |
| lifecycle_self_cpu_ms | 3 | 3000.000000 | 3030.000000 | 1.000000 |  |
| measured_turn_cpu_ms | 0 | 360.000000 | 360.000000 | 0.000000 |  |
| measured_turn_cpu_ms | 1 | 370.000000 | 370.000000 | 0.000000 |  |
| measured_turn_cpu_ms | 2 | 360.000000 | 370.000000 | 2.777778 |  |
| measured_turn_cpu_ms | 3 | 370.000000 | 360.000000 | -2.702703 |  |
| max_sampled_turn_rss_mib | 0 | 177.410156 | 178.296875 | 0.499813 |  |
| max_sampled_turn_rss_mib | 1 | 176.781250 | 179.109375 | 1.316952 |  |
| max_sampled_turn_rss_mib | 2 | 178.117188 | 178.242188 | 0.070179 |  |
| max_sampled_turn_rss_mib | 3 | 177.113281 | 177.769531 | 0.370526 |  |
| through_product_exit_wall_ms | 0 | 3225.825819 | 3297.087781 | 2.209108 |  |
| through_product_exit_wall_ms | 1 | 3268.367765 | 3236.940854 | -0.961548 |  |
| through_product_exit_wall_ms | 2 | 3284.414296 | 3252.334825 | -0.976718 |  |
| through_product_exit_wall_ms | 3 | 3273.456484 | 3290.386193 | 0.517181 |  |
| complete_harness_lifecycle_wall_ms | 0 | 3399.505812 | 3463.580186 | 1.884814 |  |
| complete_harness_lifecycle_wall_ms | 1 | 3429.991613 | 3391.410934 | -1.124804 |  |
| complete_harness_lifecycle_wall_ms | 2 | 3440.700890 | 3417.835485 | -0.664557 |  |
| complete_harness_lifecycle_wall_ms | 3 | 3434.147020 | 3450.329798 | 0.471231 |  |
| closed_logical_bytes | 0 | 9408892.000000 | 9384388.000000 | -0.260434 |  |
| closed_logical_bytes | 1 | 9408892.000000 | 9351428.000000 | -0.610741 |  |
| closed_logical_bytes | 2 | 9408868.000000 | 9388460.000000 | -0.216902 |  |
| closed_logical_bytes | 3 | 9429468.000000 | 9372028.000000 | -0.609154 |  |
| closed_allocated_bytes | 0 | 9428992.000000 | 9404416.000000 | -0.260643 |  |
| closed_allocated_bytes | 1 | 9428992.000000 | 9371648.000000 | -0.608167 |  |
| closed_allocated_bytes | 2 | 9428992.000000 | 9408512.000000 | -0.217202 |  |
| closed_allocated_bytes | 3 | 9445376.000000 | 9388032.000000 | -0.607112 |  |

### 列值和 WAL（独立诊断）

| 版本 | 历史 | 全表逻辑列值字节 | 贡献条数 / 字节 | 完整 tuple 条数 / 字节 | WAL 帧 | 提交 |
|---|---|---:|---:|---:|---:|---:|
| baseline | short-small | 1049874 | 0 / 0 | 192 / 56993 | 5666 | 160 |
| baseline | short-large | 1075825 | 0 / 0 | 192 / 57471 | 6382 | 160 |
| candidate | short-small | 1011517 | 192 / 10613 | 32 / 8034 | 5657 | 160 |
| candidate | short-large | 1037444 | 192 / 10999 | 32 / 8087 | 6391 | 160 |

### 独立检查原值

首次为进程内首次对应读取，未清内核缓存。strace 产品的时间不属于普通读取性能结果。并发产品的 read_write_bytes 包含同时进行的追加写入。

| case | pair | 版本 | 指标 | 数值 |
|---|---:|---|---|---:|
| statistics-64-plain | 0 | baseline | full_cpu_ms | 10.000000 |
| statistics-64-plain | 0 | baseline | max_rss_kib | 14080.000000 |
| statistics-64-plain | 0 | baseline | tail_first_us | 31.290000 |
| statistics-64-plain | 0 | baseline | tail_repeat_us | 9.515000 |
| statistics-64-plain | 0 | baseline | checkpoint_first_us | 9.946000 |
| statistics-64-plain | 0 | baseline | checkpoint_repeat_us | 9.560500 |
| statistics-64-plain | 0 | baseline | append_ms | 10.269421 |
| statistics-64-plain | 0 | baseline | concurrent_read_us | missing |
| statistics-64-plain | 0 | baseline | concurrent_writer_ms | missing |
| statistics-64-plain | 0 | baseline | append_write_bytes | 4632576.000000 |
| statistics-64-plain | 0 | baseline | read_write_bytes | 0.000000 |
| statistics-64-plain | 0 | baseline | full_write_bytes | 5505024.000000 |
| statistics-64-plain | 0 | candidate | full_cpu_ms | 10.000000 |
| statistics-64-plain | 0 | candidate | max_rss_kib | 14080.000000 |
| statistics-64-plain | 0 | candidate | tail_first_us | 138.749000 |
| statistics-64-plain | 0 | candidate | tail_repeat_us | 30.137500 |
| statistics-64-plain | 0 | candidate | checkpoint_first_us | 14.138000 |
| statistics-64-plain | 0 | candidate | checkpoint_repeat_us | 12.551000 |
| statistics-64-plain | 0 | candidate | append_ms | 11.980605 |
| statistics-64-plain | 0 | candidate | concurrent_read_us | missing |
| statistics-64-plain | 0 | candidate | concurrent_writer_ms | missing |
| statistics-64-plain | 0 | candidate | append_write_bytes | 4620288.000000 |
| statistics-64-plain | 0 | candidate | read_write_bytes | 0.000000 |
| statistics-64-plain | 0 | candidate | full_write_bytes | 5492736.000000 |
| statistics-512-plain | 0 | baseline | full_cpu_ms | 50.000000 |
| statistics-512-plain | 0 | baseline | max_rss_kib | 14848.000000 |
| statistics-512-plain | 0 | baseline | tail_first_us | 36.944000 |
| statistics-512-plain | 0 | baseline | tail_repeat_us | 9.285000 |
| statistics-512-plain | 0 | baseline | checkpoint_first_us | 9.934000 |
| statistics-512-plain | 0 | baseline | checkpoint_repeat_us | 9.308500 |
| statistics-512-plain | 0 | baseline | append_ms | 71.632472 |
| statistics-512-plain | 0 | baseline | concurrent_read_us | missing |
| statistics-512-plain | 0 | baseline | concurrent_writer_ms | missing |
| statistics-512-plain | 0 | baseline | append_write_bytes | 34168832.000000 |
| statistics-512-plain | 0 | baseline | read_write_bytes | 0.000000 |
| statistics-512-plain | 0 | baseline | full_write_bytes | 35213312.000000 |
| statistics-512-plain | 0 | candidate | full_cpu_ms | 60.000000 |
| statistics-512-plain | 0 | candidate | max_rss_kib | 15104.000000 |
| statistics-512-plain | 0 | candidate | tail_first_us | 144.054000 |
| statistics-512-plain | 0 | candidate | tail_repeat_us | 28.406000 |
| statistics-512-plain | 0 | candidate | checkpoint_first_us | 13.521000 |
| statistics-512-plain | 0 | candidate | checkpoint_repeat_us | 11.770000 |
| statistics-512-plain | 0 | candidate | append_ms | 68.696908 |
| statistics-512-plain | 0 | candidate | concurrent_read_us | missing |
| statistics-512-plain | 0 | candidate | concurrent_writer_ms | missing |
| statistics-512-plain | 0 | candidate | append_write_bytes | 34156544.000000 |
| statistics-512-plain | 0 | candidate | read_write_bytes | 0.000000 |
| statistics-512-plain | 0 | candidate | full_write_bytes | 35201024.000000 |
| statistics-512-concurrent | 0 | baseline | full_cpu_ms | 60.000000 |
| statistics-512-concurrent | 0 | baseline | max_rss_kib | 15104.000000 |
| statistics-512-concurrent | 0 | baseline | tail_first_us | 33.352000 |
| statistics-512-concurrent | 0 | baseline | tail_repeat_us | 9.395500 |
| statistics-512-concurrent | 0 | baseline | checkpoint_first_us | 10.231000 |
| statistics-512-concurrent | 0 | baseline | checkpoint_repeat_us | 9.500500 |
| statistics-512-concurrent | 0 | baseline | append_ms | 68.208168 |
| statistics-512-concurrent | 0 | baseline | concurrent_read_us | 9.970500 |
| statistics-512-concurrent | 0 | baseline | concurrent_writer_ms | 6.983900 |
| statistics-512-concurrent | 0 | baseline | append_write_bytes | 34168832.000000 |
| statistics-512-concurrent | 0 | baseline | read_write_bytes | 2490368.000000 |
| statistics-512-concurrent | 0 | baseline | full_write_bytes | 37646336.000000 |
| statistics-512-concurrent | 0 | candidate | full_cpu_ms | 70.000000 |
| statistics-512-concurrent | 0 | candidate | max_rss_kib | 15360.000000 |
| statistics-512-concurrent | 0 | candidate | tail_first_us | 145.472000 |
| statistics-512-concurrent | 0 | candidate | tail_repeat_us | 28.895000 |
| statistics-512-concurrent | 0 | candidate | checkpoint_first_us | 13.641000 |
| statistics-512-concurrent | 0 | candidate | checkpoint_repeat_us | 12.160000 |
| statistics-512-concurrent | 0 | candidate | append_ms | 80.411164 |
| statistics-512-concurrent | 0 | candidate | concurrent_read_us | 30.385500 |
| statistics-512-concurrent | 0 | candidate | concurrent_writer_ms | 7.090318 |
| statistics-512-concurrent | 0 | candidate | append_write_bytes | 34156544.000000 |
| statistics-512-concurrent | 0 | candidate | read_write_bytes | 2490368.000000 |
| statistics-512-concurrent | 0 | candidate | full_write_bytes | 37634048.000000 |
| statistics-64-plain | 1 | candidate | full_cpu_ms | 10.000000 |
| statistics-64-plain | 1 | candidate | max_rss_kib | 14592.000000 |
| statistics-64-plain | 1 | candidate | tail_first_us | 126.994000 |
| statistics-64-plain | 1 | candidate | tail_repeat_us | 28.568000 |
| statistics-64-plain | 1 | candidate | checkpoint_first_us | 13.503000 |
| statistics-64-plain | 1 | candidate | checkpoint_repeat_us | 11.993000 |
| statistics-64-plain | 1 | candidate | append_ms | 10.598247 |
| statistics-64-plain | 1 | candidate | concurrent_read_us | missing |
| statistics-64-plain | 1 | candidate | concurrent_writer_ms | missing |
| statistics-64-plain | 1 | candidate | append_write_bytes | 4620288.000000 |
| statistics-64-plain | 1 | candidate | read_write_bytes | 0.000000 |
| statistics-64-plain | 1 | candidate | full_write_bytes | 5492736.000000 |
| statistics-64-plain | 1 | baseline | full_cpu_ms | 10.000000 |
| statistics-64-plain | 1 | baseline | max_rss_kib | 14080.000000 |
| statistics-64-plain | 1 | baseline | tail_first_us | 34.935000 |
| statistics-64-plain | 1 | baseline | tail_repeat_us | 9.672000 |
| statistics-64-plain | 1 | baseline | checkpoint_first_us | 9.855000 |
| statistics-64-plain | 1 | baseline | checkpoint_repeat_us | 9.561500 |
| statistics-64-plain | 1 | baseline | append_ms | 10.504668 |
| statistics-64-plain | 1 | baseline | concurrent_read_us | missing |
| statistics-64-plain | 1 | baseline | concurrent_writer_ms | missing |
| statistics-64-plain | 1 | baseline | append_write_bytes | 4632576.000000 |
| statistics-64-plain | 1 | baseline | read_write_bytes | 0.000000 |
| statistics-64-plain | 1 | baseline | full_write_bytes | 5505024.000000 |
| statistics-512-plain | 1 | candidate | full_cpu_ms | 60.000000 |
| statistics-512-plain | 1 | candidate | max_rss_kib | 14848.000000 |
| statistics-512-plain | 1 | candidate | tail_first_us | 164.029000 |
| statistics-512-plain | 1 | candidate | tail_repeat_us | 29.136000 |
| statistics-512-plain | 1 | candidate | checkpoint_first_us | 14.284000 |
| statistics-512-plain | 1 | candidate | checkpoint_repeat_us | 12.242500 |
| statistics-512-plain | 1 | candidate | append_ms | 68.509346 |
| statistics-512-plain | 1 | candidate | concurrent_read_us | missing |
| statistics-512-plain | 1 | candidate | concurrent_writer_ms | missing |
| statistics-512-plain | 1 | candidate | append_write_bytes | 34156544.000000 |
| statistics-512-plain | 1 | candidate | read_write_bytes | 0.000000 |
| statistics-512-plain | 1 | candidate | full_write_bytes | 35201024.000000 |
| statistics-512-plain | 1 | baseline | full_cpu_ms | 60.000000 |
| statistics-512-plain | 1 | baseline | max_rss_kib | 14848.000000 |
| statistics-512-plain | 1 | baseline | tail_first_us | 41.798000 |
| statistics-512-plain | 1 | baseline | tail_repeat_us | 9.407000 |
| statistics-512-plain | 1 | baseline | checkpoint_first_us | 9.907000 |
| statistics-512-plain | 1 | baseline | checkpoint_repeat_us | 9.436500 |
| statistics-512-plain | 1 | baseline | append_ms | 68.414040 |
| statistics-512-plain | 1 | baseline | concurrent_read_us | missing |
| statistics-512-plain | 1 | baseline | concurrent_writer_ms | missing |
| statistics-512-plain | 1 | baseline | append_write_bytes | 34168832.000000 |
| statistics-512-plain | 1 | baseline | read_write_bytes | 0.000000 |
| statistics-512-plain | 1 | baseline | full_write_bytes | 35213312.000000 |
| statistics-512-concurrent | 1 | candidate | full_cpu_ms | 70.000000 |
| statistics-512-concurrent | 1 | candidate | max_rss_kib | 15616.000000 |
| statistics-512-concurrent | 1 | candidate | tail_first_us | 149.357000 |
| statistics-512-concurrent | 1 | candidate | tail_repeat_us | 27.909000 |
| statistics-512-concurrent | 1 | candidate | checkpoint_first_us | 13.663000 |
| statistics-512-concurrent | 1 | candidate | checkpoint_repeat_us | 11.707500 |
| statistics-512-concurrent | 1 | candidate | append_ms | 73.063488 |
| statistics-512-concurrent | 1 | candidate | concurrent_read_us | 29.477500 |
| statistics-512-concurrent | 1 | candidate | concurrent_writer_ms | 6.806120 |
| statistics-512-concurrent | 1 | candidate | append_write_bytes | 34156544.000000 |
| statistics-512-concurrent | 1 | candidate | read_write_bytes | 2490368.000000 |
| statistics-512-concurrent | 1 | candidate | full_write_bytes | 37634048.000000 |
| statistics-512-concurrent | 1 | baseline | full_cpu_ms | 70.000000 |
| statistics-512-concurrent | 1 | baseline | max_rss_kib | 15360.000000 |
| statistics-512-concurrent | 1 | baseline | tail_first_us | 40.259000 |
| statistics-512-concurrent | 1 | baseline | tail_repeat_us | 9.467000 |
| statistics-512-concurrent | 1 | baseline | checkpoint_first_us | 9.688000 |
| statistics-512-concurrent | 1 | baseline | checkpoint_repeat_us | 9.458500 |
| statistics-512-concurrent | 1 | baseline | append_ms | 68.190003 |
| statistics-512-concurrent | 1 | baseline | concurrent_read_us | 9.756500 |
| statistics-512-concurrent | 1 | baseline | concurrent_writer_ms | 6.890275 |
| statistics-512-concurrent | 1 | baseline | append_write_bytes | 34168832.000000 |
| statistics-512-concurrent | 1 | baseline | read_write_bytes | 2490368.000000 |
| statistics-512-concurrent | 1 | baseline | full_write_bytes | 37646336.000000 |
| boundary-16320 | 0 | baseline | full_cpu_ms | 0.000000 |
| boundary-16320 | 0 | baseline | max_rss_kib | 16640.000000 |
| boundary-16320 | 0 | baseline | first_read_us | 1841.069000 |
| boundary-16320 | 0 | baseline | repeat_read_us | 34.954500 |
| boundary-16320 | 0 | candidate | full_cpu_ms | 0.000000 |
| boundary-16320 | 0 | candidate | max_rss_kib | 16896.000000 |
| boundary-16320 | 0 | candidate | first_read_us | 1845.424000 |
| boundary-16320 | 0 | candidate | repeat_read_us | 34.855000 |
| boundary-16321 | 0 | baseline | full_cpu_ms | 0.000000 |
| boundary-16321 | 0 | baseline | max_rss_kib | 16640.000000 |
| boundary-16321 | 0 | baseline | first_read_us | 1881.326000 |
| boundary-16321 | 0 | baseline | repeat_read_us | 35.208000 |
| boundary-16321 | 0 | candidate | full_cpu_ms | 0.000000 |
| boundary-16321 | 0 | candidate | max_rss_kib | 15872.000000 |
| boundary-16321 | 0 | candidate | first_read_us | 89.121000 |
| boundary-16321 | 0 | candidate | repeat_read_us | 36.052000 |
| boundary-16383 | 0 | baseline | full_cpu_ms | 10.000000 |
| boundary-16383 | 0 | baseline | max_rss_kib | 16896.000000 |
| boundary-16383 | 0 | baseline | first_read_us | 2361.033000 |
| boundary-16383 | 0 | baseline | repeat_read_us | 406.997000 |
| boundary-16383 | 0 | candidate | full_cpu_ms | 10.000000 |
| boundary-16383 | 0 | candidate | max_rss_kib | 16128.000000 |
| boundary-16383 | 0 | candidate | first_read_us | 36.380000 |
| boundary-16383 | 0 | candidate | repeat_read_us | 10.576000 |
| boundary-16384 | 0 | baseline | full_cpu_ms | 10.000000 |
| boundary-16384 | 0 | baseline | max_rss_kib | 16384.000000 |
| boundary-16384 | 0 | baseline | first_read_us | 2303.843000 |
| boundary-16384 | 0 | baseline | repeat_read_us | 402.905000 |
| boundary-16384 | 0 | candidate | full_cpu_ms | 10.000000 |
| boundary-16384 | 0 | candidate | max_rss_kib | 16384.000000 |
| boundary-16384 | 0 | candidate | first_read_us | 36.741000 |
| boundary-16384 | 0 | candidate | repeat_read_us | 10.481500 |
| boundary-16320 | 1 | candidate | full_cpu_ms | 0.000000 |
| boundary-16320 | 1 | candidate | max_rss_kib | 16896.000000 |
| boundary-16320 | 1 | candidate | first_read_us | 1811.042000 |
| boundary-16320 | 1 | candidate | repeat_read_us | 34.730500 |
| boundary-16320 | 1 | baseline | full_cpu_ms | 0.000000 |
| boundary-16320 | 1 | baseline | max_rss_kib | 16640.000000 |
| boundary-16320 | 1 | baseline | first_read_us | 1810.667000 |
| boundary-16320 | 1 | baseline | repeat_read_us | 34.292000 |
| boundary-16321 | 1 | candidate | full_cpu_ms | 10.000000 |
| boundary-16321 | 1 | candidate | max_rss_kib | 16384.000000 |
| boundary-16321 | 1 | candidate | first_read_us | 86.062000 |
| boundary-16321 | 1 | candidate | repeat_read_us | 36.076000 |
| boundary-16321 | 1 | baseline | full_cpu_ms | 0.000000 |
| boundary-16321 | 1 | baseline | max_rss_kib | 16896.000000 |
| boundary-16321 | 1 | baseline | first_read_us | 1835.057000 |
| boundary-16321 | 1 | baseline | repeat_read_us | 34.732500 |
| boundary-16383 | 1 | candidate | full_cpu_ms | 10.000000 |
| boundary-16383 | 1 | candidate | max_rss_kib | 16128.000000 |
| boundary-16383 | 1 | candidate | first_read_us | 38.171000 |
| boundary-16383 | 1 | candidate | repeat_read_us | 10.685500 |
| boundary-16383 | 1 | baseline | full_cpu_ms | 0.000000 |
| boundary-16383 | 1 | baseline | max_rss_kib | 16640.000000 |
| boundary-16383 | 1 | baseline | first_read_us | 2470.786000 |
| boundary-16383 | 1 | baseline | repeat_read_us | 409.382500 |
| boundary-16384 | 1 | candidate | full_cpu_ms | 0.000000 |
| boundary-16384 | 1 | candidate | max_rss_kib | 16384.000000 |
| boundary-16384 | 1 | candidate | first_read_us | 35.691000 |
| boundary-16384 | 1 | candidate | repeat_read_us | 10.905500 |
| boundary-16384 | 1 | baseline | full_cpu_ms | 10.000000 |
| boundary-16384 | 1 | baseline | max_rss_kib | 16640.000000 |
| boundary-16384 | 1 | baseline | first_read_us | 2390.362000 |
| boundary-16384 | 1 | baseline | repeat_read_us | 420.742500 |
| trace-boundary-16383 | 0 | baseline | full_cpu_ms | 30.000000 |
| trace-boundary-16383 | 0 | baseline | max_rss_kib | 16384.000000 |
| trace-boundary-16383 | 0 | baseline | first_read_us | 3820.062000 |
| trace-boundary-16383 | 0 | baseline | repeat_read_us | 1733.086000 |
| trace-boundary-16383 | 0 | baseline | read_temp_index_creations | 9.000000 |
| trace-boundary-16383 | 0 | candidate | full_cpu_ms | 20.000000 |
| trace-boundary-16383 | 0 | candidate | max_rss_kib | 16384.000000 |
| trace-boundary-16383 | 0 | candidate | first_read_us | 69.397000 |
| trace-boundary-16383 | 0 | candidate | repeat_read_us | 34.778500 |
| trace-boundary-16383 | 0 | candidate | read_temp_index_creations | 0.000000 |
| trace-boundary-16384 | 0 | baseline | full_cpu_ms | 30.000000 |
| trace-boundary-16384 | 0 | baseline | max_rss_kib | 16640.000000 |
| trace-boundary-16384 | 0 | baseline | first_read_us | 3797.619000 |
| trace-boundary-16384 | 0 | baseline | repeat_read_us | 1713.414000 |
| trace-boundary-16384 | 0 | baseline | read_temp_index_creations | 9.000000 |
| trace-boundary-16384 | 0 | candidate | full_cpu_ms | 20.000000 |
| trace-boundary-16384 | 0 | candidate | max_rss_kib | 15872.000000 |
| trace-boundary-16384 | 0 | candidate | first_read_us | 73.873000 |
| trace-boundary-16384 | 0 | candidate | repeat_read_us | 35.535500 |
| trace-boundary-16384 | 0 | candidate | read_temp_index_creations | 0.000000 |

## 最终候选 c9d5f73e5

Fresh Rust only; Final candidate: 32 formal + 12 Statistics + 8 columns/WAL products; prior candidate and explanation products retained separately. No Pi/DeepSeek runs or new ratios.

### no-live-subscriber / short-small

| 指标 | 旧侧中位数 | 新侧中位数 | 配对变化中位数 % | 配对范围 % |
|---|---:|---:|---:|---:|
| mean_turn_ms | 7.605594 | 7.164125 | -6.609376 | -9.201472, 2.279461 |
| turn_cpu_ms_per_turn | 8.593750 | 8.125000 | -7.095491 | -13.333333, 0.000000 |
| through_cleanup_self_cpu_ms | 850.000000 | 835.000000 | -1.758029 | -5.617978, 0.000000 |
| through_cleanup_process_write_mib | 33.363281 | 33.261719 | -0.286852 | -0.339459, -0.269415 |
| lifecycle_ms | 967.519500 | 936.864000 | -3.237341 | -4.991606, 0.719932 |
| max_sampled_anon_pss_mib | 122.257812 | 121.722656 | -0.437736 | -0.490086, -0.038325 |
| max_sampled_pss_mib | 177.918945 | 177.374023 | -0.272640 | -0.498148, 0.114221 |
| max_sampled_rss_mib | 180.085938 | 179.535156 | -0.287508 | -0.498667, 0.115015 |
| median_measured_turn_ms | 6.850250 | 6.692500 | -3.551708 | -6.540889, 2.935331 |
| measured_turn_write_bytes | 26910720.000000 | 26853376.000000 | -0.190259 | -0.258673, -0.167530 |
| turn_write_bytes_per_turn | 840960.000000 | 839168.000000 | -0.190259 | -0.258673, -0.167530 |
| through_cleanup_process_write_bytes | 34983936.000000 | 34877440.000000 | -0.286852 | -0.339459, -0.269415 |
| lifecycle_self_cpu_ms | 850.000000 | 835.000000 | -1.758029 | -5.617978, 0.000000 |
| measured_turn_cpu_ms | 275.000000 | 260.000000 | -7.095491 | -13.333333, 0.000000 |
| max_sampled_turn_rss_mib | 155.298828 | 154.861328 | -0.227859 | -0.760962, 0.143595 |
| through_product_exit_wall_ms | 1025.264728 | 995.309013 | -3.112114 | -4.536694, 0.853780 |
| complete_harness_lifecycle_wall_ms | 1072.923921 | 1046.222523 | -2.935095 | -5.861647, 1.232042 |
| closed_logical_bytes | 6680367.000000 | 6637299.000000 | -0.644695 | -0.675532, -0.613858 |
| closed_allocated_bytes | 6696960.000000 | 6653952.000000 | -0.642202 | -0.672783, -0.611621 |

| 指标 | pair | 旧 | 新 | 变化 % | 比例不可用原因 |
|---|---:|---:|---:|---:|---|
| mean_turn_ms | 0 | 7.761844 | 7.154656 | -7.822723 |  |
| mean_turn_ms | 1 | 7.449344 | 7.047375 | -5.396029 |  |
| mean_turn_ms | 2 | 7.988531 | 7.253469 | -9.201472 |  |
| mean_turn_ms | 3 | 7.013719 | 7.173594 | 2.279461 |  |
| turn_cpu_ms_per_turn | 0 | 9.062500 | 8.125000 | -10.344828 |  |
| turn_cpu_ms_per_turn | 1 | 8.125000 | 7.812500 | -3.846154 |  |
| turn_cpu_ms_per_turn | 2 | 9.375000 | 8.125000 | -13.333333 |  |
| turn_cpu_ms_per_turn | 3 | 8.125000 | 8.125000 | 0.000000 |  |
| through_cleanup_self_cpu_ms | 0 | 890.000000 | 840.000000 | -5.617978 |  |
| through_cleanup_self_cpu_ms | 1 | 840.000000 | 830.000000 | -1.190476 |  |
| through_cleanup_self_cpu_ms | 2 | 860.000000 | 840.000000 | -2.325581 |  |
| through_cleanup_self_cpu_ms | 3 | 830.000000 | 830.000000 | 0.000000 |  |
| through_cleanup_process_write_mib | 0 | 33.371094 | 33.257812 | -0.339459 |  |
| through_cleanup_process_write_mib | 1 | 33.363281 | 33.265625 | -0.292706 |  |
| through_cleanup_process_write_mib | 2 | 33.347656 | 33.257812 | -0.269415 |  |
| through_cleanup_process_write_mib | 3 | 33.363281 | 33.269531 | -0.280998 |  |
| lifecycle_ms | 0 | 988.820000 | 939.462000 | -4.991606 |  |
| lifecycle_ms | 1 | 947.635000 | 924.110000 | -2.482496 |  |
| lifecycle_ms | 2 | 987.404000 | 947.985000 | -3.992186 |  |
| lifecycle_ms | 3 | 927.588000 | 934.266000 | 0.719932 |  |
| max_sampled_anon_pss_mib | 0 | 122.289062 | 121.792969 | -0.405673 |  |
| max_sampled_anon_pss_mib | 1 | 121.949219 | 121.351562 | -0.490086 |  |
| max_sampled_anon_pss_mib | 2 | 122.308594 | 122.261719 | -0.038325 |  |
| max_sampled_anon_pss_mib | 3 | 122.226562 | 121.652344 | -0.469799 |  |
| max_sampled_pss_mib | 0 | 178.127930 | 177.631836 | -0.278504 |  |
| max_sampled_pss_mib | 1 | 177.539062 | 177.065430 | -0.266777 |  |
| max_sampled_pss_mib | 2 | 177.834961 | 178.038086 | 0.114221 |  |
| max_sampled_pss_mib | 3 | 178.002930 | 177.116211 | -0.498148 |  |
| max_sampled_rss_mib | 0 | 180.296875 | 179.800781 | -0.275154 |  |
| max_sampled_rss_mib | 1 | 179.769531 | 179.230469 | -0.299863 |  |
| max_sampled_rss_mib | 2 | 180.003906 | 180.210938 | 0.115015 |  |
| max_sampled_rss_mib | 3 | 180.167969 | 179.269531 | -0.498667 |  |
| median_measured_turn_ms | 0 | 7.101000 | 6.652000 | -6.323053 |  |
| median_measured_turn_ms | 1 | 6.599500 | 6.548000 | -0.780362 |  |
| median_measured_turn_ms | 2 | 7.208500 | 6.737000 | -6.540889 |  |
| median_measured_turn_ms | 3 | 6.541000 | 6.733000 | 2.935331 |  |
| measured_turn_write_bytes | 0 | 26918912.000000 | 26849280.000000 | -0.258673 |  |
| measured_turn_write_bytes | 1 | 26910720.000000 | 26857472.000000 | -0.197869 |  |
| measured_turn_write_bytes | 2 | 26894336.000000 | 26849280.000000 | -0.167530 |  |
| measured_turn_write_bytes | 3 | 26910720.000000 | 26861568.000000 | -0.182648 |  |
| turn_write_bytes_per_turn | 0 | 841216.000000 | 839040.000000 | -0.258673 |  |
| turn_write_bytes_per_turn | 1 | 840960.000000 | 839296.000000 | -0.197869 |  |
| turn_write_bytes_per_turn | 2 | 840448.000000 | 839040.000000 | -0.167530 |  |
| turn_write_bytes_per_turn | 3 | 840960.000000 | 839424.000000 | -0.182648 |  |
| through_cleanup_process_write_bytes | 0 | 34992128.000000 | 34873344.000000 | -0.339459 |  |
| through_cleanup_process_write_bytes | 1 | 34983936.000000 | 34881536.000000 | -0.292706 |  |
| through_cleanup_process_write_bytes | 2 | 34967552.000000 | 34873344.000000 | -0.269415 |  |
| through_cleanup_process_write_bytes | 3 | 34983936.000000 | 34885632.000000 | -0.280998 |  |
| lifecycle_self_cpu_ms | 0 | 890.000000 | 840.000000 | -5.617978 |  |
| lifecycle_self_cpu_ms | 1 | 840.000000 | 830.000000 | -1.190476 |  |
| lifecycle_self_cpu_ms | 2 | 860.000000 | 840.000000 | -2.325581 |  |
| lifecycle_self_cpu_ms | 3 | 830.000000 | 830.000000 | 0.000000 |  |
| measured_turn_cpu_ms | 0 | 290.000000 | 260.000000 | -10.344828 |  |
| measured_turn_cpu_ms | 1 | 260.000000 | 250.000000 | -3.846154 |  |
| measured_turn_cpu_ms | 2 | 300.000000 | 260.000000 | -13.333333 |  |
| measured_turn_cpu_ms | 3 | 260.000000 | 260.000000 | 0.000000 |  |
| max_sampled_turn_rss_mib | 0 | 155.812500 | 154.988281 | -0.528981 |  |
| max_sampled_turn_rss_mib | 1 | 154.621094 | 154.734375 | 0.073264 |  |
| max_sampled_turn_rss_mib | 2 | 155.058594 | 155.281250 | 0.143595 |  |
| max_sampled_turn_rss_mib | 3 | 155.539062 | 154.355469 | -0.760962 |  |
| through_product_exit_wall_ms | 0 | 1045.420211 | 997.992695 | -4.536694 |  |
| through_product_exit_wall_ms | 1 | 1005.109245 | 980.901631 | -2.408456 |  |
| through_product_exit_wall_ms | 2 | 1045.791950 | 1005.886914 | -3.815772 |  |
| through_product_exit_wall_ms | 3 | 984.222244 | 992.625332 | 0.853780 |  |
| complete_harness_lifecycle_wall_ms | 0 | 1113.992735 | 1048.694411 | -5.861647 |  |
| complete_harness_lifecycle_wall_ms | 1 | 1049.720780 | 1032.244352 | -1.664864 |  |
| complete_harness_lifecycle_wall_ms | 2 | 1096.127062 | 1050.031340 | -4.205327 |  |
| complete_harness_lifecycle_wall_ms | 3 | 1031.047691 | 1043.750635 | 1.232042 |  |
| closed_logical_bytes | 0 | 6680367.000000 | 6635239.000000 | -0.675532 |  |
| closed_logical_bytes | 1 | 6680367.000000 | 6639359.000000 | -0.613858 |  |
| closed_logical_bytes | 2 | 6680367.000000 | 6635239.000000 | -0.675532 |  |
| closed_logical_bytes | 3 | 6680367.000000 | 6639359.000000 | -0.613858 |  |
| closed_allocated_bytes | 0 | 6696960.000000 | 6651904.000000 | -0.672783 |  |
| closed_allocated_bytes | 1 | 6696960.000000 | 6656000.000000 | -0.611621 |  |
| closed_allocated_bytes | 2 | 6696960.000000 | 6651904.000000 | -0.672783 |  |
| closed_allocated_bytes | 3 | 6696960.000000 | 6656000.000000 | -0.611621 |  |

### no-live-subscriber / short-large

| 指标 | 旧侧中位数 | 新侧中位数 | 配对变化中位数 % | 配对范围 % |
|---|---:|---:|---:|---:|
| mean_turn_ms | 8.062313 | 8.628922 | 6.054946 | 3.979080, 13.401365 |
| turn_cpu_ms_per_turn | 9.062500 | 10.000000 | 8.620690 | 6.666667, 17.857143 |
| through_cleanup_self_cpu_ms | 2800.000000 | 2870.000000 | 2.857143 | 1.428571, 4.332130 |
| through_cleanup_process_write_mib | 85.886719 | 85.794922 | -0.113706 | -0.127313, -0.104607 |
| lifecycle_ms | 2942.810000 | 3023.177000 | 2.732925 | 1.458704, 5.286205 |
| max_sampled_anon_pss_mib | 202.189453 | 202.539062 | 0.114211 | -0.133153, 0.371589 |
| max_sampled_pss_mib | 257.903320 | 257.877930 | -0.067997 | -0.128689, 0.242029 |
| max_sampled_rss_mib | 260.070312 | 260.048828 | -0.065921 | -0.130617, 0.206077 |
| median_measured_turn_ms | 7.401500 | 7.858250 | 5.394945 | 3.615329, 11.333879 |
| measured_turn_write_bytes | 31690752.000000 | 31707136.000000 | 0.051700 | -0.038755, 0.090498 |
| turn_write_bytes_per_turn | 990336.000000 | 990848.000000 | 0.051700 | -0.038755, 0.090498 |
| through_cleanup_process_write_bytes | 90058752.000000 | 89962496.000000 | -0.113706 | -0.127313, -0.104607 |
| lifecycle_self_cpu_ms | 2800.000000 | 2870.000000 | 2.857143 | 1.428571, 4.332130 |
| measured_turn_cpu_ms | 290.000000 | 320.000000 | 8.620690 | 6.666667, 17.857143 |
| max_sampled_turn_rss_mib | 172.212891 | 172.279297 | 0.094192 | -0.409864, 0.354256 |
| through_product_exit_wall_ms | 3148.131392 | 3231.005296 | 2.632319 | 1.422051, 5.087702 |
| complete_harness_lifecycle_wall_ms | 3311.090704 | 3388.575916 | 2.420790 | 1.132517, 4.852234 |
| closed_logical_bytes | 9421263.000000 | 9374087.000000 | -0.522335 | -0.872072, -0.304344 |
| closed_allocated_bytes | 9437184.000000 | 9390080.000000 | -0.520692 | -0.868056, -0.304083 |

| 指标 | pair | 旧 | 新 | 变化 % | 比例不可用原因 |
|---|---:|---:|---:|---:|---|
| mean_turn_ms | 0 | 8.247094 | 8.629719 | 4.639513 |  |
| mean_turn_ms | 1 | 8.028375 | 8.628125 | 7.470379 |  |
| mean_turn_ms | 2 | 7.992656 | 9.063781 | 13.401365 |  |
| mean_turn_ms | 3 | 8.096250 | 8.418406 | 3.979080 |  |
| turn_cpu_ms_per_turn | 0 | 9.375000 | 10.000000 | 6.666667 |  |
| turn_cpu_ms_per_turn | 1 | 9.062500 | 10.000000 | 10.344828 |  |
| turn_cpu_ms_per_turn | 2 | 8.750000 | 10.312500 | 17.857143 |  |
| turn_cpu_ms_per_turn | 3 | 9.062500 | 9.687500 | 6.896552 |  |
| through_cleanup_self_cpu_ms | 0 | 2800.000000 | 2910.000000 | 3.928571 |  |
| through_cleanup_self_cpu_ms | 1 | 2800.000000 | 2840.000000 | 1.428571 |  |
| through_cleanup_self_cpu_ms | 2 | 2770.000000 | 2890.000000 | 4.332130 |  |
| through_cleanup_self_cpu_ms | 3 | 2800.000000 | 2850.000000 | 1.785714 |  |
| through_cleanup_process_write_mib | 0 | 85.886719 | 85.781250 | -0.122800 |  |
| through_cleanup_process_write_mib | 1 | 85.910156 | 85.800781 | -0.127313 |  |
| through_cleanup_process_write_mib | 2 | 85.886719 | 85.796875 | -0.104607 |  |
| through_cleanup_process_write_mib | 3 | 85.882812 | 85.792969 | -0.104612 |  |
| lifecycle_ms | 0 | 2938.277000 | 3054.121000 | 3.942583 |  |
| lifecycle_ms | 1 | 2948.596000 | 2993.511000 | 1.523267 |  |
| lifecycle_ms | 2 | 2899.566000 | 3052.843000 | 5.286205 |  |
| lifecycle_ms | 3 | 2947.343000 | 2990.336000 | 1.458704 |  |
| max_sampled_anon_pss_mib | 0 | 202.421875 | 202.152344 | -0.133153 |  |
| max_sampled_anon_pss_mib | 1 | 202.984375 | 202.910156 | -0.036564 |  |
| max_sampled_anon_pss_mib | 2 | 201.957031 | 202.492188 | 0.264985 |  |
| max_sampled_anon_pss_mib | 3 | 201.835938 | 202.585938 | 0.371589 |  |
| max_sampled_pss_mib | 0 | 258.010742 | 257.678711 | -0.128689 |  |
| max_sampled_pss_mib | 1 | 258.761719 | 258.500000 | -0.101143 |  |
| max_sampled_pss_mib | 2 | 257.795898 | 257.706055 | -0.034851 |  |
| max_sampled_pss_mib | 3 | 257.426758 | 258.049805 | 0.242029 |  |
| max_sampled_rss_mib | 0 | 260.183594 | 259.843750 | -0.130617 |  |
| max_sampled_rss_mib | 1 | 260.972656 | 260.710938 | -0.100286 |  |
| max_sampled_rss_mib | 2 | 259.957031 | 259.875000 | -0.031556 |  |
| max_sampled_rss_mib | 3 | 259.687500 | 260.222656 | 0.206077 |  |
| median_measured_turn_ms | 0 | 7.606500 | 7.881500 | 3.615329 |  |
| median_measured_turn_ms | 1 | 7.359500 | 7.766500 | 5.530267 |  |
| median_measured_turn_ms | 2 | 7.332000 | 8.163000 | 11.333879 |  |
| median_measured_turn_ms | 3 | 7.443500 | 7.835000 | 5.259622 |  |
| measured_turn_write_bytes | 0 | 31690752.000000 | 31707136.000000 | 0.051700 |  |
| measured_turn_write_bytes | 1 | 31707136.000000 | 31694848.000000 | -0.038755 |  |
| measured_turn_write_bytes | 2 | 31682560.000000 | 31711232.000000 | 0.090498 |  |
| measured_turn_write_bytes | 3 | 31690752.000000 | 31707136.000000 | 0.051700 |  |
| turn_write_bytes_per_turn | 0 | 990336.000000 | 990848.000000 | 0.051700 |  |
| turn_write_bytes_per_turn | 1 | 990848.000000 | 990464.000000 | -0.038755 |  |
| turn_write_bytes_per_turn | 2 | 990080.000000 | 990976.000000 | 0.090498 |  |
| turn_write_bytes_per_turn | 3 | 990336.000000 | 990848.000000 | 0.051700 |  |
| through_cleanup_process_write_bytes | 0 | 90058752.000000 | 89948160.000000 | -0.122800 |  |
| through_cleanup_process_write_bytes | 1 | 90083328.000000 | 89968640.000000 | -0.127313 |  |
| through_cleanup_process_write_bytes | 2 | 90058752.000000 | 89964544.000000 | -0.104607 |  |
| through_cleanup_process_write_bytes | 3 | 90054656.000000 | 89960448.000000 | -0.104612 |  |
| lifecycle_self_cpu_ms | 0 | 2800.000000 | 2910.000000 | 3.928571 |  |
| lifecycle_self_cpu_ms | 1 | 2800.000000 | 2840.000000 | 1.428571 |  |
| lifecycle_self_cpu_ms | 2 | 2770.000000 | 2890.000000 | 4.332130 |  |
| lifecycle_self_cpu_ms | 3 | 2800.000000 | 2850.000000 | 1.785714 |  |
| measured_turn_cpu_ms | 0 | 300.000000 | 320.000000 | 6.666667 |  |
| measured_turn_cpu_ms | 1 | 290.000000 | 320.000000 | 10.344828 |  |
| measured_turn_cpu_ms | 2 | 280.000000 | 330.000000 | 17.857143 |  |
| measured_turn_cpu_ms | 3 | 290.000000 | 310.000000 | 6.896552 |  |
| max_sampled_turn_rss_mib | 0 | 172.503906 | 171.796875 | -0.409864 |  |
| max_sampled_turn_rss_mib | 1 | 172.410156 | 172.566406 | 0.090627 |  |
| max_sampled_turn_rss_mib | 2 | 171.824219 | 171.992188 | 0.097756 |  |
| max_sampled_turn_rss_mib | 3 | 172.015625 | 172.625000 | 0.354256 |  |
| through_product_exit_wall_ms | 0 | 3146.982736 | 3265.981342 | 3.781356 |  |
| through_product_exit_wall_ms | 1 | 3156.481432 | 3201.368211 | 1.422051 |  |
| through_product_exit_wall_ms | 2 | 3102.782065 | 3260.642382 | 5.087702 |  |
| through_product_exit_wall_ms | 3 | 3149.280047 | 3195.992772 | 1.483283 |  |
| complete_harness_lifecycle_wall_ms | 0 | 3307.648549 | 3426.311558 | 3.587534 |  |
| complete_harness_lifecycle_wall_ms | 1 | 3319.442150 | 3357.035382 | 1.132517 |  |
| complete_harness_lifecycle_wall_ms | 2 | 3261.844144 | 3420.116451 | 4.852234 |  |
| complete_harness_lifecycle_wall_ms | 3 | 3314.532859 | 3356.098625 | 1.254046 |  |
| closed_logical_bytes | 0 | 9413023.000000 | 9384375.000000 | -0.304344 |  |
| closed_logical_bytes | 1 | 9421287.000000 | 9384399.000000 | -0.391539 |  |
| closed_logical_bytes | 2 | 9425359.000000 | 9363799.000000 | -0.653132 |  |
| closed_logical_bytes | 3 | 9421239.000000 | 9339079.000000 | -0.872072 |  |
| closed_allocated_bytes | 0 | 9428992.000000 | 9400320.000000 | -0.304083 |  |
| closed_allocated_bytes | 1 | 9437184.000000 | 9400320.000000 | -0.390625 |  |
| closed_allocated_bytes | 2 | 9441280.000000 | 9379840.000000 | -0.650759 |  |
| closed_allocated_bytes | 3 | 9437184.000000 | 9355264.000000 | -0.868056 |  |

### live-subscriber / short-small

| 指标 | 旧侧中位数 | 新侧中位数 | 配对变化中位数 % | 配对范围 % |
|---|---:|---:|---:|---:|
| mean_turn_ms | 8.118500 | 8.104172 | -0.427205 | -10.619891, 2.622654 |
| turn_cpu_ms_per_turn | 10.468750 | 10.468750 | -1.470588 | -5.555556, 6.250000 |
| through_cleanup_self_cpu_ms | 925.000000 | 925.000000 | 0.005844 | -3.157895, 2.173913 |
| through_cleanup_process_write_mib | 33.363281 | 33.255859 | -0.333623 | -0.351247, -0.304485 |
| lifecycle_ms | 986.811500 | 988.223000 | 0.143346 | -6.198396, 1.760390 |
| max_sampled_anon_pss_mib | 126.583984 | 126.673828 | -0.264152 | -0.494820, 0.695220 |
| max_sampled_pss_mib | 182.735352 | 182.424805 | -0.375098 | -0.436814, 0.482311 |
| max_sampled_rss_mib | 184.898438 | 184.589844 | -0.370723 | -0.435918, 0.530786 |
| median_measured_turn_ms | 7.673250 | 7.646500 | -0.524441 | -10.800985, 0.241105 |
| measured_turn_write_bytes | 26910720.000000 | 26847232.000000 | -0.251079 | -0.273973, -0.213155 |
| turn_write_bytes_per_turn | 840960.000000 | 838976.000000 | -0.251079 | -0.273973, -0.213155 |
| through_cleanup_process_write_bytes | 34983936.000000 | 34871296.000000 | -0.333623 | -0.351247, -0.304485 |
| lifecycle_self_cpu_ms | 925.000000 | 925.000000 | 0.005844 | -3.157895, 2.173913 |
| measured_turn_cpu_ms | 335.000000 | 335.000000 | -1.470588 | -5.555556, 6.250000 |
| max_sampled_turn_rss_mib | 160.597656 | 160.277344 | -0.329472 | -0.489768, 0.872806 |
| through_product_exit_wall_ms | 1043.678459 | 1048.627055 | 0.474209 | -5.827269, 2.056938 |
| complete_harness_lifecycle_wall_ms | 1101.210554 | 1101.174456 | 0.006156 | -4.897070, 1.533109 |
| closed_logical_bytes | 6680364.000000 | 6633176.000000 | -0.706369 | -0.737205, -0.613859 |
| closed_allocated_bytes | 6696960.000000 | 6651904.000000 | -0.672783 | -0.733945, -0.611621 |

| 指标 | pair | 旧 | 新 | 变化 % | 比例不可用原因 |
|---|---:|---:|---:|---:|---|
| mean_turn_ms | 0 | 8.133250 | 8.130281 | -0.036501 |  |
| mean_turn_ms | 1 | 8.071500 | 8.283188 | 2.622654 |  |
| mean_turn_ms | 2 | 8.103750 | 8.037469 | -0.817908 |  |
| mean_turn_ms | 3 | 9.037875 | 8.078062 | -10.619891 |  |
| turn_cpu_ms_per_turn | 0 | 10.625000 | 10.312500 | -2.941176 |  |
| turn_cpu_ms_per_turn | 1 | 10.000000 | 10.625000 | 6.250000 |  |
| turn_cpu_ms_per_turn | 2 | 10.312500 | 10.312500 | 0.000000 |  |
| turn_cpu_ms_per_turn | 3 | 11.250000 | 10.625000 | -5.555556 |  |
| through_cleanup_self_cpu_ms | 0 | 920.000000 | 930.000000 | 1.086957 |  |
| through_cleanup_self_cpu_ms | 1 | 930.000000 | 920.000000 | -1.075269 |  |
| through_cleanup_self_cpu_ms | 2 | 920.000000 | 940.000000 | 2.173913 |  |
| through_cleanup_self_cpu_ms | 3 | 950.000000 | 920.000000 | -3.157895 |  |
| through_cleanup_process_write_mib | 0 | 33.363281 | 33.246094 | -0.351247 |  |
| through_cleanup_process_write_mib | 1 | 33.375000 | 33.257812 | -0.351124 |  |
| through_cleanup_process_write_mib | 2 | 33.363281 | 33.257812 | -0.316122 |  |
| through_cleanup_process_write_mib | 3 | 33.355469 | 33.253906 | -0.304485 |  |
| lifecycle_ms | 0 | 988.364000 | 987.839000 | -0.053118 |  |
| lifecycle_ms | 1 | 985.259000 | 988.607000 | 0.339809 |  |
| lifecycle_ms | 2 | 982.964000 | 1000.268000 | 1.760390 |  |
| lifecycle_ms | 3 | 1048.368000 | 983.386000 | -6.198396 |  |
| max_sampled_anon_pss_mib | 0 | 127.359375 | 126.933594 | -0.334315 |  |
| max_sampled_anon_pss_mib | 1 | 126.308594 | 125.683594 | -0.494820 |  |
| max_sampled_anon_pss_mib | 2 | 126.859375 | 126.613281 | -0.193989 |  |
| max_sampled_anon_pss_mib | 3 | 125.859375 | 126.734375 | 0.695220 |  |
| max_sampled_pss_mib | 0 | 183.323242 | 182.522461 | -0.436814 |  |
| max_sampled_pss_mib | 1 | 182.397461 | 181.772461 | -0.342658 |  |
| max_sampled_pss_mib | 2 | 183.073242 | 182.327148 | -0.407538 |  |
| max_sampled_pss_mib | 3 | 181.823242 | 182.700195 | 0.482311 |  |
| max_sampled_rss_mib | 0 | 185.492188 | 184.683594 | -0.435918 |  |
| max_sampled_rss_mib | 1 | 184.566406 | 183.929688 | -0.344981 |  |
| max_sampled_rss_mib | 2 | 185.230469 | 184.496094 | -0.396466 |  |
| max_sampled_rss_mib | 3 | 183.984375 | 184.960938 | 0.530786 |  |
| median_measured_turn_ms | 0 | 7.673000 | 7.691500 | 0.241105 |  |
| median_measured_turn_ms | 1 | 7.673500 | 7.687000 | 0.175930 |  |
| median_measured_turn_ms | 2 | 7.593000 | 7.500000 | -1.224812 |  |
| median_measured_turn_ms | 3 | 8.527000 | 7.606000 | -10.800985 |  |
| measured_turn_write_bytes | 0 | 26910720.000000 | 26836992.000000 | -0.273973 |  |
| measured_turn_write_bytes | 1 | 26923008.000000 | 26849280.000000 | -0.273848 |  |
| measured_turn_write_bytes | 2 | 26910720.000000 | 26849280.000000 | -0.228311 |  |
| measured_turn_write_bytes | 3 | 26902528.000000 | 26845184.000000 | -0.213155 |  |
| turn_write_bytes_per_turn | 0 | 840960.000000 | 838656.000000 | -0.273973 |  |
| turn_write_bytes_per_turn | 1 | 841344.000000 | 839040.000000 | -0.273848 |  |
| turn_write_bytes_per_turn | 2 | 840960.000000 | 839040.000000 | -0.228311 |  |
| turn_write_bytes_per_turn | 3 | 840704.000000 | 838912.000000 | -0.213155 |  |
| through_cleanup_process_write_bytes | 0 | 34983936.000000 | 34861056.000000 | -0.351247 |  |
| through_cleanup_process_write_bytes | 1 | 34996224.000000 | 34873344.000000 | -0.351124 |  |
| through_cleanup_process_write_bytes | 2 | 34983936.000000 | 34873344.000000 | -0.316122 |  |
| through_cleanup_process_write_bytes | 3 | 34975744.000000 | 34869248.000000 | -0.304485 |  |
| lifecycle_self_cpu_ms | 0 | 920.000000 | 930.000000 | 1.086957 |  |
| lifecycle_self_cpu_ms | 1 | 930.000000 | 920.000000 | -1.075269 |  |
| lifecycle_self_cpu_ms | 2 | 920.000000 | 940.000000 | 2.173913 |  |
| lifecycle_self_cpu_ms | 3 | 950.000000 | 920.000000 | -3.157895 |  |
| measured_turn_cpu_ms | 0 | 340.000000 | 330.000000 | -2.941176 |  |
| measured_turn_cpu_ms | 1 | 320.000000 | 340.000000 | 6.250000 |  |
| measured_turn_cpu_ms | 2 | 330.000000 | 330.000000 | 0.000000 |  |
| measured_turn_cpu_ms | 3 | 360.000000 | 340.000000 | -5.555556 |  |
| max_sampled_turn_rss_mib | 0 | 160.867188 | 160.234375 | -0.393376 |  |
| max_sampled_turn_rss_mib | 1 | 160.328125 | 159.902344 | -0.265569 |  |
| max_sampled_turn_rss_mib | 2 | 161.109375 | 160.320312 | -0.489768 |  |
| max_sampled_turn_rss_mib | 3 | 159.328125 | 160.718750 | 0.872806 |  |
| through_product_exit_wall_ms | 0 | 1044.219350 | 1047.960933 | 0.358314 |  |
| through_product_exit_wall_ms | 1 | 1043.137569 | 1049.293177 | 0.590105 |  |
| through_product_exit_wall_ms | 2 | 1039.027379 | 1060.399525 | 2.056938 |  |
| through_product_exit_wall_ms | 3 | 1106.386932 | 1041.914793 | -5.827269 |  |
| complete_harness_lifecycle_wall_ms | 0 | 1111.720002 | 1100.798917 | -0.982359 |  |
| complete_harness_lifecycle_wall_ms | 1 | 1090.701105 | 1101.549994 | 0.994671 |  |
| complete_harness_lifecycle_wall_ms | 2 | 1090.395103 | 1107.112044 | 1.533109 |  |
| complete_harness_lifecycle_wall_ms | 3 | 1150.147186 | 1093.823677 | -4.897070 |  |
| closed_logical_bytes | 0 | 6680364.000000 | 6631116.000000 | -0.737205 |  |
| closed_logical_bytes | 1 | 6680364.000000 | 6635236.000000 | -0.675532 |  |
| closed_logical_bytes | 2 | 6680364.000000 | 6639356.000000 | -0.613859 |  |
| closed_logical_bytes | 3 | 6680364.000000 | 6631116.000000 | -0.737205 |  |
| closed_allocated_bytes | 0 | 6696960.000000 | 6647808.000000 | -0.733945 |  |
| closed_allocated_bytes | 1 | 6696960.000000 | 6656000.000000 | -0.611621 |  |
| closed_allocated_bytes | 2 | 6696960.000000 | 6656000.000000 | -0.611621 |  |
| closed_allocated_bytes | 3 | 6696960.000000 | 6647808.000000 | -0.733945 |  |

### live-subscriber / short-large

| 指标 | 旧侧中位数 | 新侧中位数 | 配对变化中位数 % | 配对范围 % |
|---|---:|---:|---:|---:|
| mean_turn_ms | 9.004719 | 9.201672 | 2.444239 | -75.022096, 4.978770 |
| turn_cpu_ms_per_turn | 11.406250 | 11.718750 | 4.129129 | -15.909091, 5.714286 |
| through_cleanup_self_cpu_ms | 3005.000000 | 3020.000000 | 0.830620 | -5.079365, 2.013423 |
| through_cleanup_process_write_mib | 85.906250 | 85.796875 | -0.131860 | -0.154602, -0.068207 |
| lifecycle_ms | 3064.157500 | 3081.411000 | 0.946573 | -42.321515, 2.619978 |
| max_sampled_anon_pss_mib | 206.826172 | 206.853516 | 0.089255 | -1.123029, 0.607742 |
| max_sampled_pss_mib | 262.760254 | 262.817871 | -0.025299 | -1.002842, 0.359873 |
| max_sampled_rss_mib | 265.005859 | 265.015625 | -0.045289 | -0.981466, 0.356843 |
| median_measured_turn_ms | 8.336250 | 8.564500 | 2.655573 | -3.386879, 5.518645 |
| measured_turn_write_bytes | 31705088.000000 | 31705088.000000 | -0.025827 | -0.064608, 0.038755 |
| turn_write_bytes_per_turn | 990784.000000 | 990784.000000 | -0.025827 | -0.064608, 0.038755 |
| through_cleanup_process_write_bytes | 90079232.000000 | 89964544.000000 | -0.131860 | -0.154602, -0.068207 |
| lifecycle_self_cpu_ms | 3005.000000 | 3020.000000 | 0.830620 | -5.079365, 2.013423 |
| measured_turn_cpu_ms | 365.000000 | 375.000000 | 4.129129 | -15.909091, 5.714286 |
| max_sampled_turn_rss_mib | 177.542969 | 177.525391 | -0.049653 | -1.597036, 0.580462 |
| through_product_exit_wall_ms | 3273.871495 | 3291.540096 | 1.371759 | -40.961412, 2.271759 |
| complete_harness_lifecycle_wall_ms | 3441.598366 | 3453.577522 | 1.165585 | -39.858723, 2.069697 |
| closed_logical_bytes | 9415092.000000 | 9376156.000000 | -0.413843 | -0.565714, -0.173546 |
| closed_allocated_bytes | 9431040.000000 | 9392128.000000 | -0.412760 | -0.563747, -0.173762 |

| 指标 | pair | 旧 | 新 | 变化 % | 比例不可用原因 |
|---|---:|---:|---:|---:|---|
| mean_turn_ms | 0 | 8.934656 | 9.090375 | 1.742862 |  |
| mean_turn_ms | 1 | 9.074781 | 9.526594 | 4.978770 |  |
| mean_turn_ms | 2 | 8.929094 | 9.209969 | 3.145616 |  |
| mean_turn_ms | 3 | 36.806031 | 9.193375 | -75.022096 |  |
| turn_cpu_ms_per_turn | 0 | 10.937500 | 11.562500 | 5.714286 |  |
| turn_cpu_ms_per_turn | 1 | 11.250000 | 11.875000 | 5.555556 |  |
| turn_cpu_ms_per_turn | 2 | 11.562500 | 11.875000 | 2.702703 |  |
| turn_cpu_ms_per_turn | 3 | 13.750000 | 11.562500 | -15.909091 |  |
| through_cleanup_self_cpu_ms | 0 | 2980.000000 | 3040.000000 | 2.013423 |  |
| through_cleanup_self_cpu_ms | 1 | 3030.000000 | 3060.000000 | 0.990099 |  |
| through_cleanup_self_cpu_ms | 2 | 2980.000000 | 3000.000000 | 0.671141 |  |
| through_cleanup_self_cpu_ms | 3 | 3150.000000 | 2990.000000 | -5.079365 |  |
| through_cleanup_process_write_mib | 0 | 85.906250 | 85.773438 | -0.154602 |  |
| through_cleanup_process_write_mib | 1 | 85.906250 | 85.847656 | -0.068207 |  |
| through_cleanup_process_write_mib | 2 | 85.917969 | 85.804688 | -0.131848 |  |
| through_cleanup_process_write_mib | 3 | 85.902344 | 85.789062 | -0.131872 |  |
| lifecycle_ms | 0 | 3037.507000 | 3117.089000 | 2.619978 |  |
| lifecycle_ms | 1 | 3090.808000 | 3122.596000 | 1.028469 |  |
| lifecycle_ms | 2 | 3019.623000 | 3045.733000 | 0.864677 |  |
| lifecycle_ms | 3 | 5259.996000 | 3033.886000 | -42.321515 |  |
| max_sampled_anon_pss_mib | 0 | 206.222656 | 207.007812 | 0.380732 |  |
| max_sampled_anon_pss_mib | 1 | 206.964844 | 208.222656 | 0.607742 |  |
| max_sampled_anon_pss_mib | 2 | 206.687500 | 206.269531 | -0.202223 |  |
| max_sampled_anon_pss_mib | 3 | 209.046875 | 206.699219 | -1.123029 |  |
| max_sampled_pss_mib | 0 | 262.249023 | 263.034180 | 0.299393 |  |
| max_sampled_pss_mib | 1 | 262.679688 | 263.625000 | 0.359873 |  |
| max_sampled_pss_mib | 2 | 262.840820 | 261.920898 | -0.349992 |  |
| max_sampled_pss_mib | 3 | 265.261719 | 262.601562 | -1.002842 |  |
| max_sampled_rss_mib | 0 | 264.410156 | 265.199219 | 0.298424 |  |
| max_sampled_rss_mib | 1 | 264.910156 | 265.855469 | 0.356843 |  |
| max_sampled_rss_mib | 2 | 265.101562 | 264.070312 | -0.389002 |  |
| max_sampled_rss_mib | 3 | 267.457031 | 264.832031 | -0.981466 |  |
| median_measured_turn_ms | 0 | 8.128000 | 8.355000 | 2.792815 |  |
| median_measured_turn_ms | 1 | 8.353500 | 8.814500 | 5.518645 |  |
| median_measured_turn_ms | 2 | 8.319000 | 8.528500 | 2.518332 |  |
| median_measured_turn_ms | 3 | 8.902000 | 8.600500 | -3.386879 |  |
| measured_turn_write_bytes | 0 | 31698944.000000 | 31678464.000000 | -0.064608 |  |
| measured_turn_write_bytes | 1 | 31707136.000000 | 31719424.000000 | 0.038755 |  |
| measured_turn_write_bytes | 2 | 31723520.000000 | 31711232.000000 | -0.038735 |  |
| measured_turn_write_bytes | 3 | 31703040.000000 | 31698944.000000 | -0.012920 |  |
| turn_write_bytes_per_turn | 0 | 990592.000000 | 989952.000000 | -0.064608 |  |
| turn_write_bytes_per_turn | 1 | 990848.000000 | 991232.000000 | 0.038755 |  |
| turn_write_bytes_per_turn | 2 | 991360.000000 | 990976.000000 | -0.038735 |  |
| turn_write_bytes_per_turn | 3 | 990720.000000 | 990592.000000 | -0.012920 |  |
| through_cleanup_process_write_bytes | 0 | 90079232.000000 | 89939968.000000 | -0.154602 |  |
| through_cleanup_process_write_bytes | 1 | 90079232.000000 | 90017792.000000 | -0.068207 |  |
| through_cleanup_process_write_bytes | 2 | 90091520.000000 | 89972736.000000 | -0.131848 |  |
| through_cleanup_process_write_bytes | 3 | 90075136.000000 | 89956352.000000 | -0.131872 |  |
| lifecycle_self_cpu_ms | 0 | 2980.000000 | 3040.000000 | 2.013423 |  |
| lifecycle_self_cpu_ms | 1 | 3030.000000 | 3060.000000 | 0.990099 |  |
| lifecycle_self_cpu_ms | 2 | 2980.000000 | 3000.000000 | 0.671141 |  |
| lifecycle_self_cpu_ms | 3 | 3150.000000 | 2990.000000 | -5.079365 |  |
| measured_turn_cpu_ms | 0 | 350.000000 | 370.000000 | 5.714286 |  |
| measured_turn_cpu_ms | 1 | 360.000000 | 380.000000 | 5.555556 |  |
| measured_turn_cpu_ms | 2 | 370.000000 | 380.000000 | 2.702703 |  |
| measured_turn_cpu_ms | 3 | 440.000000 | 370.000000 | -15.909091 |  |
| max_sampled_turn_rss_mib | 0 | 176.750000 | 177.664062 | 0.517150 |  |
| max_sampled_turn_rss_mib | 1 | 177.660156 | 178.691406 | 0.580462 |  |
| max_sampled_turn_rss_mib | 2 | 177.425781 | 176.332031 | -0.616455 |  |
| max_sampled_turn_rss_mib | 3 | 180.265625 | 177.386719 | -1.597036 |  |
| through_product_exit_wall_ms | 0 | 3253.074312 | 3326.976326 | 2.271759 |  |
| through_product_exit_wall_ms | 1 | 3294.668677 | 3347.871138 | 1.614805 |  |
| through_product_exit_wall_ms | 2 | 3219.761973 | 3256.103866 | 1.128714 |  |
| through_product_exit_wall_ms | 3 | 5481.670817 | 3236.301066 | -40.961412 |  |
| complete_harness_lifecycle_wall_ms | 0 | 3421.054027 | 3491.859480 | 2.069697 |  |
| complete_harness_lifecycle_wall_ms | 1 | 3462.142705 | 3504.759365 | 1.230933 |  |
| complete_harness_lifecycle_wall_ms | 2 | 3378.128127 | 3415.295563 | 1.100238 |  |
| complete_harness_lifecycle_wall_ms | 3 | 5640.142527 | 3392.053767 | -39.858723 |  |
| closed_logical_bytes | 0 | 9417140.000000 | 9376156.000000 | -0.435206 |  |
| closed_logical_bytes | 1 | 9429500.000000 | 9376156.000000 | -0.565714 |  |
| closed_logical_bytes | 2 | 9413044.000000 | 9396708.000000 | -0.173546 |  |
| closed_logical_bytes | 3 | 9404804.000000 | 9367892.000000 | -0.392480 |  |
| closed_allocated_bytes | 0 | 9433088.000000 | 9392128.000000 | -0.434216 |  |
| closed_allocated_bytes | 1 | 9445376.000000 | 9392128.000000 | -0.563747 |  |
| closed_allocated_bytes | 2 | 9428992.000000 | 9412608.000000 | -0.173762 |  |
| closed_allocated_bytes | 3 | 9420800.000000 | 9383936.000000 | -0.391304 |  |

### 列值和 WAL（独立诊断）

| 版本 | 历史 | 全表逻辑列值字节 | 贡献条数 / 字节 | 完整 tuple 条数 / 字节 | WAL 帧 | 提交 |
|---|---|---:|---:|---:|---:|---:|
| baseline | short-small | 1049875 | 0 / 0 | 192 / 56993 | 5669 | 160 |
| baseline | short-large | 1075838 | 0 / 0 | 192 / 57477 | 6390 | 160 |
| candidate | short-small | 1011535 | 192 / 10620 | 32 / 8034 | 5656 | 160 |
| candidate | short-large | 1037486 | 192 / 11042 | 32 / 8086 | 6376 | 160 |

### 独立检查原值

首次为进程内首次对应读取，未清内核缓存。strace 产品的时间不属于普通读取性能结果。并发产品的 read_write_bytes 包含同时进行的追加写入。

| case | pair | 版本 | 指标 | 数值 |
|---|---:|---|---|---:|
| statistics-64-plain | 0 | baseline | full_cpu_ms | 30.000000 |
| statistics-64-plain | 0 | baseline | max_rss_kib | 14080.000000 |
| statistics-64-plain | 0 | baseline | tail_first_us | 69.214000 |
| statistics-64-plain | 0 | baseline | tail_repeat_us | 23.334500 |
| statistics-64-plain | 0 | baseline | checkpoint_first_us | 24.047000 |
| statistics-64-plain | 0 | baseline | checkpoint_repeat_us | 23.125500 |
| statistics-64-plain | 0 | baseline | append_ms | 180.839390 |
| statistics-64-plain | 0 | baseline | concurrent_read_us | missing |
| statistics-64-plain | 0 | baseline | concurrent_writer_ms | missing |
| statistics-64-plain | 0 | baseline | append_write_bytes | 4632576.000000 |
| statistics-64-plain | 0 | baseline | read_write_bytes | 0.000000 |
| statistics-64-plain | 0 | baseline | full_write_bytes | 5505024.000000 |
| statistics-64-plain | 0 | candidate | full_cpu_ms | 30.000000 |
| statistics-64-plain | 0 | candidate | max_rss_kib | 14080.000000 |
| statistics-64-plain | 0 | candidate | tail_first_us | 269.173000 |
| statistics-64-plain | 0 | candidate | tail_repeat_us | 58.655500 |
| statistics-64-plain | 0 | candidate | checkpoint_first_us | 29.691000 |
| statistics-64-plain | 0 | candidate | checkpoint_repeat_us | 24.652000 |
| statistics-64-plain | 0 | candidate | append_ms | 163.649711 |
| statistics-64-plain | 0 | candidate | concurrent_read_us | missing |
| statistics-64-plain | 0 | candidate | concurrent_writer_ms | missing |
| statistics-64-plain | 0 | candidate | append_write_bytes | 4620288.000000 |
| statistics-64-plain | 0 | candidate | read_write_bytes | 0.000000 |
| statistics-64-plain | 0 | candidate | full_write_bytes | 5492736.000000 |
| statistics-512-plain | 0 | baseline | full_cpu_ms | 100.000000 |
| statistics-512-plain | 0 | baseline | max_rss_kib | 15104.000000 |
| statistics-512-plain | 0 | baseline | tail_first_us | 35.486000 |
| statistics-512-plain | 0 | baseline | tail_repeat_us | 9.738500 |
| statistics-512-plain | 0 | baseline | checkpoint_first_us | 10.343000 |
| statistics-512-plain | 0 | baseline | checkpoint_repeat_us | 9.857500 |
| statistics-512-plain | 0 | baseline | append_ms | 1202.703613 |
| statistics-512-plain | 0 | baseline | concurrent_read_us | missing |
| statistics-512-plain | 0 | baseline | concurrent_writer_ms | missing |
| statistics-512-plain | 0 | baseline | append_write_bytes | 34168832.000000 |
| statistics-512-plain | 0 | baseline | read_write_bytes | 0.000000 |
| statistics-512-plain | 0 | baseline | full_write_bytes | 35213312.000000 |
| statistics-512-plain | 0 | candidate | full_cpu_ms | 100.000000 |
| statistics-512-plain | 0 | candidate | max_rss_kib | 15104.000000 |
| statistics-512-plain | 0 | candidate | tail_first_us | 198.741000 |
| statistics-512-plain | 0 | candidate | tail_repeat_us | 31.659000 |
| statistics-512-plain | 0 | candidate | checkpoint_first_us | 13.895000 |
| statistics-512-plain | 0 | candidate | checkpoint_repeat_us | 12.223000 |
| statistics-512-plain | 0 | candidate | append_ms | 634.209823 |
| statistics-512-plain | 0 | candidate | concurrent_read_us | missing |
| statistics-512-plain | 0 | candidate | concurrent_writer_ms | missing |
| statistics-512-plain | 0 | candidate | append_write_bytes | 34156544.000000 |
| statistics-512-plain | 0 | candidate | read_write_bytes | 0.000000 |
| statistics-512-plain | 0 | candidate | full_write_bytes | 35201024.000000 |
| statistics-512-concurrent | 0 | baseline | full_cpu_ms | 100.000000 |
| statistics-512-concurrent | 0 | baseline | max_rss_kib | 15616.000000 |
| statistics-512-concurrent | 0 | baseline | tail_first_us | 42.716000 |
| statistics-512-concurrent | 0 | baseline | tail_repeat_us | 10.561500 |
| statistics-512-concurrent | 0 | baseline | checkpoint_first_us | 11.359000 |
| statistics-512-concurrent | 0 | baseline | checkpoint_repeat_us | 10.563500 |
| statistics-512-concurrent | 0 | baseline | append_ms | 185.707723 |
| statistics-512-concurrent | 0 | baseline | concurrent_read_us | 16.725000 |
| statistics-512-concurrent | 0 | baseline | concurrent_writer_ms | 20.737238 |
| statistics-512-concurrent | 0 | baseline | append_write_bytes | 34168832.000000 |
| statistics-512-concurrent | 0 | baseline | read_write_bytes | 2490368.000000 |
| statistics-512-concurrent | 0 | baseline | full_write_bytes | 37646336.000000 |
| statistics-512-concurrent | 0 | candidate | full_cpu_ms | 100.000000 |
| statistics-512-concurrent | 0 | candidate | max_rss_kib | 15616.000000 |
| statistics-512-concurrent | 0 | candidate | tail_first_us | 184.035000 |
| statistics-512-concurrent | 0 | candidate | tail_repeat_us | 29.574000 |
| statistics-512-concurrent | 0 | candidate | checkpoint_first_us | 14.124000 |
| statistics-512-concurrent | 0 | candidate | checkpoint_repeat_us | 12.350500 |
| statistics-512-concurrent | 0 | candidate | append_ms | 173.589691 |
| statistics-512-concurrent | 0 | candidate | concurrent_read_us | 59.862500 |
| statistics-512-concurrent | 0 | candidate | concurrent_writer_ms | 21.861732 |
| statistics-512-concurrent | 0 | candidate | append_write_bytes | 34156544.000000 |
| statistics-512-concurrent | 0 | candidate | read_write_bytes | 2490368.000000 |
| statistics-512-concurrent | 0 | candidate | full_write_bytes | 37634048.000000 |
| statistics-64-plain | 1 | candidate | full_cpu_ms | 10.000000 |
| statistics-64-plain | 1 | candidate | max_rss_kib | 14336.000000 |
| statistics-64-plain | 1 | candidate | tail_first_us | 173.736000 |
| statistics-64-plain | 1 | candidate | tail_repeat_us | 37.105000 |
| statistics-64-plain | 1 | candidate | checkpoint_first_us | 16.376000 |
| statistics-64-plain | 1 | candidate | checkpoint_repeat_us | 14.594000 |
| statistics-64-plain | 1 | candidate | append_ms | 22.694612 |
| statistics-64-plain | 1 | candidate | concurrent_read_us | missing |
| statistics-64-plain | 1 | candidate | concurrent_writer_ms | missing |
| statistics-64-plain | 1 | candidate | append_write_bytes | 4620288.000000 |
| statistics-64-plain | 1 | candidate | read_write_bytes | 0.000000 |
| statistics-64-plain | 1 | candidate | full_write_bytes | 5492736.000000 |
| statistics-64-plain | 1 | baseline | full_cpu_ms | 10.000000 |
| statistics-64-plain | 1 | baseline | max_rss_kib | 14080.000000 |
| statistics-64-plain | 1 | baseline | tail_first_us | 38.591000 |
| statistics-64-plain | 1 | baseline | tail_repeat_us | 10.264000 |
| statistics-64-plain | 1 | baseline | checkpoint_first_us | 10.602000 |
| statistics-64-plain | 1 | baseline | checkpoint_repeat_us | 10.786500 |
| statistics-64-plain | 1 | baseline | append_ms | 21.661542 |
| statistics-64-plain | 1 | baseline | concurrent_read_us | missing |
| statistics-64-plain | 1 | baseline | concurrent_writer_ms | missing |
| statistics-64-plain | 1 | baseline | append_write_bytes | 4632576.000000 |
| statistics-64-plain | 1 | baseline | read_write_bytes | 0.000000 |
| statistics-64-plain | 1 | baseline | full_write_bytes | 5505024.000000 |
| statistics-512-plain | 1 | candidate | full_cpu_ms | 80.000000 |
| statistics-512-plain | 1 | candidate | max_rss_kib | 15104.000000 |
| statistics-512-plain | 1 | candidate | tail_first_us | 170.426000 |
| statistics-512-plain | 1 | candidate | tail_repeat_us | 32.790000 |
| statistics-512-plain | 1 | candidate | checkpoint_first_us | 13.770000 |
| statistics-512-plain | 1 | candidate | checkpoint_repeat_us | 12.657000 |
| statistics-512-plain | 1 | candidate | append_ms | 165.722160 |
| statistics-512-plain | 1 | candidate | concurrent_read_us | missing |
| statistics-512-plain | 1 | candidate | concurrent_writer_ms | missing |
| statistics-512-plain | 1 | candidate | append_write_bytes | 34156544.000000 |
| statistics-512-plain | 1 | candidate | read_write_bytes | 0.000000 |
| statistics-512-plain | 1 | candidate | full_write_bytes | 35201024.000000 |
| statistics-512-plain | 1 | baseline | full_cpu_ms | 90.000000 |
| statistics-512-plain | 1 | baseline | max_rss_kib | 14848.000000 |
| statistics-512-plain | 1 | baseline | tail_first_us | 39.007000 |
| statistics-512-plain | 1 | baseline | tail_repeat_us | 11.141500 |
| statistics-512-plain | 1 | baseline | checkpoint_first_us | 12.006000 |
| statistics-512-plain | 1 | baseline | checkpoint_repeat_us | 11.088000 |
| statistics-512-plain | 1 | baseline | append_ms | 170.210585 |
| statistics-512-plain | 1 | baseline | concurrent_read_us | missing |
| statistics-512-plain | 1 | baseline | concurrent_writer_ms | missing |
| statistics-512-plain | 1 | baseline | append_write_bytes | 34168832.000000 |
| statistics-512-plain | 1 | baseline | read_write_bytes | 0.000000 |
| statistics-512-plain | 1 | baseline | full_write_bytes | 35213312.000000 |
| statistics-512-concurrent | 1 | candidate | full_cpu_ms | 100.000000 |
| statistics-512-concurrent | 1 | candidate | max_rss_kib | 15360.000000 |
| statistics-512-concurrent | 1 | candidate | tail_first_us | 175.732000 |
| statistics-512-concurrent | 1 | candidate | tail_repeat_us | 35.060000 |
| statistics-512-concurrent | 1 | candidate | checkpoint_first_us | 16.156000 |
| statistics-512-concurrent | 1 | candidate | checkpoint_repeat_us | 13.799500 |
| statistics-512-concurrent | 1 | candidate | append_ms | 172.669945 |
| statistics-512-concurrent | 1 | candidate | concurrent_read_us | 47.473000 |
| statistics-512-concurrent | 1 | candidate | concurrent_writer_ms | 20.852962 |
| statistics-512-concurrent | 1 | candidate | append_write_bytes | 34156544.000000 |
| statistics-512-concurrent | 1 | candidate | read_write_bytes | 2490368.000000 |
| statistics-512-concurrent | 1 | candidate | full_write_bytes | 37634048.000000 |
| statistics-512-concurrent | 1 | baseline | full_cpu_ms | 70.000000 |
| statistics-512-concurrent | 1 | baseline | max_rss_kib | 15104.000000 |
| statistics-512-concurrent | 1 | baseline | tail_first_us | 33.563000 |
| statistics-512-concurrent | 1 | baseline | tail_repeat_us | 9.305500 |
| statistics-512-concurrent | 1 | baseline | checkpoint_first_us | 9.846000 |
| statistics-512-concurrent | 1 | baseline | checkpoint_repeat_us | 9.251000 |
| statistics-512-concurrent | 1 | baseline | append_ms | 107.740413 |
| statistics-512-concurrent | 1 | baseline | concurrent_read_us | 17.209500 |
| statistics-512-concurrent | 1 | baseline | concurrent_writer_ms | 12.319038 |
| statistics-512-concurrent | 1 | baseline | append_write_bytes | 34168832.000000 |
| statistics-512-concurrent | 1 | baseline | read_write_bytes | 2490368.000000 |
| statistics-512-concurrent | 1 | baseline | full_write_bytes | 37646336.000000 |

