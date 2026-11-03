# Resource and capacity values

All changes are descriptive. Four paired products per ordinary cell; one process per capacity scenario. Negative change means lower. The initial two-table trial and final one-table trial are separate cohorts.

## Initial two-table trial, schema 92

### Absolute ordinary product medians

| Observer / history | Side | Warm median wall ms | CPU ms/turn | Write KiB/turn | Full-cycle write MiB |
| --- | --- | --- | --- | --- | --- |
| no-live-subscriber / 8 | before | 6.4988 | 7.8125 | 819.5000 | 33.2617 |
| no-live-subscriber / 8 | after | 6.5507 | 7.9688 | 815.6875 | 33.0215 |
| no-live-subscriber / 64 | before | 7.5757 | 9.3750 | 966.9375 | 85.7754 |
| no-live-subscriber / 64 | after | 7.0665 | 8.7500 | 971.4375 | 85.6758 |
| live-subscriber / 8 | before | 7.5377 | 10.1562 | 819.4375 | 33.2598 |
| live-subscriber / 8 | after | 7.7045 | 10.4688 | 815.6250 | 33.0195 |
| live-subscriber / 64 | before | 8.7185 | 12.1875 | 967.4375 | 85.7832 |
| live-subscriber / 64 | after | 8.0435 | 10.9375 | 971.0000 | 85.6699 |

### Paired percentage changes

| Observer / history | Metric | Pair 0 | Pair 1 | Pair 2 | Pair 3 | Median % |
| --- | --- | --- | --- | --- | --- | --- |
| no-live-subscriber / 8 | median_measured_turn_ms | -2.79330 | +3.90613 | +0.41345 | +2.74888 | +1.58116 |
| no-live-subscriber / 8 | mean_turn_ms | -5.03234 | +7.01441 | +0.71288 | +4.86770 | +2.79029 |
| no-live-subscriber / 8 | turn_cpu_ms_per_turn | -11.11111 | +4.00000 | +0.00000 | +8.00000 | +2.00000 |
| no-live-subscriber / 8 | turn_write_bytes_per_turn | -0.51845 | -0.48810 | -0.42715 | -0.42709 | -0.45763 |
| no-live-subscriber / 8 | through_cleanup_process_write_bytes | -0.76318 | -0.73987 | -0.69298 | -0.69289 | -0.71642 |
| no-live-subscriber / 64 | median_measured_turn_ms | -9.26829 | -6.25290 | -7.30129 | -6.76344 | -7.03237 |
| no-live-subscriber / 64 | mean_turn_ms | -7.91619 | -6.48515 | -6.84805 | -6.70206 | -6.77506 |
| no-live-subscriber / 64 | turn_cpu_ms_per_turn | -9.67742 | -6.66667 | -3.44828 | -10.00000 | -8.17204 |
| no-live-subscriber / 64 | turn_write_bytes_per_turn | +0.51700 | +0.47822 | +0.37502 | +0.45255 | +0.46538 |
| no-live-subscriber / 64 | through_cleanup_process_write_bytes | -0.08197 | -0.05465 | -0.11845 | -0.15938 | -0.10021 |
| live-subscriber / 8 | median_measured_turn_ms | -1.32240 | +6.05502 | -2.22105 | +2.43755 | +0.55758 |
| live-subscriber / 8 | mean_turn_ms | +57.96843 | +8.30566 | -2.75576 | +3.27994 | +5.79280 |
| live-subscriber / 8 | turn_cpu_ms_per_turn | -9.30233 | +3.12500 | +3.22581 | +3.03030 | +3.07765 |
| live-subscriber / 8 | turn_write_bytes_per_turn | -0.44248 | -0.44234 | -0.47285 | -0.45767 | -0.45007 |
| live-subscriber / 8 | through_cleanup_process_write_bytes | -0.69314 | -0.70464 | -0.72813 | -0.71647 | -0.71055 |
| live-subscriber / 64 | median_measured_turn_ms | -1.99272 | -11.71100 | -9.44109 | -6.51934 | -7.98022 |
| live-subscriber / 64 | mean_turn_ms | -25.48302 | -10.90599 | -10.36419 | -8.38499 | -10.63509 |
| live-subscriber / 64 | turn_cpu_ms_per_turn | -7.69231 | -10.25641 | -12.50000 | -10.25641 | -10.25641 |
| live-subscriber / 64 | turn_write_bytes_per_turn | +0.32287 | +0.28402 | +0.45243 | +0.42669 | +0.37478 |
| live-subscriber / 64 | through_cleanup_process_write_bytes | -0.15936 | -0.17748 | -0.09564 | -0.11843 | -0.13890 |

### Single diagnostic products

| History | Side | Logical column bytes | WAL frames | Write commits | Consistent image growth bytes | Used-page growth bytes |
| --- | --- | --- | --- | --- | --- | --- |
| 8 | baseline | 1011530 | 5655 | 160 | 1470464 | 1470464 |
| 8 | complete | 1011570 | 5622 | 160 | 1462272 | 1462272 |
| 64 | baseline | 1037450 | 6384 | 160 | 1474560 | 1474560 |
| 64 | complete | 1037453 | 6409 | 160 | 1470464 | 1470464 |

The complete owner breakdown, all 160 transaction page counts, and before/after consistent images are in the raw audit. Owner attribution uses endpoint `dbstat`, with intermediate page reuse not excluded.

## Final model-only layout, schema 93

