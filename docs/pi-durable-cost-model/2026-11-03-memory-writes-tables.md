# Memory ownership and actual-write values

Initial schema93 compact-page trial and final schema94 rowid-header refinement are separate cohorts. Negative paired changes mean lower. Two balanced pairs per native128-session side and four pairs per single-session cell are descriptive, not confidence or production-capacity claims.

## Initial compact pages, clustered header bodies

| Observer/history | Side | Warm median ms | CPU ms/turn | Process write KiB/turn |
| --- | --- | --- | --- | --- |
| no-live-subscriber/8 | before | 8.15175 | 10.00000 | 816.62500 |
| no-live-subscriber/8 | after | 8.34700 | 10.00000 | 595.06250 |
| no-live-subscriber/64 | before | 8.85325 | 10.46875 | 967.06250 |
| no-live-subscriber/64 | after | 8.43325 | 10.31250 | 725.00000 |
| live-subscriber/8 | before | 8.01550 | 11.09375 | 816.62500 |
| live-subscriber/8 | after | 8.31900 | 11.25000 | 595.00000 |
| live-subscriber/64 | before | 9.20550 | 12.34375 | 967.12500 |
| live-subscriber/64 | after | 9.20000 | 12.18750 | 728.31250 |

| Observer/history | Metric | Pair0 % | Pair1 % | Pair2 % | Pair3 % | Median % |
| --- | --- | --- | --- | --- | --- | --- |
| no-live-subscriber/8 | median_measured_turn_ms | +31.56590 | +8.02815 | -2.13997 | -3.06930 | +2.94409 |
| no-live-subscriber/8 | mean_turn_ms | +13.30625 | +3.51665 | +2.75200 | -3.35297 | +3.13432 |
| no-live-subscriber/8 | turn_cpu_ms_per_turn | +26.47059 | +3.12500 | -3.22581 | -3.12500 | +0.00000 |
| no-live-subscriber/8 | turn_write_bytes_per_turn | -27.12383 | -27.18506 | -27.15029 | -27.10152 | -27.13706 |
| no-live-subscriber/8 | through_cleanup_process_write_bytes | -29.18191 | -29.22913 | -29.20208 | -29.18881 | -29.19544 |
| no-live-subscriber/64 | median_measured_turn_ms | +0.67069 | +4.01404 | -6.89870 | -5.10060 | -2.21495 |
| no-live-subscriber/64 | mean_turn_ms | -0.58496 | +2.56098 | +3.84876 | -77.89090 | +0.98801 |
| no-live-subscriber/64 | turn_cpu_ms_per_turn | +0.00000 | +0.00000 | +0.00000 | -15.38462 | +0.00000 |
| no-live-subscriber/64 | turn_write_bytes_per_turn | -24.91914 | -24.45076 | -27.82159 | -25.12271 | -25.02092 |
| no-live-subscriber/64 | through_cleanup_process_write_bytes | -27.75037 | -27.59880 | -27.57093 | -27.76890 | -27.67458 |
| live-subscriber/8 | median_measured_turn_ms | +22.79666 | -1.06206 | +2.76814 | +2.83714 | +2.80264 |
| live-subscriber/8 | mean_turn_ms | +170.66418 | -4.59773 | -4.00051 | +4.19221 | +0.09585 |
| live-subscriber/8 | turn_cpu_ms_per_turn | +23.07692 | +0.00000 | -2.85714 | +2.85714 | +1.42857 |
| live-subscriber/8 | turn_write_bytes_per_turn | -27.13914 | -27.13499 | -27.13914 | -27.18640 | -27.13914 |
| live-subscriber/8 | through_cleanup_process_write_bytes | -29.15830 | -29.23749 | -29.20552 | -29.25475 | -29.22151 |
| live-subscriber/64 | median_measured_turn_ms | -10.16141 | -0.28562 | +11.49289 | -6.92451 | -3.60506 |
| live-subscriber/64 | mean_turn_ms | -70.00298 | -0.99916 | +8.65031 | -4.99857 | -2.99886 |
| live-subscriber/64 | turn_cpu_ms_per_turn | -22.64151 | -5.12821 | +7.69231 | -7.50000 | -6.31410 |
| live-subscriber/64 | turn_write_bytes_per_turn | -24.78025 | -24.61876 | -24.52489 | -24.78677 | -24.69951 |
| live-subscriber/64 | through_cleanup_process_write_bytes | -27.54529 | -27.56653 | -27.59848 | -27.62761 | -27.58251 |

| History | Side | Page bytes | WAL frames | WAL frame bytes | Commits | Logical column growth | Consistent used-page growth |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 8 | baseline | 4096 | 5626 | 23179120 | 160 | 1011558 | 1470464 |
| 8 | complete | 2048 | 7056 | 14620032 | 160 | 1011582 | 1673216 |
| 64 | baseline | 4096 | 6357 | 26190840 | 160 | 1037490 | 1470464 |
| 64 | complete | 2048 | 7880 | 16327360 | 160 | 1037471 | 1683456 |

## Final compact pages, rowid header bodies

| Observer/history | Side | Warm median ms | CPU ms/turn | Process write KiB/turn |
| --- | --- | --- | --- | --- |
| no-live-subscriber/8 | before | 6.97450 | 8.90625 | 816.50000 |
| no-live-subscriber/8 | after | 6.85925 | 8.75000 | 590.50000 |
| no-live-subscriber/64 | before | 7.53150 | 9.37500 | 966.31250 |
| no-live-subscriber/64 | after | 7.66825 | 9.68750 | 720.50000 |
| live-subscriber/8 | before | 8.52200 | 10.93750 | 816.43750 |
| live-subscriber/8 | after | 8.11850 | 11.09375 | 590.18750 |
| live-subscriber/64 | before | 9.27050 | 12.50000 | 966.18750 |
| live-subscriber/64 | after | 8.86650 | 11.87500 | 722.18750 |

| Observer/history | Metric | Pair0 % | Pair1 % | Pair2 % | Pair3 % | Median % |
| --- | --- | --- | --- | --- | --- | --- |
| no-live-subscriber/8 | median_measured_turn_ms | -0.34750 | -4.05074 | -1.97677 | -1.33219 | -1.65448 |
| no-live-subscriber/8 | mean_turn_ms | +0.65308 | -3.01548 | +2.22795 | -1.95905 | -0.65299 |
| no-live-subscriber/8 | turn_cpu_ms_per_turn | +3.70370 | +0.00000 | -3.57143 | -6.66667 | -1.78571 |
| no-live-subscriber/8 | turn_write_bytes_per_turn | -27.68595 | -27.58726 | -27.67228 | -27.78628 | -27.67912 |
| no-live-subscriber/8 | through_cleanup_process_write_bytes | -29.74504 | -29.58678 | -29.66462 | -29.76387 | -29.70483 |
| no-live-subscriber/64 | median_measured_turn_ms | +4.06930 | +1.42961 | -2.07099 | +1.39394 | +1.41177 |
| no-live-subscriber/64 | mean_turn_ms | +5.19955 | +2.74248 | -1.46351 | +1.20805 | +1.97526 |
| no-live-subscriber/64 | turn_cpu_ms_per_turn | +6.66667 | +3.22581 | -3.33333 | +0.00000 | +1.61290 |
| no-live-subscriber/64 | turn_write_bytes_per_turn | -25.42964 | -25.34938 | -25.35321 | -25.57869 | -25.39143 |
| no-live-subscriber/64 | through_cleanup_process_write_bytes | -28.18219 | -28.32222 | -28.24309 | -28.35167 | -28.28266 |
| live-subscriber/8 | median_measured_turn_ms | -11.64670 | +29.67352 | -0.86105 | -8.41350 | -4.63728 |
| live-subscriber/8 | mean_turn_ms | -14.43594 | +23.16492 | -0.56242 | -9.12978 | -4.84610 |
| live-subscriber/8 | turn_cpu_ms_per_turn | -10.25641 | +5.71429 | +2.85714 | +0.00000 | +1.42857 |
| live-subscriber/8 | turn_write_bytes_per_turn | -27.70291 | -27.74036 | -27.66804 | -27.73186 | -27.71739 |
| live-subscriber/8 | through_cleanup_process_write_bytes | -29.74728 | -29.70484 | -29.64931 | -29.73324 | -29.71904 |
| live-subscriber/64 | median_measured_turn_ms | -3.55900 | -5.40756 | -4.42606 | -3.27770 | -3.99253 |
| live-subscriber/64 | mean_turn_ms | -1.57908 | -6.08324 | -9.88189 | -2.21956 | -4.15140 |
| live-subscriber/64 | turn_cpu_ms_per_turn | -2.56410 | -4.87805 | -9.52381 | +0.00000 | -3.72108 |
| live-subscriber/64 | turn_write_bytes_per_turn | -25.21649 | -25.10357 | -25.33972 | -25.37506 | -25.27811 |
| live-subscriber/64 | through_cleanup_process_write_bytes | -28.28260 | -28.26405 | -28.22960 | -28.31757 | -28.27332 |

| History | Side | Page bytes | WAL frames | WAL frame bytes | Commits | Logical column growth | Consistent used-page growth |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 8 | baseline | 4096 | 5629 | 23191480 | 160 | 1011562 | 1470464 |
| 8 | complete | 2048 | 7000 | 14504000 | 160 | 1011563 | 1564672 |
| 64 | baseline | 4096 | 6380 | 26285600 | 160 | 1037439 | 1474560 |
| 64 | complete | 2048 | 7810 | 16182320 | 160 | 1037425 | 1576960 |

## All accepted native products, ordinary and diagnostic kept distinct

### native-0-before

