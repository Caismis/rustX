# Final complete-costs audited tables

来源：`complete-all-audit.json`，SHA256 `7555feb0207f9289679325c9c95ca901976e32776b4e1d49fa7b109fbaa533b6`；审计模式 `strict`。
候选 `f5cf22a8fbd2b1f0b3abc7fc2082a586f1126e20` / binary `8d033129f343660821b5fd7cef957a424e6d769c55948011f2798c6e2f3a0cb7`；基线 `123accb38fd8cf1e8f88583c1858d514cb453870` / binary `6899c8fad1fc85e8ffd36a33b92e85cec877cda3689bb26a03c6f07c658a3d43`。
汇总脚本 SHA256 `c645dd8f650abec253f5f045741cb21954499675191197ff81662e9f4d07ac73`。本文件只格式化已接受审计的派生字段，不重读原始计数、不合并探索样本、不重新选样。

正式范围：direct 120、fresh Pi 120、native reference 36 个产品；每个 direct/Pi cell 固定五对，三引擎每个 case/engine 固定三次。no-live/live × 六 case 全部展示。五对仅为描述统计，不满足原七对完整追平门槛；不作跨 case 或跨 mode 的整体追平结论。

## Direct：最终候选 / 上轮 joint

百分数为审计中五个逐对百分比的中位数；负值更低。wall均值使用 `mean_turn_ms`，wall中位使用每个产品的 `median_measured_turn_ms`，两者均非跨产品混池。完整CPU/进程写入是 through-cleanup 累计值，包含初始化、历史、验证与关闭；匿名PSS为所有已采样点的最大值。

| 模式 | case | wall均值 Δ% | wall中位 Δ% | 完整self CPU Δ% | 完整进程write bytes Δ% | 采样匿名PSS Δ% |
| --- | --- | --- | --- | --- | --- | --- |
| no-live | short-small | -22.37% (5/5) | -15.27% (5/5) | -13.69% (5/5) | -6.08% (5/5) | -0.62% (5/5) |
| no-live | short-large | -21.11% (5/5) | -24.36% (5/5) | -11.52% (5/5) | -6.18% (5/5) | -1.35% (5/5) |
| no-live | stream | -0.54% (5/5) | -0.54% (5/5) | -3.12% (5/5) | -5.80% (5/5) | -0.94% (5/5) |
| no-live | large | -11.82% (5/5) | -11.82% (5/5) | -5.56% (5/5) | -6.24% (5/5) | -0.78% (5/5) |
| no-live | tool | -16.78% (5/5) | -16.78% (5/5) | -14.29% (5/5) | -6.57% (5/5) | -0.80% (5/5) |
| no-live | cancel | -14.79% (5/5) | -14.79% (5/5) | +0.00% (5/5) | -5.92% (5/5) | -0.48% (5/5) |
| live | short-small | -17.52% (5/5) | -17.94% (5/5) | -10.21% (5/5) | -6.03% (5/5) | -1.81% (5/5) |
| live | short-large | -38.23% (5/5) | -32.33% (5/5) | -14.53% (5/5) | -6.12% (5/5) | +0.12% (5/5) |
| live | stream | -1.50% (5/5) | -1.50% (5/5) | -6.67% (5/5) | -5.80% (5/5) | -0.90% (5/5) |
| live | large | -0.78% (5/5) | -0.78% (5/5) | -11.11% (5/5) | -6.24% (5/5) | -0.65% (5/5) |
| live | tool | -14.51% (5/5) | -14.51% (5/5) | +0.00% (5/5) | -6.43% (5/5) | +0.33% (5/5) |
| live | cancel | -0.82% (5/5) | -0.82% (5/5) | -5.88% (5/5) | -5.92% (5/5) | -0.54% (5/5) |

## Fresh Pi：最终 rustX / Pi Durable

以下为逐对比值的中位数 [最小值, 最大值]；1×为相等，大于1×表示 rustX 较高。括号保留可用配对数。CPU/写入/PSS与 direct 表使用同一边界。

| 模式 | case | wall均值 × [min,max] | wall中位 × [min,max] | 完整self CPU × [min,max] | 完整write bytes × [min,max] | 采样匿名PSS × [min,max] |
| --- | --- | --- | --- | --- | --- | --- |
| no-live | short-small | 5.678 [3.685, 6.318] (5/5) | 6.901 [6.107, 7.273] (5/5) | 1.574 [1.525, 1.667] (5/5) | 6.797 [6.781, 6.808] (5/5) | 1.129 [1.109, 1.194] (5/5) |
| no-live | short-large | 5.888 [4.189, 6.738] (5/5) | 5.200 [4.512, 7.513] (5/5) | 3.121 [3.022, 3.222] (5/5) | 7.649 [7.635, 7.650] (5/5) | 1.287 [1.273, 1.384] (5/5) |
| no-live | stream | 0.900 [0.892, 0.940] (5/5) | 0.900 [0.892, 0.940] (5/5) | 0.235 [0.223, 0.274] (5/5) | 4.718 [4.502, 4.727] (5/5) | 0.865 [0.843, 0.904] (5/5) |
| no-live | large | 0.441 [0.348, 0.533] (5/5) | 0.441 [0.348, 0.533] (5/5) | 0.263 [0.250, 0.288] (5/5) | 5.500 [5.500, 5.500] (5/5) | 1.082 [1.072, 1.101] (5/5) |
| no-live | tool | 0.548 [0.447, 0.587] (5/5) | 0.548 [0.447, 0.587] (5/5) | 0.295 [0.286, 0.317] (5/5) | 5.033 [5.033, 5.033] (5/5) | 0.936 [0.894, 0.952] (5/5) |
| no-live | cancel | 0.382 [0.277, 0.850] (5/5) | 0.382 [0.277, 0.850] (5/5) | 0.262 [0.246, 0.300] (5/5) | 5.698 [5.698, 5.698] (5/5) | 0.942 [0.893, 0.947] (5/5) |
| live | short-small | 5.423 [4.006, 6.709] (5/5) | 6.537 [4.424, 7.748] (5/5) | 1.844 [1.485, 2.137] (5/5) | 6.799 [6.792, 6.807] (5/5) | 1.224 [1.174, 1.260] (5/5) |
| live | short-large | 6.696 [3.925, 7.818] (5/5) | 8.224 [4.303, 8.859] (5/5) | 4.013 [3.046, 4.222] (5/5) | 7.649 [7.633, 7.658] (5/5) | 1.318 [1.307, 1.429] (5/5) |
| live | stream | 0.963 [0.902, 0.986] (5/5) | 0.963 [0.902, 0.986] (5/5) | 0.264 [0.237, 0.337] (5/5) | 4.815 [4.727, 4.815] (5/5) | 0.870 [0.836, 1.046] (5/5) |
| live | large | 0.530 [0.420, 0.560] (5/5) | 0.530 [0.420, 0.560] (5/5) | 0.293 [0.267, 0.305] (5/5) | 5.500 [5.500, 5.500] (5/5) | 1.096 [1.068, 1.112] (5/5) |
| live | tool | 0.566 [0.464, 0.647] (5/5) | 0.566 [0.464, 0.647] (5/5) | 0.306 [0.302, 0.339] (5/5) | 5.033 [5.033, 5.033] (5/5) | 0.931 [0.904, 0.961] (5/5) |
| live | cancel | 0.385 [0.315, 0.447] (5/5) | 0.385 [0.315, 0.447] (5/5) | 0.279 [0.258, 0.300] (5/5) | 5.698 [5.698, 5.698] (5/5) | 0.957 [0.931, 0.989] (5/5) |