### Absolute ordinary product medians

| Observer / history | Side | Warm median wall ms | CPU ms/turn | Write KiB/turn | Full-cycle write MiB |
| --- | --- | --- | --- | --- | --- |
| no-live-subscriber / 8 | before | 6.6197 | 8.1250 | 819.2500 | 33.2539 |
| no-live-subscriber / 8 | after | 6.4905 | 7.8125 | 816.3750 | 33.0820 |
| no-live-subscriber / 64 | before | 7.1585 | 8.7500 | 967.1250 | 85.7891 |
| no-live-subscriber / 64 | after | 7.0968 | 8.4375 | 966.1875 | 85.5469 |
| live-subscriber / 8 | before | 7.4990 | 9.6875 | 819.3125 | 33.2559 |
| live-subscriber / 8 | after | 7.2962 | 9.6875 | 816.5625 | 33.0879 |
| live-subscriber / 64 | before | 8.0282 | 11.2500 | 966.3750 | 85.7676 |
| live-subscriber / 64 | after | 8.1315 | 10.9375 | 966.8125 | 85.5508 |

### Paired percentage changes

| Observer / history | Metric | Pair 0 | Pair 1 | Pair 2 | Pair 3 | Median % |
| --- | --- | --- | --- | --- | --- | --- |
| no-live-subscriber / 8 | median_measured_turn_ms | -0.52293 | -6.29842 | -1.13689 | -1.47407 | -1.30548 |
| no-live-subscriber / 8 | mean_turn_ms | -72.42335 | -6.74962 | -1.08317 | -0.07644 | -3.91639 |
| no-live-subscriber / 8 | turn_cpu_ms_per_turn | -12.50000 | -7.40741 | +0.00000 | -4.00000 | -5.70370 |
| no-live-subscriber / 8 | turn_write_bytes_per_turn | -0.38139 | -0.33567 | -0.36619 | -0.32042 | -0.35093 |
| no-live-subscriber / 8 | through_cleanup_process_write_bytes | -0.52860 | -0.50511 | -0.52860 | -0.49336 | -0.51686 |
| no-live-subscriber / 64 | median_measured_turn_ms | -2.74471 | -3.13271 | -0.62012 | +0.58562 | -1.68242 |
| no-live-subscriber / 64 | mean_turn_ms | -0.57718 | -2.96540 | +0.84365 | +0.25729 | -0.15994 |
| no-live-subscriber / 64 | turn_cpu_ms_per_turn | -3.57143 | -6.89655 | -3.57143 | +3.70370 | -3.57143 |
| no-live-subscriber / 64 | turn_write_bytes_per_turn | +0.06462 | -0.15510 | -0.05173 | -0.21944 | -0.10341 |
| no-live-subscriber / 64 | through_cleanup_process_write_bytes | -0.22317 | -0.32324 | -0.27324 | -0.31860 | -0.29592 |
| live-subscriber / 8 | median_measured_turn_ms | -1.98658 | -3.41150 | -0.60146 | -10.18501 | -2.69904 |
| live-subscriber / 8 | mean_turn_ms | -0.72666 | -3.29394 | +0.24519 | -12.91106 | -2.01030 |
| live-subscriber / 8 | turn_cpu_ms_per_turn | +0.00000 | +0.00000 | +0.00000 | -6.06061 | +0.00000 |
| live-subscriber / 8 | turn_write_bytes_per_turn | -0.33562 | -0.36613 | -0.33567 | -0.30520 | -0.33565 |
| live-subscriber / 8 | through_cleanup_process_write_bytes | -0.50505 | -0.52854 | -0.50511 | -0.48167 | -0.50508 |
| live-subscriber / 64 | median_measured_turn_ms | -1.48327 | -0.20345 | +1.18291 | +4.64954 | +0.48973 |
| live-subscriber / 64 | mean_turn_ms | +0.06090 | -4.92515 | -0.76753 | +3.77611 | -0.35332 |
| live-subscriber / 64 | turn_cpu_ms_per_turn | -2.77778 | +0.00000 | -2.70270 | -2.77778 | -2.74024 |
| live-subscriber / 64 | turn_write_bytes_per_turn | +0.06472 | -0.07753 | +0.10352 | +0.19395 | +0.08412 |
| live-subscriber / 64 | through_cleanup_process_write_bytes | -0.22325 | -0.26413 | -0.24596 | -0.18672 | -0.23461 |

### Single diagnostic products

| History | Side | Logical column bytes | WAL frames | Write commits | Consistent image growth bytes | Used-page growth bytes |
| --- | --- | --- | --- | --- | --- | --- |
| 8 | baseline | 1011536 | 5655 | 160 | 1474560 | 1474560 |
| 8 | complete | 1011547 | 5626 | 160 | 1470464 | 1470464 |
| 64 | baseline | 1037447 | 6376 | 160 | 1470464 | 1470464 |
| 64 | complete | 1037468 | 6364 | 160 | 1470464 | 1470464 |

The complete owner breakdown, all 160 transaction page counts, and before/after consistent images are in the raw audit. Owner attribution uses endpoint `dbstat`, with intermediate page reuse not excluded.

## Native App Server scenarios

### capacity-before-refined