Sessions 128; seed history 8; requests 1235; schema [93]; diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 14.19531 | 46.12109 | 0/0/0 | 0 |
| metadata_only | 30.34766 | 72.27734 | 0/0/0 | 0 |
| empty_attached_1 | 102.63672 | 152.16016 | 1/0/0 | 0 |
| empty_attached_8 | 119.50000 | 169.42969 | 8/0/0 | 0 |
| empty_attached_32 | 162.56250 | 213.24219 | 32/0/0 | 0 |
| empty_attached_64 | 220.53125 | 272.20703 | 64/0/0 | 0 |
| empty_attached_128 | 336.91797 | 390.58984 | 128/0/0 | 0 |
| subscribed_1 | 337.10156 | 392.08594 | 128/0/0 | 0 |
| subscribed_8 | 337.17969 | 392.16406 | 128/0/0 | 0 |
| subscribed_32 | 337.23047 | 392.21484 | 128/0/0 | 0 |
| subscribed_64 | 337.25781 | 392.24219 | 128/0/0 | 0 |
| subscribed_128 | 337.26172 | 392.24609 | 128/0/0 | 0 |
| all_subscribed_empty | 337.26172 | 392.24609 | 128/0/0 | 0 |
| seeded_history_1 | 530.98047 | 593.66016 | 128/0/0 | 0 |
| seeded_history_8 | 924.38672 | 987.12891 | 128/0/0 | 0 |
| active_held_1 | 924.83203 | 987.57422 | 128/0/0 | 1 |
| active_finished_1 | 933.22266 | 995.96484 | 128/0/0 | 0 |
| active_held_4 | 934.04688 | 996.78906 | 128/0/0 | 4 |
| active_finished_4 | 958.52734 | 1021.26953 | 128/0/0 | 0 |
| active_held_8 | 959.71484 | 1022.45703 | 128/0/0 | 8 |
| active_finished_8 | 1016.66016 | 1079.40234 | 128/0/0 | 0 |
| extra_controller_connections | 1019.04688 | 1082.16406 | 128/0/0 | 0 |
| history_browsed | 1022.72266 | 1085.83984 | 128/0/0 | 0 |
| slow_consumer_held | 1023.62891 | 1086.80859 | 128/0/0 | 1 |
| slow_consumer_settled | 1023.63672 | 1086.81641 | 128/0/0 | 0 |
| all_unloaded | 1021.11719 | 1081.10938 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 1021.14453 | 1081.32422 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 1021.15234 | 1081.33203 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 1021.16797 | 1081.34766 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 6.83803 | 49.21494 | 61.38823 | 61.38823 | 19.37500 | 905.75000 |
| 4 | 60 | 19.64890 | 38.88689 | 78.95697 | 84.60526 | 14.68750 | 937.50000 |
| 8 | 120 | 27.81761 | 64.04597 | 111.66723 | 120.71975 | 13.75000 | 971.50000 |

Used-page bytes 141025280; free pages 4; post-exit main bytes 141041664; WAL bytes 0; SHM bytes 0; cancel-to-settled 12.35169 ms.

### native-0-after

Sessions 128; seed history 8; requests 1235; schema [93]; diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 14.20703 | 45.71484 | 0/0/0 | 0 |
| metadata_only | 29.99609 | 71.94141 | 0/0/0 | 0 |
| empty_attached_1 | 103.66406 | 153.64062 | 1/0/0 | 0 |
| empty_attached_8 | 119.25781 | 169.64062 | 8/0/0 | 0 |
| empty_attached_32 | 160.56250 | 211.69531 | 32/0/0 | 0 |
| empty_attached_64 | 216.67188 | 268.80469 | 64/0/0 | 0 |
| empty_attached_128 | 328.28906 | 382.42188 | 128/0/0 | 0 |
| subscribed_1 | 328.39062 | 383.83594 | 128/0/0 | 0 |
| subscribed_8 | 328.47656 | 383.92188 | 128/0/0 | 0 |
| subscribed_32 | 328.53906 | 383.98438 | 128/0/0 | 0 |
| subscribed_64 | 328.54297 | 383.98828 | 128/0/0 | 0 |
| subscribed_128 | 328.58203 | 384.02734 | 128/0/0 | 0 |
| all_subscribed_empty | 328.58203 | 384.02734 | 128/0/0 | 0 |
| seeded_history_1 | 493.34766 | 556.23047 | 128/0/0 | 0 |
| seeded_history_8 | 787.84375 | 850.72656 | 128/0/0 | 0 |
| active_held_1 | 788.93359 | 851.81641 | 128/0/0 | 1 |
| active_finished_1 | 794.41797 | 857.30078 | 128/0/0 | 0 |
| active_held_4 | 795.26953 | 858.15234 | 128/0/0 | 4 |
| active_finished_4 | 812.91406 | 875.79688 | 128/0/0 | 0 |
| active_held_8 | 814.56250 | 877.44531 | 128/0/0 | 8 |
| active_finished_8 | 854.98828 | 917.87109 | 128/0/0 | 0 |
| extra_controller_connections | 855.25781 | 918.64062 | 128/0/0 | 0 |
| history_browsed | 857.23438 | 920.61719 | 128/0/0 | 0 |
| slow_consumer_held | 858.25781 | 921.70312 | 128/0/0 | 1 |
| slow_consumer_settled | 858.25781 | 921.70312 | 128/0/0 | 0 |
| all_unloaded | 857.90234 | 918.09766 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 857.91406 | 918.17188 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 857.92188 | 918.17969 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 857.92578 | 918.18359 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 6.46125 | 21.26290 | 57.74106 | 57.74106 | 18.12500 | 687.50000 |
| 4 | 60 | 15.39657 | 43.59723 | 77.37959 | 89.36244 | 14.37500 | 713.68750 |
| 8 | 120 | 23.69045 | 74.14056 | 116.24753 | 130.45907 | 14.14062 | 742.46875 |

Used-page bytes 113068032; free pages 9; post-exit main bytes 113086464; WAL bytes 0; SHM bytes 0; cancel-to-settled 8.16094 ms.

### native-1-after

Sessions 128; seed history 8; requests 1235; schema [93]; diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 14.20703 | 45.61719 | 0/0/0 | 0 |
| metadata_only | 29.96875 | 71.69141 | 0/0/0 | 0 |
| empty_attached_1 | 102.65234 | 152.21875 | 1/0/0 | 0 |
| empty_attached_8 | 118.51172 | 168.42188 | 8/0/0 | 0 |
| empty_attached_32 | 161.42578 | 212.08594 | 32/0/0 | 0 |
| empty_attached_64 | 215.58203 | 267.24219 | 64/0/0 | 0 |
| empty_attached_128 | 329.37109 | 383.03125 | 128/0/0 | 0 |
| subscribed_1 | 329.44141 | 384.16406 | 128/0/0 | 0 |
| subscribed_8 | 329.51172 | 384.23438 | 128/0/0 | 0 |
| subscribed_32 | 329.51172 | 384.23438 | 128/0/0 | 0 |
| subscribed_64 | 329.51172 | 384.23438 | 128/0/0 | 0 |
| subscribed_128 | 329.51562 | 384.23828 | 128/0/0 | 0 |
| all_subscribed_empty | 329.51562 | 384.23828 | 128/0/0 | 0 |
| seeded_history_1 | 489.55859 | 552.15625 | 128/0/0 | 0 |
| seeded_history_8 | 786.36719 | 848.96484 | 128/0/0 | 0 |
| active_held_1 | 786.37109 | 848.96875 | 128/0/0 | 1 |
| active_finished_1 | 790.54297 | 853.14062 | 128/0/0 | 0 |
| active_held_4 | 791.83203 | 854.42969 | 128/0/0 | 4 |
| active_finished_4 | 811.64453 | 874.24219 | 128/0/0 | 0 |
| active_held_8 | 812.16797 | 874.76562 | 128/0/0 | 8 |
| active_finished_8 | 853.22656 | 915.82422 | 128/0/0 | 0 |
| extra_controller_connections | 853.78125 | 916.75391 | 128/0/0 | 0 |
| history_browsed | 855.25781 | 918.35547 | 128/0/0 | 0 |
| slow_consumer_held | 856.30859 | 919.53125 | 128/0/0 | 1 |
| slow_consumer_settled | 856.30859 | 919.53125 | 128/0/0 | 0 |
| all_unloaded | 856.70703 | 916.80469 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 856.72656 | 917.01172 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 856.72656 | 917.01172 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 856.73438 | 917.01953 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 6.62066 | 21.34706 | 59.97721 | 59.97721 | 16.25000 | 687.00000 |
| 4 | 60 | 16.60753 | 49.30217 | 76.43001 | 82.44677 | 14.06250 | 713.06250 |
| 8 | 120 | 24.63272 | 67.26792 | 110.11706 | 118.22736 | 13.75000 | 739.46875 |

Used-page bytes 113074176; free pages 8; post-exit main bytes 113090560; WAL bytes 0; SHM bytes 0; cancel-to-settled 48.17374 ms.

### native-1-before

Sessions 128; seed history 8; requests 1235; schema [93]; diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 14.19922 | 46.26953 | 0/0/0 | 0 |
| metadata_only | 31.67969 | 74.18750 | 0/0/0 | 0 |
| empty_attached_1 | 105.06250 | 155.53906 | 1/0/0 | 0 |
| empty_attached_8 | 119.29297 | 170.17578 | 8/0/0 | 0 |
| empty_attached_32 | 162.40625 | 214.03906 | 32/0/0 | 0 |
| empty_attached_64 | 220.85938 | 273.49219 | 64/0/0 | 0 |
| empty_attached_128 | 337.53516 | 392.16797 | 128/0/0 | 0 |
| subscribed_1 | 337.60547 | 393.42578 | 128/0/0 | 0 |
| subscribed_8 | 337.60938 | 393.42969 | 128/0/0 | 0 |
| subscribed_32 | 337.67578 | 393.49609 | 128/0/0 | 0 |
| subscribed_64 | 337.67578 | 393.49609 | 128/0/0 | 0 |
| subscribed_128 | 337.67578 | 393.49609 | 128/0/0 | 0 |
| all_subscribed_empty | 337.67578 | 393.49609 | 128/0/0 | 0 |
| seeded_history_1 | 529.99609 | 593.62891 | 128/0/0 | 0 |
| seeded_history_8 | 924.64844 | 988.28125 | 128/0/0 | 0 |
| active_held_1 | 926.54297 | 990.17578 | 128/0/0 | 1 |
| active_finished_1 | 935.05859 | 998.69141 | 128/0/0 | 0 |
| active_held_4 | 937.45703 | 1001.08984 | 128/0/0 | 4 |
| active_finished_4 | 961.29297 | 1024.92578 | 128/0/0 | 0 |
| active_held_8 | 964.76953 | 1028.40234 | 128/0/0 | 8 |
| active_finished_8 | 1012.41406 | 1076.04688 | 128/0/0 | 0 |
| extra_controller_connections | 1013.98828 | 1078.24609 | 128/0/0 | 0 |
| history_browsed | 1017.35547 | 1081.73828 | 128/0/0 | 0 |
| slow_consumer_held | 1019.37109 | 1083.87891 | 128/0/0 | 1 |
| slow_consumer_settled | 1019.59766 | 1084.10547 | 128/0/0 | 0 |
| all_unloaded | 1015.99609 | 1077.31641 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 1015.99609 | 1077.44141 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 1016.01172 | 1077.45703 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 1016.01172 | 1077.45703 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 6.04660 | 51.23394 | 62.19497 | 62.19497 | 16.25000 | 906.75000 |
| 4 | 60 | 17.33351 | 47.06501 | 80.14469 | 88.50960 | 14.37500 | 937.00000 |
| 8 | 120 | 23.24875 | 67.03989 | 110.86106 | 115.35149 | 13.67188 | 971.00000 |