## Warm：四个 case/mode 的绝对每turn成本

`short-small`先运行8个历史turn，`short-large`先运行64个历史turn，之后均测量32个turn。wall是每个产品32个turn延迟的中位数，再取五产品中位数；CPU和写入是测量区间计数差/32，再取五产品中位数。/proc CPU计数以100 ticks/s换算，单个累计端点粒度10ms；这不是100Hz profiler采样，真实计数零会保留。

Direct批次：每格为上轮 joint → 最终候选；不把该批候选与 fresh Pi 批次混池。

| 模式 | case | 历史turn | wall中位 ms/turn | measured self CPU ms/turn | measured write bytes/turn |
| --- | --- | --- | --- | --- | --- |
| no-live | short-small | 8 | 25.195 → 21.255 | 29.062 → 23.125 | 1,671,808.0 → 1,571,328.0 |
| no-live | short-large | 64 | 25.620 → 23.161 | 28.438 → 25.625 | 2,023,424.0 → 1,887,232.0 |
| live | short-small | 8 | 33.302 → 26.675 | 44.375 → 36.875 | 1,668,352.0 → 1,569,408.0 |
| live | short-large | 64 | 36.806 → 24.296 | 50.625 → 33.750 | 2,024,448.0 → 1,895,168.0 |

Fresh Pi批次：每格为 Pi → 最终 rustX。

| 模式 | case | 历史turn | wall中位 ms/turn | measured self CPU ms/turn | measured write bytes/turn |
| --- | --- | --- | --- | --- | --- |
| no-live | short-small | 8 | 3.325 → 22.262 | 7.500 → 23.125 | 224,512.0 → 1,571,584.0 |
| no-live | short-large | 64 | 3.271 → 17.007 | 6.562 → 20.000 | 224,384.0 → 1,893,376.0 |
| live | short-small | 8 | 3.295 → 21.457 | 7.500 → 29.688 | 224,512.0 → 1,572,224.0 |
| live | short-large | 64 | 3.563 → 29.114 | 6.875 → 37.500 | 224,384.0 → 1,891,968.0 |

## Native三引擎：独立参考的绝对测量成本

此批仅live，四个case × 三引擎 × 三次。均值wall先取每产品turn均值，再取三产品中位数；CPU/写入也取每产品每turn值的三产品中位数，直接读取独立三引擎审计 `groups`。中位wall先取每产品turn中位数，再取三产品中位数，来自已审计 `supplementary_table`，均不复算原始数据。方括号为三次产品值的[min,max]，不是混池总体算术平均。DeepSeek的SDK idle不等待最终JSONL flush/fsync，不能视为durable settlement等价，也不计算跨引擎等持久性比值。

| case | 引擎 | 均值wall ms/turn [min,max] | 中位wall ms/turn | measured CPU ms/turn [min,max] | measured write bytes/turn [min,max] |
| --- | --- | --- | --- | --- | --- |
| short-small | rust | 27.241 [15.515, 27.571] (3/3) | 27.291 | 37.500 [21.562, 37.500] (3/3) | 1,571,968.0 [1,571,968.0, 1,572,224.0] (3/3) |
| short-small | pi | 3.906 [3.712, 3.917] (3/3) | 3.291 | 7.500 [7.188, 7.500] (3/3) | 224,512.0 [224,512.0, 224,512.0] (3/3) |
| short-small | deepseek | 15.125 [15.026, 15.693] (3/3) | 14.725 | 17.500 [17.188, 18.125] (3/3) | 21,504.0 [21,504.0, 21,504.0] (3/3) |
| short-large | rust | 26.400 [18.056, 27.946] (3/3) | 27.022 | 35.625 [25.000, 38.125] (3/3) | 1,894,656.0 [1,885,824.0, 1,901,056.0] (3/3) |
| short-large | pi | 4.024 [3.966, 4.034] (3/3) | 3.570 | 6.562 [6.562, 6.875] (3/3) | 224,384.0 [224,384.0, 224,384.0] (3/3) |
| short-large | deepseek | 14.691 [14.570, 14.793] (3/3) | 14.371 | 16.562 [16.250, 16.875] (3/3) | 21,376.0 [21,376.0, 21,376.0] (3/3) |
| stream | rust | 1,591.536 [1,591.073, 1,603.925] (3/3) | 1,591.536 | 160.000 [160.000, 180.000] (3/3) | 1,863,680.0 [1,859,584.0, 1,863,680.0] (3/3) |
| stream | pi | 1,754.835 [1,674.377, 1,777.421] (3/3) | 1,754.835 | 660.000 [560.000, 690.000] (3/3) | 516,096.0 [516,096.0, 536,576.0] (3/3) |
| stream | deepseek | 1,603.692 [1,564.931, 1,607.989] (3/3) | 1,603.692 | 320.000 [290.000, 320.000] (3/3) | 28,672.0 [28,672.0, 28,672.0] (3/3) |
| large | rust | 42.421 [36.961, 48.838] (3/3) | 42.421 | 40.000 [40.000, 60.000] (3/3) | 1,449,984.0 [1,449,984.0, 1,449,984.0] (3/3) |
| large | pi | 88.634 [87.802, 93.322] (3/3) | 88.634 | 120.000 [120.000, 120.000] (3/3) | 294,912.0 [294,912.0, 294,912.0] (3/3) |
| large | deepseek | 172.507 [165.504, 174.950] (3/3) | 172.507 | 200.000 [190.000, 210.000] (3/3) | 32,768.0 [32,768.0, 32,768.0] (3/3) |