Sessions 128, seed turns per session 8, requests 1235, schema [91], diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active | Attachments |
| --- | --- | --- | --- | --- | --- |
| empty_server | 14.1992 | 45.9023 | 0/0/0 | 0 | 0 |
| metadata_only | 30.3281 | 72.7188 | 0/0/0 | 0 | 0 |
| empty_attached_1 | 103.6484 | 153.7578 | 1/0/0 | 0 | 1 |
| empty_attached_8 | 119.5703 | 170.0234 | 8/0/0 | 0 | 8 |
| empty_attached_32 | 162.3555 | 213.5586 | 32/0/0 | 0 | 32 |
| empty_attached_64 | 221.0273 | 273.2305 | 64/0/0 | 0 | 64 |
| empty_attached_128 | 336.5391 | 390.7422 | 128/0/0 | 0 | 128 |
| subscribed_1 | 336.6094 | 391.8125 | 128/0/0 | 0 | 128 |
| subscribed_8 | 336.6797 | 391.8828 | 128/0/0 | 0 | 128 |
| subscribed_32 | 336.6797 | 391.8828 | 128/0/0 | 0 | 128 |
| subscribed_64 | 336.6836 | 391.8867 | 128/0/0 | 0 | 128 |
| subscribed_128 | 336.6836 | 391.8867 | 128/0/0 | 0 | 128 |
| all_subscribed_empty | 336.6836 | 391.8867 | 128/0/0 | 0 | 128 |
| seeded_history_1 | 531.5078 | 595.0234 | 128/0/0 | 0 | 128 |
| seeded_history_8 | 921.3906 | 984.9062 | 128/0/0 | 0 | 128 |
| active_held_1 | 922.4062 | 985.9219 | 128/0/0 | 1 | 128 |
| active_finished_1 | 935.3164 | 998.8320 | 128/0/0 | 0 | 128 |
| active_held_4 | 936.2031 | 999.7188 | 128/0/0 | 4 | 128 |
| active_finished_4 | 959.7773 | 1023.2930 | 128/0/0 | 0 | 128 |
| active_held_8 | 962.3555 | 1025.8711 | 128/0/0 | 8 | 128 |
| active_finished_8 | 1012.4023 | 1075.9180 | 128/0/0 | 0 | 128 |
| extra_controller_connections | 1013.9102 | 1077.8633 | 128/0/0 | 0 | 128 |
| history_browsed | 1017.6992 | 1081.7148 | 128/0/0 | 0 | 128 |
| slow_consumer_held | 1019.0234 | 1083.1016 | 128/0/0 | 1 | 128 |
| slow_consumer_settled | 1019.9023 | 1083.9805 | 128/0/0 | 0 | 128 |
| all_unloaded | 1017.8398 | 1078.7930 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_0 | 1017.8398 | 1078.8555 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_1 | 1017.8398 | 1078.8555 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_2 | 1017.8477 | 1078.8633 | 0/0/0 | 0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | Settled P95 ms | Settled P99 ms | Burst service turns/s | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 5.8078 | 21.8634 | 59.0085 | 59.0085 | 36.8630 | 16.8750 | 913.2500 |
| 4 | 60 | 15.2495 | 37.2610 | 75.0599 | 80.6674 | 61.5440 | 13.7500 | 948.1250 |
| 8 | 120 | 26.0897 | 66.2398 | 115.3015 | 128.4415 | 74.6659 | 13.8281 | 978.9688 |

Consistent used-page bytes: 141553664; free pages: 4; post-exit main bytes: 141570048; WAL bytes: 0; SHM bytes: 0; cancel-to-settled 12.4225 ms.

### capacity-after

Sessions 128, seed turns per session 8, requests 1235, schema [92], diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active | Attachments |
| --- | --- | --- | --- | --- | --- |
| empty_server | 14.1992 | 46.5195 | 0/0/0 | 0 | 0 |
| metadata_only | 30.0078 | 73.0156 | 0/0/0 | 0 | 0 |
| empty_attached_1 | 103.7031 | 154.3047 | 1/0/0 | 0 | 1 |
| empty_attached_8 | 116.9062 | 167.9141 | 8/0/0 | 0 | 8 |
| empty_attached_32 | 162.5391 | 214.2969 | 32/0/0 | 0 | 32 |
| empty_attached_64 | 219.7461 | 272.4805 | 64/0/0 | 0 | 64 |
| empty_attached_128 | 337.3828 | 392.0938 | 128/0/0 | 0 | 128 |
| subscribed_1 | 337.4531 | 393.2266 | 128/0/0 | 0 | 128 |
| subscribed_8 | 337.5234 | 393.2969 | 128/0/0 | 0 | 128 |
| subscribed_32 | 337.5898 | 393.3633 | 128/0/0 | 0 | 128 |
| subscribed_64 | 337.5898 | 393.3633 | 128/0/0 | 0 | 128 |
| subscribed_128 | 337.5898 | 393.3633 | 128/0/0 | 0 | 128 |
| all_subscribed_empty | 337.5898 | 393.3633 | 128/0/0 | 0 | 128 |
| seeded_history_1 | 527.6562 | 591.1641 | 128/0/0 | 0 | 128 |
| seeded_history_8 | 929.1875 | 992.6953 | 128/0/0 | 0 | 128 |
| active_held_1 | 931.0078 | 994.5156 | 128/0/0 | 1 | 128 |
| active_finished_1 | 939.1055 | 1002.6133 | 128/0/0 | 0 | 128 |
| active_held_4 | 939.9609 | 1003.4688 | 128/0/0 | 4 | 128 |
| active_finished_4 | 964.8945 | 1028.4023 | 128/0/0 | 0 | 128 |
| active_held_8 | 968.0742 | 1031.5820 | 128/0/0 | 8 | 128 |
| active_finished_8 | 1017.2305 | 1080.7383 | 128/0/0 | 0 | 128 |
| extra_controller_connections | 1018.7617 | 1082.6445 | 128/0/0 | 0 | 128 |
| history_browsed | 1021.0703 | 1084.9531 | 128/0/0 | 0 | 128 |
| slow_consumer_held | 1022.2383 | 1086.1836 | 128/0/0 | 1 | 128 |
| slow_consumer_settled | 1022.2383 | 1086.1836 | 128/0/0 | 0 | 128 |
| all_unloaded | 1021.2070 | 1081.9023 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_0 | 1021.2070 | 1082.0898 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_1 | 1021.2070 | 1082.0898 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_2 | 1021.2148 | 1082.0977 | 0/0/0 | 0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | Settled P95 ms | Settled P99 ms | Burst service turns/s | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 5.7189 | 15.4798 | 57.7752 | 57.7752 | 41.4536 | 15.6250 | 906.0000 |
| 4 | 60 | 15.8335 | 38.9443 | 78.3178 | 84.3088 | 58.3147 | 14.3750 | 936.3125 |
| 8 | 120 | 24.3218 | 63.6641 | 106.2922 | 110.6522 | 78.4681 | 13.2812 | 971.3125 |