Used-page bytes 141029376; free pages 4; post-exit main bytes 141045760; WAL bytes 0; SHM bytes 0; cancel-to-settled 11.75776 ms.

### history-512-before

Sessions 2; seed history 512; requests 1075; schema [93]; diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 14.19922 | 46.26562 | 0/0/0 | 0 |
| metadata_only | 15.99219 | 57.93359 | 0/0/0 | 0 |
| empty_attached_1 | 89.17969 | 138.77734 | 1/0/0 | 0 |
| empty_attached_2 | 95.01953 | 144.83594 | 2/0/0 | 0 |
| subscribed_1 | 95.30469 | 146.37109 | 2/0/0 | 0 |
| subscribed_2 | 95.30469 | 146.37109 | 2/0/0 | 0 |
| all_subscribed_empty | 95.30469 | 146.37109 | 2/0/0 | 0 |
| seeded_history_1 | 99.23828 | 158.11719 | 2/0/0 | 0 |
| seeded_history_512 | 189.74219 | 248.62109 | 2/0/0 | 0 |
| active_held_1 | 190.00781 | 248.88672 | 2/0/0 | 1 |
| active_finished_1 | 190.16016 | 249.03906 | 2/0/0 | 0 |
| active_held_2 | 190.66016 | 249.53906 | 2/0/0 | 2 |
| active_finished_2 | 192.88672 | 251.76562 | 2/0/0 | 0 |
| extra_controller_connections | 192.88672 | 251.76562 | 2/0/0 | 0 |
| history_browsed | 192.88672 | 251.76562 | 2/0/0 | 0 |
| slow_consumer_held | 193.64844 | 253.02734 | 2/0/0 | 1 |
| slow_consumer_settled | 193.64844 | 253.02734 | 2/0/0 | 0 |
| all_unloaded | 189.56641 | 249.75781 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 196.27734 | 256.71875 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 199.60547 | 260.04688 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 202.75391 | 263.19531 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 11.65361 | 57.45485 | 65.87002 | 65.87002 | 25.62500 | 1228.25000 |
| 2 | 30 | 7.06477 | 29.05805 | 40.33011 | 65.00247 | 18.75000 | 1184.75000 |

Used-page bytes 52228096; free pages 6; post-exit main bytes 52252672; WAL bytes 0; SHM bytes 0; cancel-to-settled 8.48481 ms.

### history-512-after

Sessions 2; seed history 512; requests 1075; schema [93]; diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 14.20312 | 45.87109 | 0/0/0 | 0 |
| metadata_only | 15.69531 | 57.86328 | 0/0/0 | 0 |
| empty_attached_1 | 86.83203 | 137.03125 | 1/0/0 | 0 |
| empty_attached_2 | 93.21094 | 143.62891 | 2/0/0 | 0 |
| subscribed_1 | 93.33984 | 144.63281 | 2/0/0 | 0 |
| subscribed_2 | 93.51172 | 144.80469 | 2/0/0 | 0 |
| all_subscribed_empty | 93.51172 | 144.80469 | 2/0/0 | 0 |
| seeded_history_1 | 97.40234 | 156.38281 | 2/0/0 | 0 |
| seeded_history_512 | 172.89453 | 231.87500 | 2/0/0 | 0 |
| active_held_1 | 173.73047 | 232.71094 | 2/0/0 | 1 |
| active_finished_1 | 173.38672 | 232.36719 | 2/0/0 | 0 |
| active_held_2 | 174.06641 | 233.04688 | 2/0/0 | 2 |
| active_finished_2 | 175.98047 | 234.96094 | 2/0/0 | 0 |
| extra_controller_connections | 175.98047 | 234.96094 | 2/0/0 | 0 |
| history_browsed | 175.98047 | 234.96094 | 2/0/0 | 0 |
| slow_consumer_held | 175.98438 | 235.40234 | 2/0/0 | 1 |
| slow_consumer_settled | 175.98438 | 235.40234 | 2/0/0 | 0 |
| all_unloaded | 172.34766 | 232.57812 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 178.81250 | 239.10547 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 182.97266 | 243.26562 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 186.77734 | 247.07031 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 12.28225 | 56.04877 | 72.96833 | 72.96833 | 26.87500 | 889.00000 |
| 2 | 30 | 8.40740 | 31.32542 | 70.71779 | 72.05745 | 20.62500 | 876.25000 |

Used-page bytes 57765888; free pages 11; post-exit main bytes 57788416; WAL bytes 0; SHM bytes 0; cancel-to-settled 8.53300 ms.

### sampling-128-before

Sessions 128; seed history 8; requests 1053; schema [93]; diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 19.03906 | 56.92578 | 0/0/0 | 0 |
| metadata_only | 37.60938 | 88.62109 | 0/0/0 | 0 |
| empty_attached_1 | 110.21094 | 169.37891 | 1/0/0 | 0 |
| empty_attached_8 | 126.33594 | 186.03516 | 8/0/0 | 0 |
| empty_attached_32 | 174.30078 | 234.87500 | 32/0/0 | 0 |
| empty_attached_64 | 235.73438 | 297.37109 | 64/0/0 | 0 |
| empty_attached_128 | 357.02734 | 420.66016 | 128/0/0 | 0 |
| subscribed_1 | 364.95312 | 429.85156 | 128/0/0 | 0 |
| subscribed_8 | 365.16797 | 430.06641 | 128/0/0 | 0 |
| subscribed_32 | 365.24609 | 430.14453 | 128/0/0 | 0 |
| subscribed_64 | 365.26562 | 430.22656 | 128/0/0 | 0 |
| subscribed_128 | 365.30078 | 430.38672 | 128/0/0 | 0 |
| all_subscribed_empty | 365.30078 | 430.38672 | 128/0/0 | 0 |
| seeded_history_1 | 583.69141 | 656.59375 | 128/0/0 | 0 |
| seeded_history_8 | 995.78906 | 1068.69141 | 128/0/0 | 0 |
| active_held_1 | 1013.24219 | 1086.14453 | 128/0/0 | 1 |
| active_finished_1 | 1013.31641 | 1086.21875 | 128/0/0 | 0 |
| active_held_4 | 1013.43359 | 1086.33594 | 128/0/0 | 4 |
| active_finished_4 | 1013.56250 | 1086.46484 | 128/0/0 | 0 |
| active_held_8 | 1013.64062 | 1086.54297 | 128/0/0 | 8 |
| active_finished_8 | 1013.70703 | 1086.60938 | 128/0/0 | 0 |
| extra_controller_connections | 1015.85547 | 1089.13281 | 128/0/0 | 0 |
| history_browsed | 1022.16016 | 1095.43750 | 128/0/0 | 0 |
| slow_consumer_held | 1022.77734 | 1096.11719 | 128/0/0 | 1 |
| slow_consumer_settled | 1022.78516 | 1096.12500 | 128/0/0 | 0 |
| all_unloaded | 1022.94922 | 1093.03906 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 1023.07031 | 1093.34766 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 1023.16016 | 1093.43750 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 1023.15625 | 1093.43359 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 8.01947 | 56.26683 | 56.26683 | 56.26683 | 15.00000 | 778.00000 |
| 4 | 4 | 17.29694 | 61.92725 | 91.81127 | 91.81127 | 18.75000 | 784.00000 |
| 8 | 8 | 22.68777 | 90.29190 | 114.78655 | 114.78655 | 16.87500 | 825.75000 |

Used-page bytes 132399104; free pages 4; post-exit main bytes 132415488; WAL bytes 0; SHM bytes 0; cancel-to-settled 10.28724 ms.

### sampling-128-after

Sessions 128; seed history 8; requests 1053; schema [93]; diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 19.14844 | 56.60156 | 0/0/0 | 0 |
| metadata_only | 36.50781 | 86.33594 | 0/0/0 | 0 |
| empty_attached_1 | 111.13281 | 169.67969 | 1/0/0 | 0 |
| empty_attached_8 | 124.67969 | 183.69531 | 8/0/0 | 0 |
| empty_attached_32 | 170.84766 | 230.67578 | 32/0/0 | 0 |
| empty_attached_64 | 230.28906 | 291.24219 | 64/0/0 | 0 |
| empty_attached_128 | 346.96094 | 409.97656 | 128/0/0 | 0 |
| subscribed_1 | 354.73438 | 418.74609 | 128/0/0 | 0 |
| subscribed_8 | 354.83984 | 418.85156 | 128/0/0 | 0 |
| subscribed_32 | 354.92969 | 418.94141 | 128/0/0 | 0 |
| subscribed_64 | 352.08203 | 416.15625 | 128/0/0 | 0 |
| subscribed_128 | 354.76562 | 418.83984 | 128/0/0 | 0 |
| all_subscribed_empty | 354.76562 | 418.83984 | 128/0/0 | 0 |
| seeded_history_1 | 537.69141 | 610.07812 | 128/0/0 | 0 |
| seeded_history_8 | 849.52734 | 921.91406 | 128/0/0 | 0 |
| active_held_1 | 864.87109 | 937.32031 | 128/0/0 | 1 |
| active_finished_1 | 864.90234 | 937.35156 | 128/0/0 | 0 |
| active_held_4 | 864.98828 | 937.43750 | 128/0/0 | 4 |
| active_finished_4 | 865.03516 | 937.48438 | 128/0/0 | 0 |
| active_held_8 | 865.13281 | 937.58203 | 128/0/0 | 8 |
| active_finished_8 | 865.16406 | 937.61328 | 128/0/0 | 0 |
| extra_controller_connections | 865.19922 | 938.02344 | 128/0/0 | 0 |
| history_browsed | 871.60156 | 944.42578 | 128/0/0 | 0 |
| slow_consumer_held | 872.19922 | 945.08594 | 128/0/0 | 1 |
| slow_consumer_settled | 872.44531 | 945.33203 | 128/0/0 | 0 |
| all_unloaded | 872.75391 | 942.51562 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 873.41016 | 943.23438 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 873.47266 | 943.29688 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 873.55859 | 943.38281 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 7.22717 | 22.68744 | 22.68744 | 22.68744 | 15.00000 | 524.00000 |
| 4 | 4 | 17.37138 | 60.02477 | 86.95995 | 86.95995 | 18.75000 | 604.00000 |
| 8 | 8 | 47.54927 | 95.57227 | 106.07039 | 106.07039 | 18.12500 | 645.75000 |