Rust/Pi这里的CPU为self CPU，包含同进程benchmark driver；DeepSeek为采样遍历中存活进程及已等待子进程的CPU。DeepSeek写入不涵盖已退出子进程，均非设备I/O；不能据此做等生命周期整体成本比值。

### DeepSeek关闭与持久化边界单列

下面CPU为首次native runtime与复验native runtime退出时wait4计数，包含完整退出过程，但不包含外部driver/provider CPU；不得用它代替上表的测量区间CPU。当前证据没有独立flush耗时计数，也没有退出后的proc write_bytes端点：完整写入量与flush/fsync延迟均记不可用，不记零。可读close/resume证据不能证明断电持久性或fsync成功；SDK disposal中的allSettled可能吞掉失败。

| case | 首次runtime完整CPU ms | 复验runtime完整CPU ms | 两runtime完整CPU ms | 完整write bytes | 独立flush/fsync ms |
| --- | --- | --- | --- | --- | --- |
| short-small | 2,331.652 | 1,627.242 | 3,958.894 | 不可用 | 不可用 |
| short-large | 3,249.220 | 1,647.994 | 4,897.214 | 不可用 | 不可用 |
| stream | 1,730.310 | 1,609.642 | 3,335.238 | 不可用 | 不可用 |
| large | 1,631.316 | 1,623.013 | 3,239.034 | 不可用 | 不可用 |

## 全量不利项：不隐藏中位回退或单对回退

以下扫描direct与fresh Pi审计中的所有指标，不只筛选主表指标。每个列出的cell至少一个逐对差值>0；“中位较高”表示配对百分比中位数>0，“仅部分配对较高”保留即使中位数改善也出现的不利样本。这里描述数值，不把更多功能性存储或任何差值自动解释为缺陷。无可用比值但存在正差值的cell仍列出。