Consistent used-page bytes: 140464128; free pages: 4; post-exit main bytes: 140480512; WAL bytes: 0; SHM bytes: 0; cancel-to-settled 12.7736 ms.

### capacity-cpu-long

Sessions 128, seed turns per session 8, requests 1539, schema [92], diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active | Attachments |
| --- | --- | --- | --- | --- | --- |
| empty_server | 14.3633 | 48.0156 | 0/0/0 | 0 | 0 |
| metadata_only | 28.7070 | 72.1719 | 0/0/0 | 0 | 0 |
| empty_attached_1 | 101.5156 | 152.7617 | 1/0/0 | 0 | 1 |
| empty_attached_8 | 117.5781 | 169.2305 | 8/0/0 | 0 | 8 |
| empty_attached_32 | 161.8164 | 214.2188 | 32/0/0 | 0 | 32 |
| empty_attached_64 | 220.3906 | 273.7695 | 64/0/0 | 0 | 64 |
| empty_attached_128 | 336.1016 | 391.4570 | 128/0/0 | 0 | 128 |
| subscribed_1 | 336.1719 | 392.7148 | 128/0/0 | 0 | 128 |
| subscribed_8 | 336.2383 | 392.7812 | 128/0/0 | 0 | 128 |
| subscribed_32 | 336.2383 | 392.7812 | 128/0/0 | 0 | 128 |
| subscribed_64 | 336.2383 | 392.7812 | 128/0/0 | 0 | 128 |
| subscribed_128 | 336.3047 | 392.8477 | 128/0/0 | 0 | 128 |
| all_subscribed_empty | 336.3047 | 392.8477 | 128/0/0 | 0 | 128 |
| seeded_history_1 | 527.5430 | 591.8203 | 128/0/0 | 0 | 128 |
| seeded_history_8 | 924.9453 | 989.2852 | 128/0/0 | 0 | 128 |
| active_held_8 | 935.9648 | 1004.0469 | 128/0/0 | 8 | 128 |
| active_finished_8 | 1129.3516 | 1200.3086 | 128/0/0 | 0 | 128 |
| extra_controller_connections | 1129.5391 | 1200.8711 | 128/0/0 | 0 | 128 |
| history_browsed | 1130.5586 | 1201.8906 | 128/0/0 | 0 | 128 |
| slow_consumer_held | 1132.2578 | 1203.6523 | 128/0/0 | 1 | 128 |
| slow_consumer_settled | 1132.3125 | 1203.7070 | 128/0/0 | 0 | 128 |
| all_unloaded | 1124.8125 | 1193.0195 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_0 | 1124.8359 | 1193.1055 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_1 | 1124.8477 | 1193.1172 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_2 | 1124.8477 | 1193.1172 | 0/0/0 | 0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | Settled P95 ms | Settled P99 ms | Burst service turns/s | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 8 | 504 | 23.5520 | 67.2628 | 111.1827 | 118.0732 | 77.7810 | 14.1016 | 1027.1328 |

Consistent used-page bytes: 154767360; free pages: 4; post-exit main bytes: 154783744; WAL bytes: 0; SHM bytes: 0; cancel-to-settled 8.5983 ms.

### capacity-heap-bounded