Used-page bytes 103577600; free pages 9; post-exit main bytes 103596032; WAL bytes 0; SHM bytes 0; cancel-to-settled 48.83604 ms.

### sql-memory-before

Sessions 8; seed history 8; requests 93; schema [93]; diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 17.54688 | 83.99609 | 0/0/0 | 0 |
| metadata_only | 21.05078 | 99.37500 | 0/0/0 | 0 |
| empty_attached_1 | 92.15625 | 181.76172 | 1/0/0 | 0 |
| empty_attached_8 | 108.26172 | 198.64844 | 8/0/0 | 0 |
| subscribed_1 | 108.52734 | 199.97656 | 8/0/0 | 0 |
| subscribed_8 | 108.66406 | 200.11328 | 8/0/0 | 0 |
| all_subscribed_empty | 108.66406 | 200.11328 | 8/0/0 | 0 |
| seeded_history_1 | 124.24609 | 229.38281 | 8/0/0 | 0 |
| seeded_history_8 | 152.15234 | 257.47656 | 8/0/0 | 0 |
| active_held_1 | 152.69922 | 258.02344 | 8/0/0 | 1 |
| active_finished_1 | 153.09375 | 258.41797 | 8/0/0 | 0 |
| active_held_4 | 154.75391 | 260.07812 | 8/0/0 | 4 |
| active_finished_4 | 156.60547 | 261.92969 | 8/0/0 | 0 |
| active_held_8 | 159.78125 | 265.10547 | 8/0/0 | 8 |
| active_finished_8 | 164.67578 | 270.00000 | 8/0/0 | 0 |
| extra_controller_connections | 165.01953 | 270.34375 | 8/0/0 | 0 |
| history_browsed | 166.85156 | 272.17578 | 8/0/0 | 0 |
| slow_consumer_held | 168.02344 | 274.84766 | 8/0/0 | 1 |
| slow_consumer_settled | 168.77344 | 275.59766 | 8/0/0 | 0 |
| all_unloaded | 169.80859 | 277.57031 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 172.31641 | 280.14062 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 178.36328 | 286.18750 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 180.80859 | 288.63281 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 21.05802 | 41.35218 | 41.35218 | 41.35218 | 65.00000 | 638.00000 |
| 4 | 4 | 18.33783 | 94.55424 | 110.17387 | 110.17387 | 48.75000 | 641.00000 |
| 8 | 8 | 66.67813 | 233.47487 | 286.03497 | 286.03497 | 49.37500 | 685.50000 |

Used-page bytes 9523200; free pages 6; post-exit main bytes 9547776; WAL bytes 0; SHM bytes 0; cancel-to-settled 17.04535 ms.

### sql-replay-128-before

Sessions 128; seed history 8; requests 1053; schema [93]; diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 21.80469 | 95.74219 | 0/0/0 | 0 |
| metadata_only | 42.68750 | 133.25000 | 0/0/0 | 0 |
| empty_attached_1 | 111.25781 | 212.53906 | 1/0/0 | 0 |
| empty_attached_8 | 127.14453 | 229.45703 | 8/0/0 | 0 |
| empty_attached_32 | 171.08203 | 274.45703 | 32/0/0 | 0 |
| empty_attached_64 | 228.11328 | 332.98828 | 64/0/0 | 0 |
| empty_attached_128 | 341.76172 | 449.06641 | 128/0/0 | 0 |
| subscribed_1 | 342.14453 | 450.57422 | 128/0/0 | 0 |
| subscribed_8 | 342.47266 | 450.90234 | 128/0/0 | 0 |
| subscribed_32 | 344.41406 | 452.84375 | 128/0/0 | 0 |
| subscribed_64 | 346.33594 | 454.76562 | 128/0/0 | 0 |
| subscribed_128 | 348.27344 | 456.70312 | 128/0/0 | 0 |
| all_subscribed_empty | 348.27344 | 456.70312 | 128/0/0 | 0 |
| seeded_history_1 | 554.11719 | 677.49219 | 128/0/0 | 0 |
| seeded_history_8 | 943.31250 | 1066.87500 | 128/0/0 | 0 |
| active_held_1 | 944.63672 | 1068.19922 | 128/0/0 | 1 |
| active_finished_1 | 944.98047 | 1068.54297 | 128/0/0 | 0 |
| active_held_4 | 947.19141 | 1070.75391 | 128/0/0 | 4 |
| active_finished_4 | 949.10547 | 1072.66797 | 128/0/0 | 0 |
| active_held_8 | 953.29688 | 1076.85938 | 128/0/0 | 8 |
| active_finished_8 | 955.89844 | 1079.46094 | 128/0/0 | 0 |
| extra_controller_connections | 960.69531 | 1085.69531 | 128/0/0 | 0 |
| history_browsed | 965.65234 | 1090.65234 | 128/0/0 | 0 |
| slow_consumer_held | 966.38672 | 1091.38672 | 128/0/0 | 1 |
| slow_consumer_settled | 966.41016 | 1091.41016 | 128/0/0 | 0 |
| all_unloaded | 967.46094 | 1089.39844 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 967.45703 | 1089.39453 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 967.42969 | 1089.36719 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 967.43359 | 1089.37109 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 25.79024 | 91.52324 | 91.52324 | 91.52324 | 70.00000 | 802.00000 |
| 4 | 4 | 41.18176 | 122.05090 | 144.87368 | 144.87368 | 65.00000 | 790.00000 |
| 8 | 8 | 48.19790 | 232.64431 | 289.25907 | 289.25907 | 62.50000 | 831.25000 |

Used-page bytes 132399104; free pages 4; post-exit main bytes 132415488; WAL bytes 0; SHM bytes 0; cancel-to-settled 58.59213 ms.

### sql-replay-serial-before

Sessions 128; seed history 8; requests 1053; schema [93]; diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 22.28906 | 96.22656 | 0/0/0 | 0 |
| metadata_only | 43.48828 | 133.67578 | 0/0/0 | 0 |
| empty_attached_1 | 112.88672 | 213.98047 | 1/0/0 | 0 |
| empty_attached_8 | 127.66406 | 229.78906 | 8/0/0 | 0 |
| empty_attached_32 | 171.58984 | 274.96484 | 32/0/0 | 0 |
| empty_attached_64 | 228.21094 | 333.14844 | 64/0/0 | 0 |
| empty_attached_128 | 341.50781 | 448.75000 | 128/0/0 | 0 |
| subscribed_1 | 341.69531 | 450.00000 | 128/0/0 | 0 |
| subscribed_8 | 342.03125 | 450.33594 | 128/0/0 | 0 |
| subscribed_32 | 344.18750 | 452.49219 | 128/0/0 | 0 |
| subscribed_64 | 346.06641 | 454.43359 | 128/0/0 | 0 |
| subscribed_128 | 348.02734 | 456.39453 | 128/0/0 | 0 |
| all_subscribed_empty | 348.02734 | 456.39453 | 128/0/0 | 0 |
| seeded_history_1 | 553.80078 | 677.17578 | 128/0/0 | 0 |
| seeded_history_8 | 945.82812 | 1069.51562 | 128/0/0 | 0 |
| active_held_1 | 947.02344 | 1070.71094 | 128/0/0 | 1 |
| active_finished_1 | 947.56250 | 1071.25000 | 128/0/0 | 0 |
| active_held_4 | 948.60547 | 1072.29297 | 128/0/0 | 4 |
| active_finished_4 | 950.53906 | 1074.22656 | 128/0/0 | 0 |
| active_held_8 | 954.76562 | 1078.45312 | 128/0/0 | 8 |
| active_finished_8 | 958.07422 | 1081.76172 | 128/0/0 | 0 |
| extra_controller_connections | 962.47266 | 1087.59766 | 128/0/0 | 0 |
| history_browsed | 966.32422 | 1091.44922 | 128/0/0 | 0 |
| slow_consumer_held | 967.10547 | 1092.23047 | 128/0/0 | 1 |
| slow_consumer_settled | 967.11719 | 1092.24219 | 128/0/0 | 0 |
| all_unloaded | 960.62891 | 1082.81250 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 960.84375 | 1083.02734 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 960.85156 | 1083.03516 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 960.86719 | 1083.05078 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 17.50019 | 75.72453 | 75.72453 | 75.72453 | 90.00000 | 1018.00000 |
| 4 | 4 | 162.12459 | 241.50410 | 414.19945 | 414.19945 | 65.00000 | 790.00000 |
| 8 | 8 | 198.53613 | 370.48195 | 891.67514 | 891.67514 | 63.75000 | 831.75000 |

Used-page bytes 132399104; free pages 4; post-exit main bytes 132415488; WAL bytes 0; SHM bytes 0; cancel-to-settled 22.14874 ms.

### sql-fields-eight-before