| 比较 | 模式 | case | 审计metric | 分类 | 配对 Δ% median [min,max] | 较高对数 | 五对均较高 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| direct | no-live | short-small | max_sampled_anon_pss_mib | 仅部分配对较高 | -0.62% [-1.63%, +0.09%] (5/5) | 1/5 | 否 |
| direct | no-live | short-small | max_sampled_pss_mib | 仅部分配对较高 | -0.65% [-1.87%, +0.17%] (5/5) | 1/5 | 否 |
| direct | no-live | short-small | max_sampled_rss_mib | 仅部分配对较高 | -0.63% [-1.78%, +0.08%] (5/5) | 1/5 | 否 |
| direct | no-live | short-large | measured_turn_cpu_ms | 仅部分配对较高 | -20.00% [-28.21%, +1.23%] (5/5) | 1/5 | 否 |
| direct | no-live | short-large | median_measured_turn_ms | 仅部分配对较高 | -24.36% [-29.86%, +6.37%] (5/5) | 1/5 | 否 |
| direct | no-live | short-large | turn_cpu_ms_per_turn | 仅部分配对较高 | -20.00% [-28.21%, +1.23%] (5/5) | 1/5 | 否 |
| direct | no-live | stream | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -0.15% [-1.56%, +2.59%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | lifecycle_ms | 仅部分配对较高 | -0.41% [-1.46%, +2.42%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | lifecycle_self_cpu_ms | 仅部分配对较高 | -3.12% [-13.79%, +14.81%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | max_sampled_anon_pss_mib | 仅部分配对较高 | -0.94% [-1.91%, +0.56%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | max_sampled_pss_mib | 仅部分配对较高 | -0.00% [-1.81%, +0.18%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | max_sampled_rss_mib | 仅部分配对较高 | -0.00% [-1.70%, +0.16%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.36% [-1.54%, +0.59%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | mean_turn_ms | 仅部分配对较高 | -0.54% [-1.87%, +2.38%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | measured_turn_cpu_ms | 仅部分配对较高 | -6.25% [-22.22%, +28.57%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | median_measured_turn_ms | 仅部分配对较高 | -0.54% [-1.87%, +2.38%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | through_cleanup_self_cpu_ms | 仅部分配对较高 | -3.12% [-13.79%, +14.81%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | through_product_exit_wall_ms | 仅部分配对较高 | -0.32% [-1.43%, +2.58%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | turn_cpu_ms_per_turn | 仅部分配对较高 | -6.25% [-22.22%, +28.57%] (5/5) | 2/5 | 否 |
| direct | no-live | large | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -1.25% [-6.05%, +6.96%] (5/5) | 1/5 | 否 |
| direct | no-live | large | lifecycle_ms | 仅部分配对较高 | -2.22% [-6.63%, +3.74%] (5/5) | 2/5 | 否 |
| direct | no-live | large | lifecycle_self_cpu_ms | 仅部分配对较高 | -5.56% [-16.67%, +12.50%] (5/5) | 2/5 | 否 |
| direct | no-live | large | max_sampled_anon_pss_mib | 仅部分配对较高 | -0.78% [-1.24%, +0.50%] (5/5) | 1/5 | 否 |
| direct | no-live | large | max_sampled_pss_mib | 仅部分配对较高 | -0.27% [-0.65%, +0.58%] (5/5) | 1/5 | 否 |
| direct | no-live | large | max_sampled_rss_mib | 仅部分配对较高 | -0.33% [-0.69%, +0.62%] (5/5) | 1/5 | 否 |
| direct | no-live | large | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.22% [-0.66%, +0.26%] (5/5) | 1/5 | 否 |
| direct | no-live | large | measured_turn_cpu_ms | 仅部分配对较高 | -16.67% [-33.33%, +25.00%] (5/5) | 1/5 | 否 |
| direct | no-live | large | through_cleanup_self_cpu_ms | 仅部分配对较高 | -5.56% [-16.67%, +12.50%] (5/5) | 2/5 | 否 |
| direct | no-live | large | through_product_exit_wall_ms | 仅部分配对较高 | -1.76% [-5.39%, +3.12%] (5/5) | 2/5 | 否 |
| direct | no-live | large | turn_cpu_ms_per_turn | 仅部分配对较高 | -16.67% [-33.33%, +25.00%] (5/5) | 1/5 | 否 |
| direct | no-live | tool | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -7.43% [-11.88%, +14.40%] (5/5) | 1/5 | 否 |
| direct | no-live | tool | lifecycle_ms | 仅部分配对较高 | -7.86% [-12.65%, +9.00%] (5/5) | 1/5 | 否 |
| direct | no-live | tool | lifecycle_self_cpu_ms | 仅部分配对较高 | -14.29% [-19.05%, +17.65%] (5/5) | 1/5 | 否 |
| direct | no-live | tool | max_sampled_anon_pss_mib | 仅部分配对较高 | -0.80% [-2.39%, +0.18%] (5/5) | 1/5 | 否 |
| direct | no-live | tool | max_sampled_pss_mib | 仅部分配对较高 | -0.59% [-1.71%, +0.20%] (5/5) | 1/5 | 否 |
| direct | no-live | tool | max_sampled_rss_mib | 仅部分配对较高 | -0.57% [-1.62%, +0.20%] (5/5) | 1/5 | 否 |
| direct | no-live | tool | through_cleanup_self_cpu_ms | 仅部分配对较高 | -14.29% [-19.05%, +17.65%] (5/5) | 1/5 | 否 |
| direct | no-live | tool | through_product_exit_wall_ms | 仅部分配对较高 | -7.68% [-12.20%, +11.97%] (5/5) | 1/5 | 否 |
| direct | no-live | cancel | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -0.97% [-8.49%, +9.25%] (5/5) | 2/5 | 否 |
| direct | no-live | cancel | lifecycle_ms | 仅部分配对较高 | -1.92% [-7.74%, +4.94%] (5/5) | 2/5 | 否 |
| direct | no-live | cancel | lifecycle_self_cpu_ms | 仅部分配对较高 | +0.00% [-11.76%, +7.14%] (5/5) | 1/5 | 否 |
| direct | no-live | cancel | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.61% [-0.99%, +0.37%] (5/5) | 1/5 | 否 |
| direct | no-live | cancel | through_cleanup_self_cpu_ms | 仅部分配对较高 | +0.00% [-11.76%, +7.14%] (5/5) | 1/5 | 否 |
| direct | no-live | cancel | through_product_exit_wall_ms | 仅部分配对较高 | -2.15% [-8.91%, +6.79%] (5/5) | 2/5 | 否 |
| direct | live | short-small | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -7.62% [-11.56%, +6.79%] (5/5) | 2/5 | 否 |
| direct | live | short-small | lifecycle_ms | 仅部分配对较高 | -8.02% [-12.62%, +5.55%] (5/5) | 2/5 | 否 |
| direct | live | short-small | lifecycle_self_cpu_ms | 仅部分配对较高 | -10.21% [-15.51%, +4.74%] (5/5) | 2/5 | 否 |
| direct | live | short-small | mean_turn_ms | 仅部分配对较高 | -17.52% [-26.69%, +14.36%] (5/5) | 2/5 | 否 |
| direct | live | short-small | measured_turn_cpu_ms | 仅部分配对较高 | -15.17% [-26.06%, +12.93%] (5/5) | 2/5 | 否 |
| direct | live | short-small | median_measured_turn_ms | 仅部分配对较高 | -17.94% [-27.69%, +21.85%] (5/5) | 2/5 | 否 |
| direct | live | short-small | through_cleanup_self_cpu_ms | 仅部分配对较高 | -10.21% [-15.51%, +4.74%] (5/5) | 2/5 | 否 |
| direct | live | short-small | through_product_exit_wall_ms | 仅部分配对较高 | -7.48% [-12.09%, +5.51%] (5/5) | 2/5 | 否 |
| direct | live | short-small | turn_cpu_ms_per_turn | 仅部分配对较高 | -15.17% [-26.06%, +12.93%] (5/5) | 2/5 | 否 |
| direct | live | short-large | max_sampled_anon_pss_mib | 中位较高 | +0.12% [-0.48%, +0.36%] (5/5) | 3/5 | 否 |
| direct | live | short-large | max_sampled_pss_mib | 中位较高 | +0.03% [-0.43%, +0.45%] (5/5) | 3/5 | 否 |
| direct | live | short-large | max_sampled_rss_mib | 仅部分配对较高 | -0.01% [-0.43%, +0.43%] (5/5) | 2/5 | 否 |
| direct | live | short-large | max_sampled_turn_rss_mib | 中位较高 | +0.17% [-0.78%, +0.66%] (5/5) | 3/5 | 否 |
| direct | live | stream | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -0.84% [-4.29%, +1.53%] (5/5) | 1/5 | 否 |
| direct | live | stream | lifecycle_ms | 仅部分配对较高 | -0.68% [-4.03%, +1.60%] (5/5) | 1/5 | 否 |
| direct | live | stream | lifecycle_self_cpu_ms | 仅部分配对较高 | -6.67% [-14.71%, +6.67%] (5/5) | 2/5 | 否 |
| direct | live | stream | max_sampled_anon_pss_mib | 仅部分配对较高 | -0.90% [-2.44%, +0.12%] (5/5) | 1/5 | 否 |
| direct | live | stream | max_sampled_pss_mib | 仅部分配对较高 | -0.74% [-1.31%, +0.34%] (5/5) | 2/5 | 否 |
| direct | live | stream | max_sampled_rss_mib | 仅部分配对较高 | -0.73% [-1.28%, +0.41%] (5/5) | 2/5 | 否 |
| direct | live | stream | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.41% [-1.39%, +0.56%] (5/5) | 1/5 | 否 |
| direct | live | stream | mean_turn_ms | 仅部分配对较高 | -1.50% [-2.60%, +0.30%] (5/5) | 2/5 | 否 |
| direct | live | stream | measured_turn_cpu_ms | 仅部分配对较高 | -11.11% [-29.41%, +5.56%] (5/5) | 1/5 | 否 |
| direct | live | stream | median_measured_turn_ms | 仅部分配对较高 | -1.50% [-2.60%, +0.30%] (5/5) | 2/5 | 否 |
| direct | live | stream | through_cleanup_self_cpu_ms | 仅部分配对较高 | -6.67% [-14.71%, +6.67%] (5/5) | 2/5 | 否 |
| direct | live | stream | through_product_exit_wall_ms | 仅部分配对较高 | -0.53% [-4.28%, +1.70%] (5/5) | 1/5 | 否 |
| direct | live | stream | turn_cpu_ms_per_turn | 仅部分配对较高 | -11.11% [-29.41%, +5.56%] (5/5) | 1/5 | 否 |
| direct | live | large | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -5.65% [-12.08%, +9.90%] (5/5) | 2/5 | 否 |
| direct | live | large | lifecycle_ms | 仅部分配对较高 | -4.44% [-12.16%, +3.17%] (5/5) | 2/5 | 否 |
| direct | live | large | lifecycle_self_cpu_ms | 仅部分配对较高 | -11.11% [-26.32%, +11.11%] (5/5) | 2/5 | 否 |
| direct | live | large | max_sampled_anon_pss_mib | 仅部分配对较高 | -0.65% [-1.16%, +0.41%] (5/5) | 1/5 | 否 |
| direct | live | large | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.73% [-0.78%, +0.16%] (5/5) | 1/5 | 否 |
| direct | live | large | mean_turn_ms | 仅部分配对较高 | -0.78% [-42.13%, +31.18%] (5/5) | 2/5 | 否 |
| direct | live | large | measured_turn_cpu_ms | 仅部分配对较高 | +0.00% [-37.50%, +20.00%] (5/5) | 2/5 | 否 |
| direct | live | large | median_measured_turn_ms | 仅部分配对较高 | -0.78% [-42.13%, +31.18%] (5/5) | 2/5 | 否 |
| direct | live | large | through_cleanup_self_cpu_ms | 仅部分配对较高 | -11.11% [-26.32%, +11.11%] (5/5) | 2/5 | 否 |
| direct | live | large | through_product_exit_wall_ms | 仅部分配对较高 | -5.52% [-10.98%, +6.01%] (5/5) | 2/5 | 否 |
| direct | live | large | turn_cpu_ms_per_turn | 仅部分配对较高 | +0.00% [-37.50%, +20.00%] (5/5) | 2/5 | 否 |
| direct | live | tool | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -2.48% [-3.58%, +5.07%] (5/5) | 2/5 | 否 |
| direct | live | tool | lifecycle_ms | 仅部分配对较高 | -2.23% [-3.89%, +3.87%] (5/5) | 2/5 | 否 |
| direct | live | tool | lifecycle_self_cpu_ms | 仅部分配对较高 | +0.00% [-13.64%, +5.56%] (5/5) | 1/5 | 否 |
| direct | live | tool | max_sampled_anon_pss_mib | 中位较高 | +0.33% [-1.53%, +0.92%] (5/5) | 3/5 | 否 |
| direct | live | tool | max_sampled_pss_mib | 中位较高 | +0.03% [-0.97%, +0.35%] (5/5) | 3/5 | 否 |
| direct | live | tool | max_sampled_rss_mib | 中位较高 | +0.05% [-0.96%, +0.34%] (5/5) | 3/5 | 否 |
| direct | live | tool | max_sampled_turn_rss_mib | 中位较高 | +0.16% [-0.95%, +0.78%] (5/5) | 3/5 | 否 |
| direct | live | tool | measured_turn_cpu_ms | 仅部分配对较高 | +0.00% [-25.00%, +14.29%] (5/5) | 1/5 | 否 |
| direct | live | tool | through_cleanup_self_cpu_ms | 仅部分配对较高 | +0.00% [-13.64%, +5.56%] (5/5) | 1/5 | 否 |
| direct | live | tool | through_product_exit_wall_ms | 仅部分配对较高 | -2.26% [-3.38%, +4.89%] (5/5) | 2/5 | 否 |
| direct | live | tool | turn_cpu_ms_per_turn | 仅部分配对较高 | +0.00% [-25.00%, +14.29%] (5/5) | 1/5 | 否 |
| direct | live | cancel | complete_harness_lifecycle_wall_ms | 中位较高 | +4.23% [-1.18%, +13.71%] (5/5) | 3/5 | 否 |
| direct | live | cancel | lifecycle_ms | 中位较高 | +2.31% [-0.76%, +14.03%] (5/5) | 4/5 | 否 |
| direct | live | cancel | lifecycle_self_cpu_ms | 仅部分配对较高 | -5.88% [-6.67%, +12.50%] (5/5) | 1/5 | 否 |
| direct | live | cancel | max_sampled_anon_pss_mib | 仅部分配对较高 | -0.54% [-2.35%, +0.12%] (5/5) | 1/5 | 否 |
| direct | live | cancel | max_sampled_pss_mib | 仅部分配对较高 | -0.67% [-1.73%, +0.29%] (5/5) | 2/5 | 否 |
| direct | live | cancel | max_sampled_rss_mib | 仅部分配对较高 | -0.73% [-1.77%, +0.22%] (5/5) | 2/5 | 否 |
| direct | live | cancel | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.20% [-2.19%, +0.15%] (5/5) | 1/5 | 否 |
| direct | live | cancel | mean_turn_ms | 仅部分配对较高 | -0.82% [-5.92%, +14.23%] (5/5) | 2/5 | 否 |
| direct | live | cancel | measured_turn_cpu_ms | 仅部分配对较高 | +0.00% [+0.00%, +25.00%] (5/5) | 1/5 | 否 |
| direct | live | cancel | median_measured_turn_ms | 仅部分配对较高 | -0.82% [-5.92%, +14.23%] (5/5) | 2/5 | 否 |
| direct | live | cancel | through_cleanup_self_cpu_ms | 仅部分配对较高 | -5.88% [-6.67%, +12.50%] (5/5) | 1/5 | 否 |
| direct | live | cancel | through_product_exit_wall_ms | 中位较高 | +4.48% [-0.52%, +14.77%] (5/5) | 3/5 | 否 |
| direct | live | cancel | turn_cpu_ms_per_turn | 仅部分配对较高 | +0.00% [+0.00%, +25.00%] (5/5) | 1/5 | 否 |
| fresh Pi | no-live | short-small | closed_allocated_bytes | 中位较高 | +4797.62% [+4797.62%, +4800.00%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | closed_logical_bytes | 中位较高 | +4788.20% [+4788.20%, +4790.58%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | complete_harness_lifecycle_wall_ms | 中位较高 | +125.48% [+118.35%, +170.78%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | lifecycle_ms | 中位较高 | +246.94% [+230.93%, +277.70%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | lifecycle_self_cpu_ms | 中位较高 | +190.20% [+161.02%, +203.92%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | max_sampled_anon_pss_mib | 中位较高 | +12.89% [+10.90%, +19.43%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | max_sampled_pss_mib | 中位较高 | +46.49% [+44.07%, +53.43%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | max_sampled_rss_mib | 中位较高 | +6.89% [+5.57%, +10.64%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | mean_turn_ms | 中位较高 | +467.84% [+268.47%, +531.76%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | measured_turn_cpu_ms | 中位较高 | +204.17% [+184.62%, +243.48%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | measured_turn_write_bytes | 中位较高 | +600.00% [+597.95%, +601.37%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | median_measured_turn_ms | 中位较高 | +590.15% [+510.73%, +627.28%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | through_cleanup_process_write_bytes | 中位较高 | +579.71% [+578.08%, +580.77%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | through_cleanup_process_write_mib | 中位较高 | +579.71% [+578.08%, +580.77%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | through_cleanup_self_cpu_ms | 中位较高 | +57.45% [+52.48%, +66.67%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | through_product_exit_wall_ms | 中位较高 | +123.88% [+116.86%, +174.81%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | turn_cpu_ms_per_turn | 中位较高 | +204.17% [+184.62%, +243.48%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | turn_write_bytes_per_turn | 中位较高 | +600.00% [+597.95%, +601.37%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | closed_allocated_bytes | 中位较高 | +5240.00% [+5190.00%, +5255.00%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | closed_logical_bytes | 中位较高 | +5232.21% [+5181.91%, +5247.29%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | complete_harness_lifecycle_wall_ms | 中位较高 | +289.89% [+279.03%, +473.24%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | lifecycle_ms | 中位较高 | +417.53% [+410.00%, +694.72%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | lifecycle_self_cpu_ms | 中位较高 | +347.92% [+338.95%, +367.74%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | max_sampled_anon_pss_mib | 中位较高 | +28.70% [+27.35%, +38.37%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | max_sampled_pss_mib | 中位较高 | +51.16% [+49.70%, +61.90%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | max_sampled_rss_mib | 中位较高 | +19.66% [+18.97%, +26.40%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | max_sampled_turn_rss_mib | 仅部分配对较高 | -6.08% [-7.28%, +0.55%] (5/5) | 1/5 | 否 |
| fresh Pi | no-live | short-large | mean_turn_ms | 中位较高 | +488.82% [+318.90%, +573.79%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | measured_turn_cpu_ms | 中位较高 | +204.76% [+175.00%, +342.11%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | measured_turn_write_bytes | 中位较高 | +743.81% [+740.84%, +745.52%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | median_measured_turn_ms | 中位较高 | +420.01% [+351.22%, +651.29%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | through_cleanup_process_write_bytes | 中位较高 | +664.92% [+663.53%, +665.01%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | through_cleanup_process_write_mib | 中位较高 | +664.92% [+663.53%, +665.01%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | through_cleanup_self_cpu_ms | 中位较高 | +212.06% [+202.17%, +222.22%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | through_product_exit_wall_ms | 中位较高 | +299.86% [+288.05%, +497.91%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | turn_cpu_ms_per_turn | 中位较高 | +204.76% [+175.00%, +342.11%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | turn_write_bytes_per_turn | 中位较高 | +743.81% [+740.84%, +745.52%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | closed_allocated_bytes | 中位较高 | +1931.25% [+1814.71%, +1934.38%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | closed_logical_bytes | 中位较高 | +1920.46% [+1804.57%, +1923.60%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | lifecycle_ms | 仅部分配对较高 | -4.47% [-5.52%, +0.03%] (5/5) | 1/5 | 否 |
| fresh Pi | no-live | stream | max_sampled_pss_mib | 中位较高 | +24.22% [+21.87%, +29.61%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | measured_turn_write_bytes | 中位较高 | +260.32% [+237.04%, +261.11%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | through_cleanup_process_write_bytes | 中位较高 | +371.82% [+350.22%, +372.73%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | through_cleanup_process_write_mib | 中位较高 | +371.82% [+350.22%, +372.73%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | turn_write_bytes_per_turn | 中位较高 | +260.32% [+237.04%, +261.11%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | closed_allocated_bytes | 中位较高 | +1292.11% [+1292.11%, +1292.11%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | closed_logical_bytes | 中位较高 | +1283.80% [+1283.80%, +1283.80%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | lifecycle_ms | 中位较高 | +20.21% [+17.23%, +31.37%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | lifecycle_self_cpu_ms | 仅部分配对较高 | +0.00% [-6.25%, +13.33%] (5/5) | 2/5 | 否 |
| fresh Pi | no-live | large | max_sampled_anon_pss_mib | 中位较高 | +8.23% [+7.21%, +10.13%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | max_sampled_pss_mib | 中位较高 | +52.15% [+51.39%, +53.98%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | max_sampled_rss_mib | 中位较高 | +2.80% [+2.29%, +3.65%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | max_sampled_turn_rss_mib | 中位较高 | +2.96% [+2.63%, +3.70%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | measured_turn_write_bytes | 中位较高 | +391.67% [+391.67%, +391.67%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | through_cleanup_process_write_bytes | 中位较高 | +450.00% [+450.00%, +450.00%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | through_cleanup_process_write_mib | 中位较高 | +450.00% [+450.00%, +450.00%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | turn_write_bytes_per_turn | 中位较高 | +391.67% [+391.67%, +391.67%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | closed_allocated_bytes | 中位较高 | +2390.62% [+2390.62%, +2390.62%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | closed_logical_bytes | 中位较高 | +2379.38% [+2379.38%, +2379.38%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | lifecycle_ms | 中位较高 | +23.75% [+20.76%, +34.54%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | lifecycle_self_cpu_ms | 仅部分配对较高 | +0.00% [-5.26%, +5.88%] (5/5) | 2/5 | 否 |
| fresh Pi | no-live | tool | max_sampled_pss_mib | 中位较高 | +33.66% [+29.12%, +35.92%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | max_sampled_turn_rss_mib | 中位较高 | +2.39% [+1.75%, +3.53%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | measured_turn_write_bytes | 中位较高 | +311.64% [+311.64%, +311.64%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | through_cleanup_process_write_bytes | 中位较高 | +403.33% [+403.33%, +403.33%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | through_cleanup_process_write_mib | 中位较高 | +403.33% [+403.33%, +403.33%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | turn_write_bytes_per_turn | 中位较高 | +311.64% [+311.64%, +311.64%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | closed_allocated_bytes | 中位较高 | +1640.00% [+1640.00%, +1640.00%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | closed_logical_bytes | 中位较高 | +1629.34% [+1629.34%, +1629.34%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | lifecycle_ms | 中位较高 | +15.70% [+11.64%, +112.23%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | lifecycle_self_cpu_ms | 仅部分配对较高 | -11.11% [-16.67%, +5.88%] (5/5) | 1/5 | 否 |
| fresh Pi | no-live | cancel | max_sampled_pss_mib | 中位较高 | +34.35% [+28.77%, +35.33%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | max_sampled_turn_rss_mib | 中位较高 | +1.05% [+0.43%, +2.52%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | measured_turn_write_bytes | 中位较高 | +386.57% [+386.57%, +386.57%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | through_cleanup_process_write_bytes | 中位较高 | +469.81% [+469.81%, +469.81%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | through_cleanup_process_write_mib | 中位较高 | +469.81% [+469.81%, +469.81%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | turn_write_bytes_per_turn | 中位较高 | +386.57% [+386.57%, +386.57%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | closed_allocated_bytes | 中位较高 | +4800.00% [+4797.62%, +4800.00%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | closed_logical_bytes | 中位较高 | +4790.58% [+4788.20%, +4790.58%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | complete_harness_lifecycle_wall_ms | 中位较高 | +122.43% [+87.26%, +151.53%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | lifecycle_ms | 中位较高 | +247.98% [+183.31%, +294.92%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | lifecycle_self_cpu_ms | 中位较高 | +233.96% [+161.82%, +283.02%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | max_sampled_anon_pss_mib | 中位较高 | +22.44% [+17.36%, +26.03%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | max_sampled_pss_mib | 中位较高 | +55.29% [+49.42%, +59.78%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | max_sampled_rss_mib | 中位较高 | +12.89% [+9.82%, +15.13%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | max_sampled_turn_rss_mib | 中位较高 | +2.58% [+0.19%, +3.64%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | mean_turn_ms | 中位较高 | +442.28% [+300.56%, +570.95%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | measured_turn_cpu_ms | 中位较高 | +295.83% [+187.50%, +360.00%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | measured_turn_write_bytes | 中位较高 | +600.29% [+599.32%, +601.25%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | median_measured_turn_ms | 中位较高 | +553.68% [+342.42%, +674.77%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | through_cleanup_process_write_bytes | 中位较高 | +579.93% [+579.23%, +580.72%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | through_cleanup_process_write_mib | 中位较高 | +579.93% [+579.23%, +580.72%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | through_cleanup_self_cpu_ms | 中位较高 | +84.38% [+48.45%, +113.68%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | through_product_exit_wall_ms | 中位较高 | +122.89% [+87.16%, +155.86%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | turn_cpu_ms_per_turn | 中位较高 | +295.83% [+187.50%, +360.00%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | turn_write_bytes_per_turn | 中位较高 | +600.29% [+599.32%, +601.25%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | closed_allocated_bytes | 中位较高 | +5248.33% [+5245.00%, +5353.33%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | closed_logical_bytes | 中位较高 | +5240.58% [+5237.23%, +5345.56%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | complete_harness_lifecycle_wall_ms | 中位较高 | +336.91% [+248.09%, +357.59%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | lifecycle_ms | 中位较高 | +486.50% [+353.08%, +513.43%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | lifecycle_self_cpu_ms | 中位较高 | +464.15% [+320.91%, +501.98%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | max_sampled_anon_pss_mib | 中位较高 | +31.79% [+30.74%, +42.85%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | max_sampled_pss_mib | 中位较高 | +53.91% [+52.93%, +65.88%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | max_sampled_rss_mib | 中位较高 | +22.35% [+21.51%, +29.76%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | max_sampled_turn_rss_mib | 仅部分配对较高 | -5.20% [-5.67%, +1.67%] (5/5) | 1/5 | 否 |
| fresh Pi | live | short-large | mean_turn_ms | 中位较高 | +569.61% [+292.55%, +681.80%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | measured_turn_cpu_ms | 中位较高 | +445.45% [+200.00%, +540.91%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | measured_turn_write_bytes | 中位较高 | +743.18% [+742.10%, +762.98%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | median_measured_turn_ms | 中位较高 | +722.43% [+330.35%, +785.93%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | through_cleanup_process_write_bytes | 中位较高 | +664.86% [+663.32%, +665.76%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | through_cleanup_process_write_mib | 中位较高 | +664.86% [+663.32%, +665.76%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | through_cleanup_self_cpu_ms | 中位较高 | +301.34% [+204.61%, +322.22%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | through_product_exit_wall_ms | 中位较高 | +350.42% [+255.95%, +371.99%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | turn_cpu_ms_per_turn | 中位较高 | +445.45% [+200.00%, +540.91%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | turn_write_bytes_per_turn | 中位较高 | +743.18% [+742.10%, +762.98%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | closed_allocated_bytes | 中位较高 | +1934.38% [+1934.38%, +1934.38%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | closed_logical_bytes | 中位较高 | +1923.60% [+1923.60%, +1923.60%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | lifecycle_ms | 中位较高 | +0.86% [-5.07%, +4.86%] (5/5) | 3/5 | 否 |
| fresh Pi | live | stream | max_sampled_anon_pss_mib | 仅部分配对较高 | -12.99% [-16.37%, +4.60%] (5/5) | 1/5 | 否 |
| fresh Pi | live | stream | max_sampled_pss_mib | 中位较高 | +24.92% [+21.27%, +47.60%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | max_sampled_rss_mib | 仅部分配对较高 | -9.95% [-12.22%, +0.86%] (5/5) | 1/5 | 否 |
| fresh Pi | live | stream | measured_turn_write_bytes | 中位较高 | +272.95% [+261.11%, +272.95%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | through_cleanup_process_write_bytes | 中位较高 | +381.48% [+372.73%, +381.48%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | through_cleanup_process_write_mib | 中位较高 | +381.48% [+372.73%, +381.48%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | turn_write_bytes_per_turn | 中位较高 | +272.95% [+261.11%, +272.95%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | closed_allocated_bytes | 中位较高 | +1292.11% [+1292.11%, +1292.11%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | closed_logical_bytes | 中位较高 | +1283.80% [+1283.80%, +1283.80%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | lifecycle_ms | 中位较高 | +21.13% [+7.30%, +28.54%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | lifecycle_self_cpu_ms | 中位较高 | +12.50% [-5.88%, +13.33%] (5/5) | 4/5 | 否 |
| fresh Pi | live | large | max_sampled_anon_pss_mib | 中位较高 | +9.61% [+6.84%, +11.19%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | max_sampled_pss_mib | 中位较高 | +53.49% [+50.13%, +56.43%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | max_sampled_rss_mib | 中位较高 | +3.75% [+2.07%, +4.63%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | max_sampled_turn_rss_mib | 中位较高 | +3.64% [+2.40%, +4.22%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | measured_turn_write_bytes | 中位较高 | +391.67% [+391.67%, +391.67%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | through_cleanup_process_write_bytes | 中位较高 | +450.00% [+450.00%, +450.00%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | through_cleanup_process_write_mib | 中位较高 | +450.00% [+450.00%, +450.00%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | turn_write_bytes_per_turn | 中位较高 | +391.67% [+391.67%, +391.67%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | closed_allocated_bytes | 中位较高 | +2390.62% [+2390.62%, +2390.62%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | closed_logical_bytes | 中位较高 | +2379.38% [+2379.38%, +2379.38%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | lifecycle_ms | 中位较高 | +25.38% [+18.19%, +35.41%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | lifecycle_self_cpu_ms | 仅部分配对较高 | +0.00% [-5.00%, +10.53%] (5/5) | 2/5 | 否 |
| fresh Pi | live | tool | max_sampled_pss_mib | 中位较高 | +33.05% [+29.98%, +36.91%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | max_sampled_turn_rss_mib | 中位较高 | +3.92% [+3.01%, +5.19%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | measured_turn_write_bytes | 中位较高 | +311.64% [+311.64%, +311.64%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | through_cleanup_process_write_bytes | 中位较高 | +403.33% [+403.33%, +403.33%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | through_cleanup_process_write_mib | 中位较高 | +403.33% [+403.33%, +403.33%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | turn_write_bytes_per_turn | 中位较高 | +311.64% [+311.64%, +311.64%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | closed_allocated_bytes | 中位较高 | +1640.00% [+1640.00%, +1640.00%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | closed_logical_bytes | 中位较高 | +1629.34% [+1629.34%, +1629.34%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | lifecycle_ms | 中位较高 | +23.36% [+16.83%, +29.79%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | lifecycle_self_cpu_ms | 仅部分配对较高 | -5.56% [-5.88%, +5.88%] (5/5) | 1/5 | 否 |
| fresh Pi | live | cancel | max_sampled_pss_mib | 中位较高 | +36.63% [+33.22%, +41.13%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | max_sampled_turn_rss_mib | 中位较高 | +1.54% [+0.92%, +4.97%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | measured_turn_write_bytes | 中位较高 | +386.57% [+386.57%, +386.57%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | through_cleanup_process_write_bytes | 中位较高 | +469.81% [+469.81%, +469.81%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | through_cleanup_process_write_mib | 中位较高 | +469.81% [+469.81%, +469.81%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | turn_write_bytes_per_turn | 中位较高 | +386.57% [+386.57%, +386.57%] (5/5) | 5/5 | 是 |

### 不可用配对单列

缺失计数与零分母保持不可用；不会用零或相邻cell补齐。

direct与fresh Pi所有审计配对比值均可用。

未显示为独立时延指标的DeepSeek flush仍是未测量，而非零耗时。第一轮pass1、探索筛查、SQL/WAL采集及CPU profiler数据均不进入本文件正式样本的中位数或比值。