Sessions 8, seed turns per session 1, requests 37, schema [91], diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active | Attachments |
| --- | --- | --- | --- | --- | --- |
| empty_server | 69.3242 | 109.3984 | 0/0/0 | 0 | 0 |
| metadata_only | 75.5039 | 125.9531 | 0/0/0 | 0 | 0 |
| empty_attached_1 | 174.0703 | 232.9844 | 1/0/0 | 0 | 1 |
| empty_attached_8 | 206.6328 | 266.2656 | 8/0/0 | 0 | 8 |
| subscribed_1 | 209.5859 | 270.4688 | 8/0/0 | 0 | 8 |
| subscribed_8 | 210.5898 | 271.4727 | 8/0/0 | 0 | 8 |
| all_subscribed_empty | 210.6094 | 271.4922 | 8/0/0 | 0 | 8 |
| seeded_history_1 | 264.3672 | 333.1875 | 8/0/0 | 0 | 8 |
| active_held_1 | 267.1016 | 335.9219 | 8/0/0 | 1 | 8 |
| active_finished_1 | 268.1055 | 336.9258 | 8/0/0 | 0 | 8 |
| active_held_4 | 270.1641 | 338.9844 | 8/0/0 | 4 | 8 |
| active_finished_4 | 273.9609 | 342.7812 | 8/0/0 | 0 | 8 |
| active_held_8 | 280.0508 | 348.8711 | 8/0/0 | 8 | 8 |
| active_finished_8 | 287.1914 | 356.0117 | 8/0/0 | 0 | 8 |
| extra_controller_connections | 287.5664 | 356.3867 | 8/0/0 | 0 | 8 |
| history_browsed | 290.3984 | 359.2812 | 8/0/0 | 0 | 8 |
| slow_consumer_held | 293.0000 | 362.4453 | 8/0/0 | 1 | 8 |
| slow_consumer_settled | 294.3672 | 363.8125 | 8/0/0 | 0 | 8 |
| all_unloaded | 301.2852 | 371.2305 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_0 | 305.7852 | 375.7930 | 0/0/1 | 0 | 0 |
| reload_unload_cycle_1 | 305.9648 | 375.9727 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_2 | 306.1641 | 376.1719 | 0/0/0 | 0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | Settled P95 ms | Settled P99 ms | Burst service turns/s | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 44.6825 | 494.8126 | 494.8126 | 494.8126 | 2.0210 | 655.0000 | 1118.0000 |
| 4 | 4 | 105.8106 | 2043.5825 | 2150.2717 | 2150.2717 | 1.8602 | 856.2500 | 606.0000 |
| 8 | 8 | 233.3960 | 4068.7251 | 4680.9243 | 4680.9243 | 1.7090 | 901.8750 | 722.2500 |

Consistent used-page bytes: 7241728; free pages: 7; post-exit main bytes: 7270400; WAL bytes: 0; SHM bytes: 0; cancel-to-settled 632.4295 ms.

First heap diagnostic reload cycle sampled unloading=1; later two cycles and all_unloaded are zero. No exact teardown-barrier claim for that point.

### refined/capacity-final

Sessions 128, seed turns per session 8, requests 1235, schema [93], diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active | Attachments |
| --- | --- | --- | --- | --- | --- |
| empty_server | 14.2031 | 46.2695 | 0/0/0 | 0 | 0 |
| metadata_only | 30.2148 | 72.5938 | 0/0/0 | 0 | 0 |
| empty_attached_1 | 102.6211 | 152.6562 | 1/0/0 | 0 | 1 |
| empty_attached_8 | 119.3867 | 169.7656 | 8/0/0 | 0 | 8 |
| empty_attached_32 | 163.0820 | 214.2109 | 32/0/0 | 0 | 32 |
| empty_attached_64 | 220.6133 | 272.7422 | 64/0/0 | 0 | 64 |
| empty_attached_128 | 336.5039 | 390.6328 | 128/0/0 | 0 | 128 |
| subscribed_1 | 336.5742 | 391.8281 | 128/0/0 | 0 | 128 |
| subscribed_8 | 336.7188 | 391.9727 | 128/0/0 | 0 | 128 |
| subscribed_32 | 336.7266 | 391.9805 | 128/0/0 | 0 | 128 |
| subscribed_64 | 336.7266 | 391.9805 | 128/0/0 | 0 | 128 |
| subscribed_128 | 336.7578 | 392.0117 | 128/0/0 | 0 | 128 |
| all_subscribed_empty | 336.7578 | 392.0117 | 128/0/0 | 0 | 128 |
| seeded_history_1 | 526.6641 | 589.7305 | 128/0/0 | 0 | 128 |
| seeded_history_8 | 925.4297 | 988.4961 | 128/0/0 | 0 | 128 |
| active_held_1 | 925.5664 | 988.6328 | 128/0/0 | 1 | 128 |
| active_finished_1 | 935.7812 | 998.8477 | 128/0/0 | 0 | 128 |
| active_held_4 | 938.3008 | 1001.3672 | 128/0/0 | 4 | 128 |
| active_finished_4 | 957.3633 | 1020.4297 | 128/0/0 | 0 | 128 |
| active_held_8 | 960.7656 | 1023.8320 | 128/0/0 | 8 | 128 |
| active_finished_8 | 1017.0664 | 1080.1328 | 128/0/0 | 0 | 128 |
| extra_controller_connections | 1017.6133 | 1081.1797 | 128/0/0 | 0 | 128 |
| history_browsed | 1019.7461 | 1083.3750 | 128/0/0 | 0 | 128 |
| slow_consumer_held | 1021.3477 | 1085.0391 | 128/0/0 | 1 | 128 |
| slow_consumer_settled | 1022.2656 | 1085.9570 | 128/0/0 | 0 | 128 |
| all_unloaded | 1019.4961 | 1080.1875 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_0 | 1019.5039 | 1080.2578 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_1 | 1019.5117 | 1080.2656 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_2 | 1019.5117 | 1080.2656 | 0/0/0 | 0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | Settled P95 ms | Settled P99 ms | Burst service turns/s | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 6.5233 | 19.1409 | 66.2962 | 66.2962 | 33.9481 | 16.2500 | 909.2500 |
| 4 | 60 | 16.3538 | 44.4760 | 79.5243 | 83.2266 | 56.0612 | 13.9062 | 942.3750 |
| 8 | 120 | 21.7049 | 61.9626 | 106.2066 | 111.3916 | 85.4936 | 13.2031 | 975.4375 |