Sessions 8; seed history 8; requests 93; schema [93]; diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 21.80469 | 96.89844 | 0/0/0 | 0 |
| metadata_only | 25.72266 | 113.44141 | 0/0/0 | 0 |
| empty_attached_1 | 94.78516 | 194.66016 | 1/0/0 | 0 |
| empty_attached_8 | 110.45312 | 211.92188 | 8/0/0 | 0 |
| subscribed_1 | 110.85156 | 213.44531 | 8/0/0 | 0 |
| subscribed_8 | 111.48438 | 214.20312 | 8/0/0 | 0 |
| all_subscribed_empty | 111.48438 | 214.20312 | 8/0/0 | 0 |
| seeded_history_1 | 127.17969 | 244.21094 | 8/0/0 | 0 |
| seeded_history_8 | 156.41016 | 274.12891 | 8/0/0 | 0 |
| active_held_1 | 157.02344 | 275.11719 | 8/0/0 | 1 |
| active_finished_1 | 157.50391 | 275.59766 | 8/0/0 | 0 |
| active_held_4 | 159.05859 | 277.15234 | 8/0/0 | 4 |
| active_finished_4 | 160.74219 | 278.83594 | 8/0/0 | 0 |
| active_held_8 | 165.10156 | 283.32031 | 8/0/0 | 8 |
| active_finished_8 | 168.53906 | 286.75781 | 8/0/0 | 0 |
| extra_controller_connections | 168.94531 | 287.16406 | 8/0/0 | 0 |
| history_browsed | 171.41797 | 289.69922 | 8/0/0 | 0 |
| slow_consumer_held | 173.22656 | 292.94531 | 8/0/0 | 1 |
| slow_consumer_settled | 173.75000 | 293.46875 | 8/0/0 | 0 |
| all_unloaded | 171.02734 | 291.74219 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 171.24219 | 292.01953 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 171.26562 | 292.04297 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 171.27344 | 292.05078 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 21.86938 | 72.86525 | 72.86525 | 72.86525 | 50.00000 | 638.00000 |
| 4 | 4 | 11.71878 | 109.96451 | 161.87375 | 161.87375 | 50.00000 | 642.00000 |
| 8 | 8 | 50.50581 | 206.97543 | 244.17716 | 244.17716 | 49.37500 | 685.50000 |

Used-page bytes 9523200; free pages 6; post-exit main bytes 9547776; WAL bytes 0; SHM bytes 0; cancel-to-settled 21.54358 ms.

### heap-eight-history

Sessions 8; seed history 8; requests 93; schema [93]; diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 69.41797 | 109.54688 | 0/0/0 | 0 |
| metadata_only | 75.45703 | 126.58594 | 0/0/0 | 0 |
| empty_attached_1 | 174.17969 | 233.83594 | 1/0/0 | 0 |
| empty_attached_8 | 208.48047 | 268.78516 | 8/0/0 | 0 |
| subscribed_1 | 211.69531 | 273.12500 | 8/0/0 | 0 |
| subscribed_8 | 212.67578 | 274.10547 | 8/0/0 | 0 |
| all_subscribed_empty | 212.67578 | 274.10547 | 8/0/0 | 0 |
| seeded_history_1 | 261.13672 | 330.13672 | 8/0/0 | 0 |
| seeded_history_8 | 302.69922 | 371.69922 | 8/0/0 | 0 |
| active_held_1 | 305.42188 | 374.42188 | 8/0/0 | 1 |
| active_finished_1 | 306.24609 | 375.24609 | 8/0/0 | 0 |
| active_held_4 | 307.91016 | 376.91016 | 8/0/0 | 4 |
| active_finished_4 | 310.36328 | 379.36328 | 8/0/0 | 0 |
| active_held_8 | 315.89453 | 384.89453 | 8/0/0 | 8 |
| active_finished_8 | 323.53125 | 392.53125 | 8/0/0 | 0 |
| extra_controller_connections | 323.78516 | 392.78516 | 8/0/0 | 0 |
| history_browsed | 327.89844 | 396.89844 | 8/0/0 | 0 |
| slow_consumer_held | 330.17969 | 399.67969 | 8/0/0 | 1 |
| slow_consumer_settled | 332.03906 | 401.53906 | 8/0/0 | 0 |
| all_unloaded | 338.50391 | 408.50391 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 342.95312 | 413.14062 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 342.85547 | 413.04297 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 342.93359 | 413.12109 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 46.02112 | 566.18677 | 566.18677 | 566.18677 | 775.00000 | 1142.00000 |
| 4 | 4 | 109.14741 | 2239.43358 | 2434.34679 | 2434.34679 | 958.75000 | 639.00000 |
| 8 | 8 | 449.87586 | 4182.45317 | 5159.43248 | 5159.43248 | 1011.25000 | 759.50000 |

Used-page bytes 9531392; free pages 6; post-exit main bytes 9555968; WAL bytes 0; SHM bytes 0; cancel-to-settled 613.72811 ms.

### sql-fields-eight-after

Sessions 8; seed history 8; requests 93; schema [93]; diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 21.81250 | 96.08984 | 0/0/0 | 0 |
| metadata_only | 25.68359 | 114.39844 | 0/0/0 | 0 |
| empty_attached_1 | 95.84766 | 195.65625 | 1/0/0 | 0 |
| empty_attached_8 | 110.27734 | 211.61719 | 8/0/0 | 0 |
| subscribed_1 | 110.68359 | 212.89844 | 8/0/0 | 0 |
| subscribed_8 | 111.33203 | 213.54688 | 8/0/0 | 0 |
| all_subscribed_empty | 111.33203 | 213.54688 | 8/0/0 | 0 |
| seeded_history_1 | 124.85547 | 241.57031 | 8/0/0 | 0 |
| seeded_history_8 | 148.95703 | 266.67188 | 8/0/0 | 0 |
| active_held_1 | 149.36328 | 267.07812 | 8/0/0 | 1 |
| active_finished_1 | 149.80078 | 267.51562 | 8/0/0 | 0 |
| active_held_4 | 150.83594 | 268.67578 | 8/0/0 | 4 |
| active_finished_4 | 152.82812 | 270.91797 | 8/0/0 | 0 |
| active_held_8 | 155.54688 | 273.63672 | 8/0/0 | 8 |
| active_finished_8 | 158.18359 | 276.27344 | 8/0/0 | 0 |
| extra_controller_connections | 158.56250 | 276.65234 | 8/0/0 | 0 |
| history_browsed | 160.44141 | 278.53125 | 8/0/0 | 0 |
| slow_consumer_held | 161.63281 | 281.22266 | 8/0/0 | 1 |
| slow_consumer_settled | 162.62891 | 282.21875 | 8/0/0 | 0 |
| all_unloaded | 158.98828 | 279.26172 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 159.23828 | 279.57422 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 159.32031 | 279.65625 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 159.36328 | 279.69922 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 14.70885 | 37.28495 | 37.28495 | 37.28495 | 55.00000 | 384.00000 |
| 4 | 4 | 51.82643 | 98.68667 | 124.20069 | 124.20069 | 47.50000 | 460.50000 |
| 8 | 8 | 32.07069 | 191.95964 | 219.35076 | 219.35076 | 48.12500 | 504.50000 |

Used-page bytes 7983104; free pages 11; post-exit main bytes 8005632; WAL bytes 0; SHM bytes 0; cancel-to-settled 56.00724 ms.

### refinement/native-0-before

Sessions 128; seed history 8; requests 1235; schema [93]; diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 14.28125 | 46.53906 | 0/0/0 | 0 |
| metadata_only | 30.68750 | 73.44531 | 0/0/0 | 0 |
| empty_attached_1 | 103.45703 | 153.99609 | 1/0/0 | 0 |
| empty_attached_8 | 119.66406 | 170.60938 | 8/0/0 | 0 |
| empty_attached_32 | 162.69922 | 214.39453 | 32/0/0 | 0 |
| empty_attached_64 | 221.25000 | 273.94531 | 64/0/0 | 0 |
| empty_attached_128 | 337.16016 | 391.85547 | 128/0/0 | 0 |
| subscribed_1 | 337.32812 | 393.14844 | 128/0/0 | 0 |
| subscribed_8 | 337.48828 | 393.30859 | 128/0/0 | 0 |
| subscribed_32 | 337.53516 | 393.35547 | 128/0/0 | 0 |
| subscribed_64 | 337.56641 | 393.38672 | 128/0/0 | 0 |
| subscribed_128 | 337.57031 | 393.39062 | 128/0/0 | 0 |
| all_subscribed_empty | 337.57031 | 393.39062 | 128/0/0 | 0 |
| seeded_history_1 | 528.63672 | 592.33203 | 128/0/0 | 0 |
| seeded_history_8 | 925.25000 | 988.94531 | 128/0/0 | 0 |
| active_held_1 | 925.50000 | 989.19531 | 128/0/0 | 1 |
| active_finished_1 | 936.43359 | 1000.12891 | 128/0/0 | 0 |
| active_held_4 | 937.24219 | 1000.93750 | 128/0/0 | 4 |
| active_finished_4 | 962.43359 | 1026.12891 | 128/0/0 | 0 |
| active_held_8 | 966.74219 | 1030.43750 | 128/0/0 | 8 |
| active_finished_8 | 1013.03516 | 1076.73047 | 128/0/0 | 0 |
| extra_controller_connections | 1014.16797 | 1078.36328 | 128/0/0 | 0 |
| history_browsed | 1016.20703 | 1080.58984 | 128/0/0 | 0 |
| slow_consumer_held | 1016.95703 | 1081.33984 | 128/0/0 | 1 |
| slow_consumer_settled | 1017.04297 | 1081.42578 | 128/0/0 | 0 |
| all_unloaded | 1014.20703 | 1075.33984 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 1014.21484 | 1075.47266 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 1014.22266 | 1075.48047 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 1014.23047 | 1075.48828 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 7.78296 | 22.20635 | 60.65592 | 60.65592 | 16.87500 | 909.25000 |
| 4 | 60 | 18.78110 | 53.33500 | 86.90369 | 97.43457 | 15.31250 | 941.18750 |
| 8 | 120 | 32.48615 | 81.38126 | 124.45049 | 132.92444 | 15.00000 | 975.28125 |

Used-page bytes 141021184; free pages 4; post-exit main bytes 141037568; WAL bytes 0; SHM bytes 0; cancel-to-settled 14.79585 ms.

### refinement/native-0-after

Sessions 128; seed history 8; requests 1235; schema [94]; diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 14.20312 | 46.04688 | 0/0/0 | 0 |
| metadata_only | 32.76953 | 74.92578 | 0/0/0 | 0 |
| empty_attached_1 | 105.89844 | 155.83594 | 1/0/0 | 0 |
| empty_attached_8 | 119.15625 | 169.50000 | 8/0/0 | 0 |
| empty_attached_32 | 161.13281 | 212.22656 | 32/0/0 | 0 |
| empty_attached_64 | 216.97656 | 269.07031 | 64/0/0 | 0 |
| empty_attached_128 | 329.75000 | 383.84375 | 128/0/0 | 0 |
| subscribed_1 | 329.82031 | 385.22656 | 128/0/0 | 0 |
| subscribed_8 | 329.82422 | 385.23047 | 128/0/0 | 0 |
| subscribed_32 | 329.89062 | 385.29688 | 128/0/0 | 0 |
| subscribed_64 | 329.89062 | 385.29688 | 128/0/0 | 0 |
| subscribed_128 | 329.89062 | 385.29688 | 128/0/0 | 0 |
| all_subscribed_empty | 329.89062 | 385.29688 | 128/0/0 | 0 |
| seeded_history_1 | 490.76562 | 553.98438 | 128/0/0 | 0 |
| seeded_history_8 | 778.55078 | 841.76953 | 128/0/0 | 0 |
| active_held_1 | 778.55859 | 841.77734 | 128/0/0 | 1 |
| active_finished_1 | 788.00391 | 851.22266 | 128/0/0 | 0 |
| active_held_4 | 790.32812 | 853.54688 | 128/0/0 | 4 |
| active_finished_4 | 804.82422 | 868.04297 | 128/0/0 | 0 |
| active_held_8 | 806.09766 | 869.31641 | 128/0/0 | 8 |
| active_finished_8 | 843.74219 | 906.96094 | 128/0/0 | 0 |
| extra_controller_connections | 844.47656 | 908.00781 | 128/0/0 | 0 |
| history_browsed | 847.11719 | 910.64844 | 128/0/0 | 0 |
| slow_consumer_held | 849.32031 | 912.91406 | 128/0/0 | 1 |
| slow_consumer_settled | 849.40234 | 912.99609 | 128/0/0 | 0 |
| all_unloaded | 849.80469 | 910.02344 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 849.81250 | 910.09375 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 849.93359 | 910.21484 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 849.93359 | 910.21484 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 7.94568 | 50.69605 | 61.17876 | 61.17876 | 19.37500 | 685.50000 |
| 4 | 60 | 14.76788 | 38.71438 | 79.29864 | 83.66045 | 14.68750 | 711.37500 |
| 8 | 120 | 24.46736 | 69.82797 | 107.58535 | 122.37879 | 14.14062 | 737.18750 |

Used-page bytes 109471744; free pages 9; post-exit main bytes 109490176; WAL bytes 0; SHM bytes 0; cancel-to-settled 8.33677 ms.

### refinement/native-1-after

Sessions 128; seed history 8; requests 1235; schema [94]; diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 14.29297 | 46.33203 | 0/0/0 | 0 |
| metadata_only | 30.16016 | 72.26172 | 0/0/0 | 0 |
| empty_attached_1 | 102.36328 | 151.80859 | 1/0/0 | 0 |
| empty_attached_8 | 119.32812 | 169.17969 | 8/0/0 | 0 |
| empty_attached_32 | 161.33203 | 211.91797 | 32/0/0 | 0 |
| empty_attached_64 | 217.25000 | 268.83594 | 64/0/0 | 0 |
| empty_attached_128 | 329.51562 | 383.08594 | 128/0/0 | 0 |
| subscribed_1 | 329.51953 | 384.21484 | 128/0/0 | 0 |
| subscribed_8 | 329.58984 | 384.28516 | 128/0/0 | 0 |
| subscribed_32 | 329.65625 | 384.35156 | 128/0/0 | 0 |
| subscribed_64 | 329.65625 | 384.35156 | 128/0/0 | 0 |
| subscribed_128 | 329.65625 | 384.35156 | 128/0/0 | 0 |
| all_subscribed_empty | 329.65625 | 384.35156 | 128/0/0 | 0 |
| seeded_history_1 | 486.20703 | 549.18359 | 128/0/0 | 0 |
| seeded_history_8 | 781.83203 | 844.80859 | 128/0/0 | 0 |
| active_held_1 | 783.40625 | 846.38281 | 128/0/0 | 1 |
| active_finished_1 | 789.60547 | 852.58203 | 128/0/0 | 0 |
| active_held_4 | 790.94531 | 853.92188 | 128/0/0 | 4 |
| active_finished_4 | 804.87500 | 867.85156 | 128/0/0 | 0 |
| active_held_8 | 810.33984 | 873.31641 | 128/0/0 | 8 |
| active_finished_8 | 841.69922 | 904.67578 | 128/0/0 | 0 |
| extra_controller_connections | 842.77344 | 906.06250 | 128/0/0 | 0 |
| history_browsed | 844.38281 | 907.73438 | 128/0/0 | 0 |
| slow_consumer_held | 846.42188 | 909.83594 | 128/0/0 | 1 |
| slow_consumer_settled | 846.67969 | 910.09375 | 128/0/0 | 0 |
| all_unloaded | 847.34766 | 907.51172 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 846.30859 | 906.53516 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 845.32422 | 905.55078 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 845.32422 | 905.55078 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 6.60006 | 22.35558 | 60.75015 | 60.75015 | 18.12500 | 685.25000 |
| 4 | 60 | 16.63126 | 40.99375 | 81.56773 | 87.76400 | 14.53125 | 710.56250 |
| 8 | 120 | 27.02382 | 72.33097 | 114.03696 | 120.81502 | 14.53125 | 736.93750 |

Used-page bytes 109481984; free pages 8; post-exit main bytes 109498368; WAL bytes 0; SHM bytes 0; cancel-to-settled 9.31268 ms.

### refinement/native-1-before

Sessions 128; seed history 8; requests 1235; schema [93]; diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 14.19922 | 46.38672 | 0/0/0 | 0 |
| metadata_only | 32.37891 | 75.06641 | 0/0/0 | 0 |
| empty_attached_1 | 105.00391 | 155.47266 | 1/0/0 | 0 |
| empty_attached_8 | 119.82422 | 170.69922 | 8/0/0 | 0 |
| empty_attached_32 | 163.06250 | 214.68750 | 32/0/0 | 0 |
| empty_attached_64 | 221.34375 | 273.96875 | 64/0/0 | 0 |
| empty_attached_128 | 337.75391 | 392.35156 | 128/0/0 | 0 |
| subscribed_1 | 337.82422 | 393.54688 | 128/0/0 | 0 |
| subscribed_8 | 337.89453 | 393.61719 | 128/0/0 | 0 |
| subscribed_32 | 337.96094 | 393.68359 | 128/0/0 | 0 |
| subscribed_64 | 337.96094 | 393.68359 | 128/0/0 | 0 |
| subscribed_128 | 337.96094 | 393.68359 | 128/0/0 | 0 |
| all_subscribed_empty | 337.96094 | 393.68359 | 128/0/0 | 0 |
| seeded_history_1 | 533.08203 | 596.70703 | 128/0/0 | 0 |
| seeded_history_8 | 924.03906 | 987.66406 | 128/0/0 | 0 |
| active_held_1 | 926.05078 | 989.67578 | 128/0/0 | 1 |
| active_finished_1 | 937.85547 | 1001.48047 | 128/0/0 | 0 |
| active_held_4 | 941.07031 | 1004.69531 | 128/0/0 | 4 |
| active_finished_4 | 961.72656 | 1025.35156 | 128/0/0 | 0 |
| active_held_8 | 963.48047 | 1027.10547 | 128/0/0 | 8 |
| active_finished_8 | 1014.01172 | 1077.63672 | 128/0/0 | 0 |
| extra_controller_connections | 1015.64844 | 1079.77344 | 128/0/0 | 0 |
| history_browsed | 1017.76172 | 1082.07422 | 128/0/0 | 0 |
| slow_consumer_held | 1019.45703 | 1083.76953 | 128/0/0 | 1 |
| slow_consumer_settled | 1020.04688 | 1084.35938 | 128/0/0 | 0 |
| all_unloaded | 1017.87109 | 1078.93359 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 1017.87891 | 1079.06641 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 1017.88672 | 1079.07422 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 1017.88672 | 1079.07422 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 6.57940 | 19.58026 | 55.16953 | 55.16953 | 16.25000 | 910.50000 |
| 4 | 60 | 14.64551 | 56.87868 | 1671.06683 | 1883.09287 | 14.37500 | 933.81250 |
| 8 | 120 | 39.70321 | 155.63199 | 907.63216 | 1163.33089 | 14.68750 | 978.84375 |

Used-page bytes 141025280; free pages 4; post-exit main bytes 141041664; WAL bytes 0; SHM bytes 0; cancel-to-settled 222.96007 ms.

### refinement/history-512-before

Sessions 2; seed history 512; requests 1075; schema [93]; diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 14.19922 | 46.76562 | 0/0/0 | 0 |
| metadata_only | 16.02734 | 58.71875 | 0/0/0 | 0 |
| empty_attached_1 | 89.15625 | 139.62891 | 1/0/0 | 0 |
| empty_attached_2 | 95.14453 | 145.83594 | 2/0/0 | 0 |
| subscribed_1 | 95.27344 | 147.02734 | 2/0/0 | 0 |
| subscribed_2 | 95.33984 | 147.09375 | 2/0/0 | 0 |
| all_subscribed_empty | 95.33984 | 147.09375 | 2/0/0 | 0 |
| seeded_history_1 | 98.58203 | 157.89844 | 2/0/0 | 0 |
| seeded_history_512 | 191.69141 | 251.00781 | 2/0/0 | 0 |
| active_held_1 | 191.74219 | 251.05859 | 2/0/0 | 1 |
| active_finished_1 | 192.75000 | 252.06641 | 2/0/0 | 0 |
| active_held_2 | 192.75000 | 252.06641 | 2/0/0 | 2 |
| active_finished_2 | 194.44141 | 253.75781 | 2/0/0 | 0 |
| extra_controller_connections | 194.44141 | 253.75781 | 2/0/0 | 0 |
| history_browsed | 194.44141 | 253.75781 | 2/0/0 | 0 |
| slow_consumer_held | 194.44531 | 254.19922 | 2/0/0 | 1 |
| slow_consumer_settled | 194.44531 | 254.19922 | 2/0/0 | 0 |
| all_unloaded | 189.11719 | 249.62109 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 197.34375 | 258.03516 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 197.49609 | 258.18750 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 206.83594 | 267.52734 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 5.21622 | 52.08051 | 55.31011 | 55.31011 | 17.50000 | 1232.75000 |
| 2 | 30 | 7.00841 | 29.34640 | 65.73139 | 70.96776 | 18.43750 | 1184.12500 |

Used-page bytes 52244480; free pages 6; post-exit main bytes 52269056; WAL bytes 0; SHM bytes 0; cancel-to-settled 47.77369 ms.

### refinement/history-512-after