Consistent used-page bytes: 141029376; free pages: 4; post-exit main bytes: 141045760; WAL bytes: 0; SHM bytes: 0; cancel-to-settled 12.9708 ms.

### refined/history-512-before

Sessions 2, seed turns per session 512, requests 1075, schema [91], diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active | Attachments |
| --- | --- | --- | --- | --- | --- |
| empty_server | 14.1953 | 46.3984 | 0/0/0 | 0 | 0 |
| metadata_only | 15.9766 | 58.8672 | 0/0/0 | 0 | 0 |
| empty_attached_1 | 89.1250 | 139.3594 | 1/0/0 | 0 | 1 |
| empty_attached_2 | 93.0547 | 143.5078 | 2/0/0 | 0 | 2 |
| subscribed_1 | 93.3125 | 144.9531 | 2/0/0 | 0 | 2 |
| subscribed_2 | 93.4336 | 145.0742 | 2/0/0 | 0 | 2 |
| all_subscribed_empty | 93.4336 | 145.0742 | 2/0/0 | 0 | 2 |
| seeded_history_1 | 97.7773 | 157.1055 | 2/0/0 | 0 | 2 |
| seeded_history_512 | 193.4570 | 252.7852 | 2/0/0 | 0 | 2 |
| active_held_1 | 193.7539 | 253.0820 | 2/0/0 | 1 | 2 |
| active_finished_1 | 194.6719 | 254.0000 | 2/0/0 | 0 | 2 |
| active_held_2 | 194.6719 | 254.0000 | 2/0/0 | 2 | 2 |
| active_finished_2 | 194.9062 | 254.2344 | 2/0/0 | 0 | 2 |
| extra_controller_connections | 194.9062 | 254.2344 | 2/0/0 | 0 | 2 |
| history_browsed | 194.9062 | 254.2969 | 2/0/0 | 0 | 2 |
| slow_consumer_held | 195.1094 | 255.1250 | 2/0/0 | 1 | 2 |
| slow_consumer_settled | 195.1094 | 255.1250 | 2/0/0 | 0 | 2 |
| all_unloaded | 188.5469 | 249.3125 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_0 | 197.9922 | 258.8828 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_1 | 206.3750 | 267.2656 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_2 | 206.4062 | 267.2969 | 0/0/0 | 0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | Settled P95 ms | Settled P99 ms | Burst service turns/s | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 8.7259 | 57.2576 | 66.6174 | 66.6174 | 21.2527 | 23.1250 | 1155.5000 |
| 2 | 30 | 8.1562 | 25.2127 | 69.5783 | 70.7226 | 58.2151 | 18.1250 | 1221.1250 |

Consistent used-page bytes: 52252672; free pages: 6; post-exit main bytes: 52277248; WAL bytes: 0; SHM bytes: 0; cancel-to-settled 8.7370 ms.

### refined/history-512-final

Sessions 2, seed turns per session 512, requests 1075, schema [93], diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active | Attachments |
| --- | --- | --- | --- | --- | --- |
| empty_server | 14.1992 | 45.8281 | 0/0/0 | 0 | 0 |
| metadata_only | 15.9805 | 58.4844 | 0/0/0 | 0 | 0 |
| empty_attached_1 | 87.1758 | 137.1484 | 1/0/0 | 0 | 1 |
| empty_attached_2 | 93.5703 | 143.7617 | 2/0/0 | 0 | 2 |
| subscribed_1 | 93.6641 | 145.1680 | 2/0/0 | 0 | 2 |
| subscribed_2 | 93.7578 | 145.2617 | 2/0/0 | 0 | 2 |
| all_subscribed_empty | 93.7578 | 145.2617 | 2/0/0 | 0 | 2 |
| seeded_history_1 | 99.1602 | 158.5391 | 2/0/0 | 0 | 2 |
| seeded_history_512 | 191.5117 | 250.9531 | 2/0/0 | 0 | 2 |
| active_held_1 | 191.5117 | 250.9531 | 2/0/0 | 1 | 2 |
| active_finished_1 | 192.2695 | 251.7109 | 2/0/0 | 0 | 2 |
| active_held_2 | 192.2695 | 251.7109 | 2/0/0 | 2 | 2 |
| active_finished_2 | 193.4414 | 252.8828 | 2/0/0 | 0 | 2 |
| extra_controller_connections | 193.4453 | 252.8867 | 2/0/0 | 0 | 2 |
| history_browsed | 193.4453 | 252.8867 | 2/0/0 | 0 | 2 |
| slow_consumer_held | 194.1289 | 254.3203 | 2/0/0 | 1 | 2 |
| slow_consumer_settled | 193.3594 | 253.5508 | 2/0/0 | 0 | 2 |
| all_unloaded | 188.6953 | 249.6992 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_0 | 190.5898 | 251.7812 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_1 | 199.4961 | 260.6875 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_2 | 206.9844 | 268.1758 | 0/0/0 | 0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | Settled P95 ms | Settled P99 ms | Burst service turns/s | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 5.8257 | 51.8072 | 55.8964 | 55.8964 | 26.0542 | 18.1250 | 1234.0000 |
| 2 | 30 | 6.7027 | 28.6357 | 65.2993 | 69.9833 | 45.1866 | 19.0625 | 1189.2500 |