Sessions 2; seed history 512; requests 1075; schema [94]; diagnostic False.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 14.19922 | 46.40625 | 0/0/0 | 0 |
| metadata_only | 15.58203 | 58.41406 | 0/0/0 | 0 |
| empty_attached_1 | 88.28125 | 138.45703 | 1/0/0 | 0 |
| empty_attached_2 | 91.19531 | 141.58984 | 2/0/0 | 0 |
| subscribed_1 | 91.33594 | 142.85547 | 2/0/0 | 0 |
| subscribed_2 | 91.33594 | 142.85547 | 2/0/0 | 0 |
| all_subscribed_empty | 91.33594 | 142.85547 | 2/0/0 | 0 |
| seeded_history_1 | 95.99609 | 155.26562 | 2/0/0 | 0 |
| seeded_history_512 | 174.55078 | 233.82031 | 2/0/0 | 0 |
| active_held_1 | 174.98438 | 234.25391 | 2/0/0 | 1 |
| active_finished_1 | 175.23828 | 234.50781 | 2/0/0 | 0 |
| active_held_2 | 175.23828 | 234.50781 | 2/0/0 | 2 |
| active_finished_2 | 176.93359 | 236.20312 | 2/0/0 | 0 |
| extra_controller_connections | 176.93359 | 236.20312 | 2/0/0 | 0 |
| history_browsed | 176.93359 | 236.20312 | 2/0/0 | 0 |
| slow_consumer_held | 176.94531 | 237.08984 | 2/0/0 | 1 |
| slow_consumer_settled | 176.94531 | 237.08984 | 2/0/0 | 0 |
| all_unloaded | 173.57422 | 234.40625 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 176.98047 | 238.00000 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 188.08984 | 249.10938 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 188.20703 | 249.22656 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 15 | 6.98768 | 53.33379 | 64.29459 | 64.29459 | 23.75000 | 870.50000 |
| 2 | 30 | 9.73428 | 32.31566 | 70.51638 | 74.31272 | 20.93750 | 909.37500 |

Used-page bytes 54249472; free pages 11; post-exit main bytes 54272000; WAL bytes 0; SHM bytes 0; cancel-to-settled 11.03460 ms.

### refinement/sampling-128-after

Sessions 128; seed history 8; requests 1053; schema [94]; diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 18.98438 | 56.64453 | 0/0/0 | 0 |
| metadata_only | 38.05859 | 87.84375 | 0/0/0 | 0 |
| empty_attached_1 | 111.14062 | 169.20703 | 1/0/0 | 0 |
| empty_attached_8 | 125.81250 | 184.22266 | 8/0/0 | 0 |
| empty_attached_32 | 171.30469 | 230.71484 | 32/0/0 | 0 |
| empty_attached_64 | 230.60156 | 291.07422 | 64/0/0 | 0 |
| empty_attached_128 | 347.69141 | 410.22656 | 128/0/0 | 0 |
| subscribed_1 | 354.07031 | 418.03516 | 128/0/0 | 0 |
| subscribed_8 | 354.16016 | 418.12500 | 128/0/0 | 0 |
| subscribed_32 | 354.61719 | 418.58203 | 128/0/0 | 0 |
| subscribed_64 | 355.75000 | 419.71484 | 128/0/0 | 0 |
| subscribed_128 | 355.79688 | 419.76172 | 128/0/0 | 0 |
| all_subscribed_empty | 355.79688 | 419.76172 | 128/0/0 | 0 |
| seeded_history_1 | 536.91797 | 609.00781 | 128/0/0 | 0 |
| seeded_history_8 | 845.28516 | 917.43750 | 128/0/0 | 0 |
| active_held_1 | 862.75391 | 934.90625 | 128/0/0 | 1 |
| active_finished_1 | 862.80078 | 934.95312 | 128/0/0 | 0 |
| active_held_4 | 862.86328 | 935.01562 | 128/0/0 | 4 |
| active_finished_4 | 862.91016 | 935.06250 | 128/0/0 | 0 |
| active_held_8 | 863.10938 | 935.26172 | 128/0/0 | 8 |
| active_finished_8 | 863.22266 | 935.37500 | 128/0/0 | 0 |
| extra_controller_connections | 863.32812 | 935.98047 | 128/0/0 | 0 |
| history_browsed | 869.39453 | 942.17188 | 128/0/0 | 0 |
| slow_consumer_held | 869.51953 | 942.35938 | 128/0/0 | 1 |
| slow_consumer_settled | 869.54297 | 942.38281 | 128/0/0 | 0 |
| all_unloaded | 869.67578 | 939.39062 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 870.00000 | 939.90234 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 870.09766 | 940.00000 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 870.19531 | 940.09766 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 8.85874 | 16.92879 | 16.92879 | 16.92879 | 20.00000 | 522.00000 |
| 4 | 4 | 12.90248 | 39.87951 | 87.89521 | 87.89521 | 17.50000 | 596.00000 |
| 8 | 8 | 17.45039 | 92.10371 | 118.77264 | 118.77264 | 17.50000 | 641.50000 |

Used-page bytes 100585472; free pages 9; post-exit main bytes 100603904; WAL bytes 0; SHM bytes 0; cancel-to-settled 17.41244 ms.

### sql-fields-eight-refined

Sessions 8; seed history 8; requests 93; schema [94]; diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 21.80469 | 96.72656 | 0/0/0 | 0 |
| metadata_only | 24.97266 | 112.83203 | 0/0/0 | 0 |
| empty_attached_1 | 95.32812 | 195.03125 | 1/0/0 | 0 |
| empty_attached_8 | 109.43750 | 210.29688 | 8/0/0 | 0 |
| subscribed_1 | 109.80078 | 211.47266 | 8/0/0 | 0 |
| subscribed_8 | 110.48047 | 212.15234 | 8/0/0 | 0 |
| all_subscribed_empty | 110.49609 | 212.16797 | 8/0/0 | 0 |
| seeded_history_1 | 126.14453 | 241.56641 | 8/0/0 | 0 |
| seeded_history_8 | 148.42969 | 265.78906 | 8/0/0 | 0 |
| active_held_1 | 148.83984 | 266.19922 | 8/0/0 | 1 |
| active_finished_1 | 149.31250 | 266.67188 | 8/0/0 | 0 |
| active_held_4 | 150.64453 | 268.06641 | 8/0/0 | 4 |
| active_finished_4 | 152.14844 | 269.63281 | 8/0/0 | 0 |
| active_held_8 | 155.28516 | 272.76953 | 8/0/0 | 8 |
| active_finished_8 | 158.74609 | 276.54297 | 8/0/0 | 0 |
| extra_controller_connections | 159.12500 | 276.92188 | 8/0/0 | 0 |
| history_browsed | 162.67969 | 280.47656 | 8/0/0 | 0 |
| slow_consumer_held | 163.60938 | 282.96875 | 8/0/0 | 1 |
| slow_consumer_settled | 163.82422 | 283.18359 | 8/0/0 | 0 |
| all_unloaded | 160.28125 | 280.69922 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 160.73438 | 281.21484 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 160.74609 | 281.22656 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 160.75391 | 281.23438 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 21.58757 | 49.25491 | 49.25491 | 49.25491 | 60.00000 | 380.00000 |
| 4 | 4 | 22.73293 | 113.27593 | 159.59991 | 159.59991 | 48.75000 | 457.50000 |
| 8 | 8 | 41.66571 | 208.21645 | 266.31312 | 266.31312 | 48.12500 | 500.25000 |

Used-page bytes 7700480; free pages 11; post-exit main bytes 7723008; WAL bytes 0; SHM bytes 0; cancel-to-settled 21.42227 ms.

### sql-fields-128-refined

Sessions 128; seed history 8; requests 1053; schema [94]; diagnostic True.

| Phase | Anon PSS MiB | RSS MiB | Loaded/loading/unloading | Active |
| --- | --- | --- | --- | --- |
| empty_server | 21.79688 | 96.71875 | 0/0/0 | 0 |
| metadata_only | 40.44922 | 130.74609 | 0/0/0 | 0 |
| empty_attached_1 | 109.70703 | 211.28516 | 1/0/0 | 0 |
| empty_attached_8 | 124.86719 | 227.85156 | 8/0/0 | 0 |
| empty_attached_32 | 166.73828 | 270.66016 | 32/0/0 | 0 |
| empty_attached_64 | 222.35156 | 327.64844 | 64/0/0 | 0 |
| empty_attached_128 | 331.64062 | 439.49219 | 128/0/0 | 0 |
| subscribed_1 | 331.87500 | 440.60156 | 128/0/0 | 0 |
| subscribed_8 | 332.22266 | 441.01172 | 128/0/0 | 0 |
| subscribed_32 | 334.28906 | 443.07812 | 128/0/0 | 0 |
| subscribed_64 | 335.79688 | 444.58594 | 128/0/0 | 0 |
| subscribed_128 | 337.37500 | 446.16406 | 128/0/0 | 0 |
| all_subscribed_empty | 337.37500 | 446.16406 | 128/0/0 | 0 |
| seeded_history_1 | 511.69531 | 634.86719 | 128/0/0 | 1 |
| seeded_history_8 | 801.26953 | 925.00391 | 128/0/0 | 0 |
| active_held_1 | 801.42969 | 925.16406 | 128/0/0 | 1 |
| active_finished_1 | 801.62891 | 925.36328 | 128/0/0 | 0 |
| active_held_4 | 803.35156 | 927.08594 | 128/0/0 | 4 |
| active_finished_4 | 805.26172 | 928.99609 | 128/0/0 | 1 |
| active_held_8 | 807.97266 | 931.70703 | 128/0/0 | 8 |
| active_finished_8 | 810.61719 | 934.41406 | 128/0/0 | 0 |
| extra_controller_connections | 815.07422 | 940.24609 | 128/0/0 | 0 |
| history_browsed | 818.35938 | 943.59375 | 128/0/0 | 0 |
| slow_consumer_held | 818.85156 | 944.21094 | 128/0/0 | 1 |
| slow_consumer_settled | 819.02344 | 944.38281 | 128/0/0 | 0 |
| all_unloaded | 813.98828 | 936.46875 | 0/0/0 | 0 |
| reload_unload_cycle_0 | 814.16406 | 936.64453 | 0/0/0 | 0 |
| reload_unload_cycle_1 | 814.17188 | 936.65234 | 0/0/0 | 0 |
| reload_unload_cycle_2 | 814.18750 | 936.66797 | 0/0/0 | 0 |

| Width | Nonheld n | Accepted median ms | Settled median ms | P95 ms | P99 ms | CPU ms/all turns | Write KiB/all turns |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 | 22.67903 | 43.92920 | 43.92920 | 43.92920 | 65.00000 | 522.00000 |
| 4 | 4 | 45.06081 | 126.08672 | 134.58980 | 134.58980 | 61.25000 | 600.50000 |
| 8 | 8 | 60.10220 | 231.10744 | 279.80121 | 279.80121 | 60.62500 | 643.25000 |

Used-page bytes 100589568; free pages 9; post-exit main bytes 100608000; WAL bytes 0; SHM bytes 0; cancel-to-settled 19.99822 ms.

Latency excludes the first deliberately held wave. Native CPU/write windows include all waves and orchestration gaps; burst service throughput excludes gate setup gaps. Replacement-allocator and root-O0 diagnostics are not ordinary glibc memory/timing. P95/P99 are small-sample descriptions.

## Initial full sampled allocation export

| Side/phase | Raw records | Raw sampled bytes | Weighted requested MiB | Collapsed requested MiB | Allocator charged MiB | Allocator heap MiB | Page-free MiB |
| --- | --- | --- | --- | --- | --- | --- | --- |
| before/empty_attached_128 | 4786 | 11224271 | 305.47373 | 305.47329 | 336.58508 | 368.99219 | 8.35938 |
| before/seeded_history_8 | 13206 | 56385274 | 856.58951 | 856.58860 | 944.55851 | 1016.28906 | 16.28125 |
| before/all_unloaded | 1354 | 3342297 | 86.98750 | 86.98729 | 96.40189 | 1025.28906 | 708.96875 |
| after/empty_attached_128 | 4550 | 9756303 | 290.01067 | 290.01025 | 328.17905 | 358.93750 | 7.76562 |
| after/seeded_history_8 | 11228 | 36532178 | 720.92535 | 720.92445 | 803.02104 | 869.75000 | 16.13281 |
| after/all_unloaded | 1329 | 3610800 | 85.60921 | 85.60900 | 96.23702 | 875.75000 | 578.12500 |

| Side/phase | Overlapping stack group | Weighted requested MiB |
| --- | --- | --- |
| before/empty_attached_128 | sqlite_malloc | 78.91467 |
| before/empty_attached_128 | sqlite_page_cache | 30.68589 |
| before/empty_attached_128 | schema_compilation | 63.09311 |
| before/empty_attached_128 | checkpoint_materialization | 0.00000 |
| before/empty_attached_128 | publication_encoding | 18.02524 |
| before/empty_attached_128 | replay_deque_growth | 0.06306 |
| before/seeded_history_8 | sqlite_malloc | 236.70680 |
| before/seeded_history_8 | sqlite_page_cache | 127.40569 |
| before/seeded_history_8 | schema_compilation | 63.09311 |
| before/seeded_history_8 | checkpoint_materialization | 120.15950 |
| before/seeded_history_8 | publication_encoding | 222.11951 |
| before/seeded_history_8 | replay_deque_growth | 19.15280 |
| before/all_unloaded | sqlite_malloc | 0.00000 |
| before/all_unloaded | sqlite_page_cache | 0.00000 |
| before/all_unloaded | schema_compilation | 63.09311 |
| before/all_unloaded | checkpoint_materialization | 0.00000 |
| before/all_unloaded | publication_encoding | 2.47485 |
| before/all_unloaded | replay_deque_growth | 0.00000 |
| after/empty_attached_128 | sqlite_malloc | 70.82083 |
| after/empty_attached_128 | sqlite_page_cache | 20.73644 |
| after/empty_attached_128 | schema_compilation | 64.46953 |
| after/empty_attached_128 | checkpoint_materialization | 0.00000 |
| after/empty_attached_128 | publication_encoding | 16.87460 |
| after/empty_attached_128 | replay_deque_growth | 0.31795 |
| after/seeded_history_8 | sqlite_malloc | 230.11988 |
| after/seeded_history_8 | sqlite_page_cache | 116.79652 |
| after/seeded_history_8 | schema_compilation | 64.46953 |
| after/seeded_history_8 | checkpoint_materialization | 20.91250 |
| after/seeded_history_8 | publication_encoding | 123.26202 |
| after/seeded_history_8 | replay_deque_growth | 9.00016 |
| after/all_unloaded | sqlite_malloc | 0.00000 |
| after/all_unloaded | sqlite_page_cache | 0.00000 |
| after/all_unloaded | schema_compilation | 64.46953 |
| after/all_unloaded | checkpoint_materialization | 0.00000 |
| after/all_unloaded | publication_encoding | 2.06795 |
| after/all_unloaded | replay_deque_growth | 0.00000 |

Raw sampled counts/bytes exactly cover exported header. Independent Poisson correction agrees with pprof collapsed integer weights within per-stack rounding. Requested-byte estimates differ from allocator class-charged totals and ordinary glibc PSS. Stack-presence groups overlap, optimized aliases/inlining limit attribution; never normalize them to925MiB or add inclusive groups.

## Final full sampled allocation export

| Side/phase | Raw records | Raw sampled bytes | Weighted requested MiB | Collapsed requested MiB | Allocator charged MiB | Allocator heap MiB | Page-free MiB |
| --- | --- | --- | --- | --- | --- | --- | --- |
| before/empty_attached_128 | 4786 | 11224271 | 305.47373 | 305.47329 | 336.58508 | 368.99219 | 8.35938 |
| before/seeded_history_8 | 13206 | 56385274 | 856.58951 | 856.58860 | 944.55851 | 1016.28906 | 16.28125 |
| before/all_unloaded | 1354 | 3342297 | 86.98750 | 86.98729 | 96.40189 | 1025.28906 | 708.96875 |
| after/empty_attached_128 | 4702 | 10889816 | 300.13816 | 300.13771 | 329.34879 | 359.97656 | 6.28906 |
| after/seeded_history_8 | 11420 | 37955841 | 733.69451 | 733.69357 | 801.16939 | 865.83594 | 15.92188 |
| after/all_unloaded | 1415 | 3840465 | 91.10249 | 91.10227 | 96.86452 | 871.83594 | 572.89844 |

| Side/phase | Overlapping stack group | Weighted requested MiB |
| --- | --- | --- |
| before/empty_attached_128 | sqlite_malloc | 78.91467 |
| before/empty_attached_128 | sqlite_page_cache | 30.68589 |
| before/empty_attached_128 | schema_compilation | 63.09311 |
| before/empty_attached_128 | checkpoint_materialization | 0.00000 |
| before/empty_attached_128 | publication_encoding | 18.02524 |
| before/empty_attached_128 | replay_deque_growth | 0.06306 |
| before/seeded_history_8 | sqlite_malloc | 236.70680 |
| before/seeded_history_8 | sqlite_page_cache | 127.40569 |
| before/seeded_history_8 | schema_compilation | 63.09311 |
| before/seeded_history_8 | checkpoint_materialization | 120.15950 |
| before/seeded_history_8 | publication_encoding | 222.11951 |
| before/seeded_history_8 | replay_deque_growth | 19.15280 |
| before/all_unloaded | sqlite_malloc | 0.00000 |
| before/all_unloaded | sqlite_page_cache | 0.00000 |
| before/all_unloaded | schema_compilation | 63.09311 |
| before/all_unloaded | checkpoint_materialization | 0.00000 |
| before/all_unloaded | publication_encoding | 2.47485 |
| before/all_unloaded | replay_deque_growth | 0.00000 |
| after/empty_attached_128 | sqlite_malloc | 70.09793 |
| after/empty_attached_128 | sqlite_page_cache | 19.91083 |
| after/empty_attached_128 | schema_compilation | 67.71980 |
| after/empty_attached_128 | checkpoint_materialization | 0.00000 |
| after/empty_attached_128 | publication_encoding | 17.03117 |
| after/empty_attached_128 | replay_deque_growth | 0.31795 |
| after/seeded_history_8 | sqlite_malloc | 231.95834 |
| after/seeded_history_8 | sqlite_page_cache | 117.05291 |
| after/seeded_history_8 | schema_compilation | 67.71980 |
| after/seeded_history_8 | checkpoint_materialization | 21.46741 |
| after/seeded_history_8 | publication_encoding | 127.97142 |
| after/seeded_history_8 | replay_deque_growth | 9.00016 |
| after/all_unloaded | sqlite_malloc | 0.00000 |
| after/all_unloaded | sqlite_page_cache | 0.00000 |
| after/all_unloaded | schema_compilation | 67.71980 |
| after/all_unloaded | checkpoint_materialization | 0.00000 |
| after/all_unloaded | publication_encoding | 2.35723 |
| after/all_unloaded | replay_deque_growth | 0.00000 |

Raw sampled counts/bytes exactly cover exported header. Independent Poisson correction agrees with pprof collapsed integer weights within per-stack rounding. Requested-byte estimates differ from allocator class-charged totals and ordinary glibc PSS. Stack-presence groups overlap, optimized aliases/inlining limit attribution; never normalize them to925MiB or add inclusive groups.

## Direct SQLite and replay-release diagnostics

| Product | Seq64 connections | Pager cache MiB/connection | Statement MiB/connection | Schema MiB/connection | Plans |
| --- | --- | --- | --- | --- | --- |
| sql-replay-serial-before | 128 | 0.922997 | 0.705711 | 0.208549 | 210.0 |
| sql-fields-128-refined | 128 | 0.801414 | 0.705124 | 0.208466 | 210.0 |

| Product | Normal 8-history release observations | Median replay-release MiB | Min MiB | Max MiB | Median retained events | Median replay wire-size budget MiB |
| --- | --- | --- | --- | --- | --- | --- |
| sql-replay-serial-before | 95 | 2.666611 | 2.563904 | 2.731873 | 104 | 2.358741 |
| sql-fields-128-refined | 95 | 1.702164 | 1.595367 | 1.756943 | 105 | 2.362629 |

Private root-O0/default-feature diagnostic binaries, separate from ordinary O3 native results. SQLite db_status gives approximate per-connection pager/schema/statement counters. Normal8-history replay uses95untouched sessions32..126, excluding active prefixes, transferred observers and slow-last-session. Global allocator before/after differences retain all outliers and can overlap asynchronous teardown; report medians/ranges, not an exact total925MiB partition. Shared owners can survive replay release.

Replay wire bytes are admission accounting for typed retained events, not a separately held encoded buffer. Full per-owner release values, per-field attribution samples, raw counters, exact wire/native oracles, all transaction page counts and page-owner associations are retained in the raw archive. Inclusive attribution and noisy global release differences are not additive exact partitions of ordinary PSS.