Consistent used-page bytes: 52232192; free pages: 6; post-exit main bytes: 52256768; WAL bytes: 0; SHM bytes: 0; cancel-to-settled 8.1368 ms.

### refined/capacity-cpu-final

Sessions 128, seed turns per session 8, requests 1539, schema [93], diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active | Attachments |
| --- | --- | --- | --- | --- | --- |
| empty_server | 14.3516 | 48.0742 | 0/0/0 | 0 | 0 |
| metadata_only | 30.3789 | 74.4766 | 0/0/0 | 0 | 0 |
| empty_attached_1 | 104.5547 | 155.9336 | 1/0/0 | 0 | 1 |
| empty_attached_8 | 119.2969 | 171.0820 | 8/0/0 | 0 | 8 |
| empty_attached_32 | 162.3711 | 214.9062 | 32/0/0 | 0 | 32 |
| empty_attached_64 | 220.4961 | 274.0156 | 64/0/0 | 0 | 64 |
| empty_attached_128 | 337.4414 | 392.9453 | 128/0/0 | 0 | 128 |
| subscribed_1 | 337.4453 | 394.1367 | 128/0/0 | 0 | 128 |
| subscribed_8 | 337.4961 | 394.1875 | 128/0/0 | 0 | 128 |
| subscribed_32 | 337.6289 | 394.3203 | 128/0/0 | 0 | 128 |
| subscribed_64 | 337.6289 | 394.3203 | 128/0/0 | 0 | 128 |
| subscribed_128 | 337.6289 | 394.3203 | 128/0/0 | 0 | 128 |
| all_subscribed_empty | 337.6289 | 394.3203 | 128/0/0 | 0 | 128 |
| seeded_history_1 | 531.4336 | 595.8438 | 128/0/0 | 0 | 128 |
| seeded_history_8 | 928.3164 | 992.7266 | 128/0/0 | 0 | 128 |
| active_held_8 | 939.8516 | 1008.1367 | 128/0/0 | 8 | 128 |
| active_finished_8 | 1137.4062 | 1208.5039 | 128/0/0 | 0 | 128 |
| extra_controller_connections | 1137.8594 | 1209.2695 | 128/0/0 | 0 | 128 |
| history_browsed | 1139.7500 | 1211.2852 | 128/0/0 | 0 | 128 |
| slow_consumer_held | 1139.7734 | 1211.3711 | 128/0/0 | 1 | 128 |
| slow_consumer_settled | 1140.0820 | 1211.6797 | 128/0/0 | 0 | 128 |
| all_unloaded | 1132.0312 | 1200.6914 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_0 | 1132.0430 | 1200.7656 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_1 | 1132.0469 | 1200.7695 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_2 | 1132.0547 | 1200.7773 | 0/0/0 | 0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | Settled P95 ms | Settled P99 ms | Burst service turns/s | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 8 | 504 | 24.5228 | 67.3218 | 108.5262 | 124.3669 | 79.7583 | 13.9453 | 1030.3594 |

Consistent used-page bytes: 155389952; free pages: 4; post-exit main bytes: 155406336; WAL bytes: 0; SHM bytes: 0; cancel-to-settled 8.8700 ms.

### capacity-unload-barrier-smoke

Sessions 8, seed turns per session 1, requests 37, schema [93], diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active | Attachments |
| --- | --- | --- | --- | --- | --- |
| empty_server | 14.2031 | 46.0117 | 0/0/0 | 0 | 0 |
| metadata_only | 16.4375 | 58.2461 | 0/0/0 | 0 | 0 |
| empty_attached_1 | 89.5312 | 139.2461 | 1/0/0 | 0 | 1 |
| empty_attached_8 | 104.9453 | 155.0664 | 8/0/0 | 0 | 8 |
| subscribed_1 | 105.0000 | 156.2461 | 8/0/0 | 0 | 8 |
| subscribed_8 | 105.1484 | 156.3945 | 8/0/0 | 0 | 8 |
| all_subscribed_empty | 105.1484 | 156.3945 | 8/0/0 | 0 | 8 |
| seeded_history_1 | 117.4766 | 176.6602 | 8/0/0 | 0 | 8 |
| active_held_1 | 118.2266 | 177.4102 | 8/0/0 | 1 | 8 |
| active_finished_1 | 118.8281 | 178.0117 | 8/0/0 | 0 | 8 |
| active_held_4 | 120.5312 | 179.7148 | 8/0/0 | 4 | 8 |
| active_finished_4 | 123.0039 | 182.1875 | 8/0/0 | 0 | 8 |
| active_held_8 | 127.1250 | 186.3086 | 8/0/0 | 8 | 8 |
| active_finished_8 | 130.5312 | 189.7148 | 8/0/0 | 0 | 8 |
| extra_controller_connections | 130.7656 | 189.9492 | 8/0/0 | 0 | 8 |
| history_browsed | 132.3438 | 191.5273 | 8/0/0 | 0 | 8 |
| slow_consumer_held | 133.8086 | 193.5547 | 8/0/0 | 1 | 8 |
| slow_consumer_settled | 134.4805 | 194.2266 | 8/0/0 | 0 | 8 |
| all_unloaded | 135.6602 | 195.9688 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_0 | 148.5469 | 208.8555 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_1 | 153.0195 | 213.3281 | 0/0/0 | 0 | 0 |
| reload_unload_cycle_2 | 156.7578 | 217.0664 | 0/0/0 | 0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | Settled P95 ms | Settled P99 ms | Burst service turns/s | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 6.2037 | 13.8258 | 13.8258 | 13.8258 | 72.3286 | 15.0000 | 590.0000 |
| 4 | 4 | 5.6093 | 26.7258 | 69.4644 | 69.4644 | 57.5834 | 10.0000 | 596.0000 |
| 8 | 8 | 5.9374 | 52.1883 | 94.0141 | 94.0141 | 85.0295 | 10.0000 | 639.5000 |

Consistent used-page bytes: 7208960; free pages: 7; post-exit main bytes: 7237632; WAL bytes: 0; SHM bytes: 0; cancel-to-settled 7.2818 ms.

Latency/burst service windows exclude the deliberately held first round. CPU/write windows include all rounds and driver orchestration gaps, although the counters belong only to the native process. Burst service throughput excludes gate setup gaps and is not sustained service capacity. P99 has only 15/60/120 ordinary observations at widths 1/4/8. Raw per-turn values remain in `active-*.json` and the audit.

## Native CPU and bounded heap diagnostics

capacity-cpu-long: 1771 raw / 1771 collapsed samples, period 4000 us.

| Exclusive leaf class | Samples | Percent |
| --- | --- | --- |
| serde_leaf | 391 | 22.078 |
| sqlite_leaf | 168 | 9.486 |
| allocation_leaf | 261 | 14.737 |
| other_leaf | 741 | 41.841 |
| copy_compare_leaf | 210 | 11.858 |

| Overlapping stack presence | Samples |
| --- | --- |
| sqlite3_any | 383 |
| request_source_any | 92 |
| statistics_any | 34 |
| execution_any | 17 |
| app_server_any | 400 |

refined/capacity-cpu-final: 1758 raw / 1758 collapsed samples, period 4000 us.

| Exclusive leaf class | Samples | Percent |
| --- | --- | --- |
| serde_leaf | 358 | 20.364 |
| other_leaf | 762 | 43.345 |
| allocation_leaf | 269 | 15.301 |
| sqlite_leaf | 162 | 9.215 |
| copy_compare_leaf | 207 | 11.775 |

| Overlapping stack presence | Samples |
| --- | --- |
| sqlite3_any | 391 |
| request_source_any | 103 |
| statistics_any | 35 |
| execution_any | 12 |
| app_server_any | 362 |

| Heap file | Global live bytes | Serialized live bytes | Coverage % | Missing live bytes | Global allocated bytes | Serialized allocated bytes |
| --- | --- | --- | --- | --- | --- | --- |
| heap.0001.heap | 88295698 | 76181829 | 86.2803 | 12113869 | 499999482 | 77433029 |
| heap.0002.heap | 101842479 | 81583924 | 80.1080 | 20258555 | 606025710 | 84095010 |
| heap.0003.heap | 110088842 | 87302319 | 79.3017 | 22786523 | 729166280 | 94267691 |
| heap.0004.heap | 77339073 | 68640233 | 88.7523 | 8698840 | 880424282 | 72341448 |
| heap.0005.heap | 76532806 | 68156223 | 89.0549 | 8376583 | 1366457996 | 69833566 |
| heap.0006.heap | 76499579 | 68143231 | 89.0766 | 8356348 | 1366457996 | 69820574 |

Installed gperftools 2.15-3build1 caps heap profile serialization at 1MiB. Global header and serialized buckets differ; pprof attribution covers only serialized rows. Heap uses tcmalloc replacement in a bounded 8-session diagnostic, not ordinary allocator memory/latency or exact 128-session attribution.

## Previous archived phase memory, no new Pi execution

| Observer | History | Engine | Warm sampled anon maximum MiB | Whole sampled anon maximum MiB | Peak phases, four products |
| --- | --- | --- | --- | --- | --- |
| no-live-subscriber | 8 | after | 99.4219 | 121.3398 | after_cleanup, after_cleanup, after_cleanup, after_cleanup |
| no-live-subscriber | 8 | pi | 102.7500 | 111.6836 | after_cleanup, after_cleanup, after_cleanup, after_cleanup |
| no-live-subscriber | 64 | after | 117.4414 | 202.5391 | after_reopen_verification, after_reopen_verification, after_reopen_verification, after_reopen_verification |
| no-live-subscriber | 64 | pi | 125.3145 | 156.1367 | after_cleanup, after_cleanup, after_cleanup, after_cleanup |
| live-subscriber | 8 | after | 104.5000 | 126.3906 | after_cleanup, after_cleanup, after_cleanup, after_cleanup |
| live-subscriber | 8 | pi | 103.9961 | 112.0293 | after_cleanup, after_cleanup, after_cleanup, after_cleanup |
| live-subscriber | 64 | after | 122.6914 | 207.4668 | after_reopen_verification, after_reopen_verification, after_reopen_verification, after_reopen_verification |
| live-subscriber | 64 | pi | 126.0742 | 154.1738 | after_cleanup, after_cleanup, after_cleanup, after_cleanup |

Per-phase medians and all raw counter projections are archived, linked by SHA to the previous immutable archive. Endpoints cannot prove continuous peaks.

