# Pi-flow audited tables

来源：`complete-all-audit.json`，SHA256 `902e748b08ff085dc4d1edcd7a38eea181bde0b5542a8972bbd3e459472eb98f`；审计模式 `strict`。
候选 `d22ce66c5a1cda895cac382ce2c350f345933876` / binary `775b6426d1ce59e1f80e746000d52527e876917f117361841ce120b183d042bb`；基线 `f5cf22a8fbd2b1f0b3abc7fc2082a586f1126e20` / binary `8d033129f343660821b5fd7cef957a424e6d769c55948011f2798c6e2f3a0cb7`。
汇总脚本 SHA256 `a6e39c5f0a8198e6474eb33e8e7fbc19b693caaf7043b33e6790a2b37ec9ec16`。本文件只格式化已接受审计的派生字段，不重读原始计数、不合并探索样本、不重新选样。

正式范围：direct 120、fresh Pi 120、native reference 36 个产品；每个 direct/Pi cell 固定五对，三引擎每个 case/engine 固定三次。no-live/live × 六 case 全部展示。五对仅为描述统计，不满足原七对完整追平门槛；不作跨 case 或跨 mode 的整体追平结论。

构建边界：上轮基线 schema 79，本轮 Pi-flow schema 80。功能检查使用 release 配置、rustX package O0、all-features；性能产品另外 clean 后编译，使用普通 O3、default features，不能把功能检查二进制当作性能二进制。

持久性取舍：ready/direct 改为连接内 epoch；重新打开以已提交 J/T 为基线，不再持久保存旧进程是否完成安装或发布。这里采用 Pi 的 commit→adopt 模式；这不是所有旧损坏处理与恢复行为完全不变的承诺。共享连接的当前 epoch 仍保留安装门控、事务回滚与发布顺序。

## Direct：Pi-flow 候选 / 上轮 complete-costs

百分数为审计中五个逐对百分比的中位数；负值更低。wall均值使用 `mean_turn_ms`，wall中位使用每个产品的 `median_measured_turn_ms`，两者均非跨产品混池。完整CPU/进程写入是 through-cleanup 累计值，包含初始化、历史、验证与关闭；匿名PSS为所有已采样点的最大值。

| 模式 | case | wall均值 Δ% | wall中位 Δ% | 完整self CPU Δ% | 完整进程write bytes Δ% | 采样匿名PSS Δ% |
| --- | --- | --- | --- | --- | --- | --- |
| no-live | short-small | -12.05% (5/5) | -14.20% (5/5) | -8.55% (5/5) | -29.37% (5/5) | -0.62% (5/5) |
| no-live | short-large | -46.89% (5/5) | -17.34% (5/5) | -1.99% (5/5) | -30.86% (5/5) | -0.59% (5/5) |
| no-live | stream | -0.05% (5/5) | -0.05% (5/5) | +0.00% (5/5) | -15.00% (5/5) | -1.20% (5/5) |
| no-live | large | -3.45% (5/5) | -3.45% (5/5) | +0.00% (5/5) | -17.23% (5/5) | -0.19% (5/5) |
| no-live | tool | -13.16% (5/5) | -13.16% (5/5) | -5.26% (5/5) | -16.80% (5/5) | -1.13% (5/5) |
| no-live | cancel | +12.14% (5/5) | +12.14% (5/5) | -6.25% (5/5) | -16.11% (5/5) | -0.63% (5/5) |
| live | short-small | -17.14% (5/5) | -21.01% (5/5) | -8.25% (5/5) | -29.34% (5/5) | -1.52% (5/5) |
| live | short-large | -21.50% (5/5) | -22.95% (5/5) | -9.54% (5/5) | -30.83% (5/5) | -0.24% (5/5) |
| live | stream | -0.16% (5/5) | -0.16% (5/5) | -3.23% (5/5) | -15.00% (5/5) | -0.42% (5/5) |
| live | large | +4.13% (5/5) | +4.13% (5/5) | +5.56% (5/5) | -17.23% (5/5) | -0.72% (5/5) |
| live | tool | +14.71% (5/5) | +14.71% (5/5) | +5.56% (5/5) | -16.80% (5/5) | -1.03% (5/5) |
| live | cancel | -7.35% (5/5) | -7.35% (5/5) | -5.56% (5/5) | -16.11% (5/5) | +0.21% (5/5) |

## Fresh Pi：最终 rustX / Pi Durable

以下为逐对比值的中位数 [最小值, 最大值]；1×为相等，大于1×表示 rustX 较高。括号保留可用配对数。CPU/写入/PSS与 direct 表使用同一边界。

| 模式 | case | wall均值 × [min,max] | wall中位 × [min,max] | 完整self CPU × [min,max] | 完整write bytes × [min,max] | 采样匿名PSS × [min,max] |
| --- | --- | --- | --- | --- | --- | --- |
| no-live | short-small | 4.854 [2.507, 5.425] (5/5) | 5.698 [3.093, 6.272] (5/5) | 1.453 [1.098, 1.516] (5/5) | 4.798 [4.797, 4.799] (5/5) | 1.119 [1.106, 1.187] (5/5) |
| no-live | short-large | 5.090 [3.116, 11.889] (5/5) | 5.951 [3.266, 6.202] (5/5) | 2.877 [2.641, 3.131] (5/5) | 5.289 [5.288, 5.290] (5/5) | 1.279 [1.262, 1.302] (5/5) |
| no-live | stream | 0.934 [0.929, 0.967] (5/5) | 0.934 [0.929, 0.967] (5/5) | 0.265 [0.236, 0.306] (5/5) | 4.018 [4.009, 4.093] (5/5) | 0.846 [0.838, 0.848] (5/5) |
| no-live | large | 0.378 [0.344, 0.413] (5/5) | 0.378 [0.344, 0.413] (5/5) | 0.259 [0.250, 0.276] (5/5) | 4.552 [4.552, 4.552] (5/5) | 1.074 [1.062, 1.080] (5/5) |
| no-live | tool | 0.409 [0.385, 0.455] (5/5) | 0.409 [0.385, 0.455] (5/5) | 0.266 [0.254, 0.279] (5/5) | 4.188 [4.188, 4.188] (5/5) | 0.931 [0.909, 0.970] (5/5) |
| no-live | cancel | 0.317 [0.308, 0.345] (5/5) | 0.317 [0.308, 0.345] (5/5) | 0.258 [0.250, 0.262] (5/5) | 4.780 [4.780, 4.780] (5/5) | 0.953 [0.910, 0.983] (5/5) |
| live | short-small | 4.884 [3.932, 6.041] (5/5) | 5.788 [3.856, 6.376] (5/5) | 1.792 [1.564, 1.875] (5/5) | 4.798 [4.797, 4.799] (5/5) | 1.213 [1.155, 1.284] (5/5) |
| live | short-large | 5.332 [5.042, 6.070] (5/5) | 6.321 [5.899, 6.694] (5/5) | 3.405 [3.007, 3.609] (5/5) | 5.290 [5.287, 5.292] (5/5) | 1.316 [1.284, 1.328] (5/5) |
| live | stream | 0.929 [0.900, 0.944] (5/5) | 0.929 [0.900, 0.944] (5/5) | 0.257 [0.252, 0.290] (5/5) | 4.009 [3.894, 4.018] (5/5) | 0.853 [0.844, 0.860] (5/5) |
| live | large | 0.433 [0.326, 0.470] (5/5) | 0.433 [0.326, 0.470] (5/5) | 0.254 [0.246, 0.283] (5/5) | 4.552 [4.552, 4.552] (5/5) | 1.081 [1.047, 1.096] (5/5) |
| live | tool | 0.426 [0.397, 0.597] (5/5) | 0.426 [0.397, 0.597] (5/5) | 0.295 [0.254, 0.333] (5/5) | 4.188 [4.188, 4.188] (5/5) | 0.924 [0.901, 0.932] (5/5) |
| live | cancel | 0.405 [0.342, 0.410] (5/5) | 0.405 [0.342, 0.410] (5/5) | 0.267 [0.231, 0.283] (5/5) | 4.780 [4.780, 4.780] (5/5) | 0.945 [0.936, 0.960] (5/5) |

## Warm：四个 case/mode 的绝对每turn成本

`short-small`先运行8个历史turn，`short-large`先运行64个历史turn，之后均测量32个turn。wall是每个产品32个turn延迟的中位数，再取五产品中位数；CPU和写入是测量区间计数差/32，再取五产品中位数。/proc CPU计数以100 ticks/s换算，单个累计端点粒度10ms；这不是100Hz profiler采样，真实计数零会保留。

Direct批次：每格为上轮 complete-costs → Pi-flow 候选；不把该批候选与 fresh Pi 批次混池。

| 模式 | case | 历史turn | wall中位 ms/turn | measured self CPU ms/turn | measured write bytes/turn |
| --- | --- | --- | --- | --- | --- |
| no-live | short-small | 8 | 21.279 → 18.192 | 23.438 → 20.938 | 1,569,536.0 → 1,097,472.0 |
| no-live | short-large | 64 | 21.877 → 19.165 | 24.062 → 20.312 | 1,895,808.0 → 1,254,912.0 |
| live | short-small | 8 | 26.994 → 21.323 | 34.375 → 32.812 | 1,569,536.0 → 1,097,600.0 |
| live | short-large | 64 | 29.689 → 20.935 | 39.375 → 29.062 | 1,892,992.0 → 1,255,168.0 |

Fresh Pi批次：每格为 Pi → 最终 rustX。

| 模式 | case | 历史turn | wall中位 ms/turn | measured self CPU ms/turn | measured write bytes/turn |
| --- | --- | --- | --- | --- | --- |
| no-live | short-small | 8 | 3.135 → 17.840 | 7.188 → 19.375 | 224,512.0 → 1,097,728.0 |
| no-live | short-large | 64 | 3.299 → 19.563 | 6.250 → 21.250 | 224,384.0 → 1,254,528.0 |
| live | short-small | 8 | 3.420 → 19.794 | 7.500 → 28.125 | 224,512.0 → 1,097,728.0 |
| live | short-large | 64 | 3.541 → 22.072 | 6.875 → 31.250 | 224,384.0 → 1,254,912.0 |

## Native三引擎：独立参考的绝对测量成本

此批仅live，四个case × 三引擎 × 三次。均值wall先取每产品turn均值，再取三产品中位数；CPU/写入也取每产品每turn值的三产品中位数，直接读取独立三引擎审计 `groups`。中位wall先取每产品turn中位数，再取三产品中位数，来自已审计 `supplementary_table`，均不复算原始数据。方括号为三次产品值的[min,max]，不是混池总体算术平均。DeepSeek的SDK idle不等待最终JSONL flush/fsync，不能视为durable settlement等价，也不计算跨引擎等持久性比值。

| case | 引擎 | 均值wall ms/turn [min,max] | 中位wall ms/turn | measured CPU ms/turn [min,max] | measured write bytes/turn [min,max] |
| --- | --- | --- | --- | --- | --- |
| short-small | rust | 21.847 [12.424, 22.638] (3/3) | 21.537 | 31.250 [17.812, 32.812] (3/3) | 1,097,984.0 [1,097,856.0, 1,097,984.0] (3/3) |
| short-small | pi | 4.087 [3.889, 4.089] (3/3) | 3.430 | 7.812 [7.500, 8.125] (3/3) | 224,512.0 [224,512.0, 224,512.0] (3/3) |
| short-small | deepseek | 15.541 [14.864, 16.176] (3/3) | 14.774 | 18.125 [17.188, 18.750] (3/3) | 21,504.0 [21,504.0, 21,504.0] (3/3) |
| short-large | rust | 23.325 [19.914, 23.750] (3/3) | 23.172 | 33.438 [28.438, 34.062] (3/3) | 1,254,144.0 [1,253,632.0, 1,254,400.0] (3/3) |
| short-large | pi | 3.914 [3.893, 4.113] (3/3) | 3.585 | 6.875 [6.562, 6.875] (3/3) | 224,384.0 [224,384.0, 224,384.0] (3/3) |
| short-large | deepseek | 15.450 [15.414, 16.023] (3/3) | 15.336 | 17.500 [17.188, 18.125] (3/3) | 21,376.0 [21,376.0, 21,376.0] (3/3) |
| stream | rust | 1,584.178 [1,560.470, 1,601.459] (3/3) | 1,584.178 | 150.000 [130.000, 150.000] (3/3) | 1,458,176.0 [1,458,176.0, 1,458,176.0] (3/3) |
| stream | pi | 1,690.757 [1,608.556, 1,742.362] (3/3) | 1,690.757 | 570.000 [480.000, 660.000] (3/3) | 516,096.0 [499,712.0, 536,576.0] (3/3) |
| stream | deepseek | 1,583.218 [1,555.872, 1,598.485] (3/3) | 1,583.218 | 300.000 [280.000, 320.000] (3/3) | 28,672.0 [28,672.0, 28,672.0] (3/3) |
| large | rust | 41.889 [33.709, 50.663] (3/3) | 41.889 | 50.000 [40.000, 60.000] (3/3) | 1,011,712.0 [1,011,712.0, 1,011,712.0] (3/3) |
| large | pi | 90.497 [87.030, 95.069] (3/3) | 90.497 | 120.000 [110.000, 120.000] (3/3) | 294,912.0 [294,912.0, 294,912.0] (3/3) |
| large | deepseek | 165.613 [165.132, 165.822] (3/3) | 165.613 | 200.000 [190.000, 200.000] (3/3) | 28,672.0 [28,672.0, 32,768.0] (3/3) |

Rust/Pi这里的CPU为self CPU，包含同进程benchmark driver；DeepSeek为采样遍历中存活进程及已等待子进程的CPU。DeepSeek写入不涵盖已退出子进程，均非设备I/O；不能据此做等生命周期整体成本比值。

### DeepSeek关闭与持久化边界单列

下面CPU为首次native runtime与复验native runtime退出时wait4计数，包含完整退出过程，但不包含外部driver/provider CPU；不得用它代替上表的测量区间CPU。当前证据没有独立flush耗时计数，也没有退出后的proc write_bytes端点：完整写入量与flush/fsync延迟均记不可用，不记零。可读close/resume证据不能证明断电持久性或fsync成功；SDK disposal中的allSettled可能吞掉失败。

| case | 首次runtime完整CPU ms | 复验runtime完整CPU ms | 两runtime完整CPU ms | 完整write bytes | 独立flush/fsync ms |
| --- | --- | --- | --- | --- | --- |
| short-small | 2,343.295 | 1,621.093 | 3,976.084 | 不可用 | 不可用 |
| short-large | 3,304.869 | 1,638.379 | 4,940.365 | 不可用 | 不可用 |
| stream | 1,710.586 | 1,609.854 | 3,326.846 | 不可用 | 不可用 |
| large | 1,586.904 | 1,584.731 | 3,168.942 | 不可用 | 不可用 |

## 全量不利项：不隐藏中位回退或单对回退

以下扫描direct与fresh Pi审计中的所有指标，不只筛选主表指标。每个列出的cell至少一个逐对差值>0；“中位较高”表示配对百分比中位数>0，“仅部分配对较高”保留即使中位数改善也出现的不利样本。这里描述数值，不把更多功能性存储或任何差值自动解释为缺陷。无可用比值但存在正差值的cell仍列出。

| 比较 | 模式 | case | 审计metric | 分类 | 配对 Δ% median [min,max] | 较高对数 | 五对均较高 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| direct | no-live | short-small | max_sampled_anon_pss_mib | 仅部分配对较高 | -0.62% [-1.50%, +0.88%] (5/5) | 2/5 | 否 |
| direct | no-live | short-small | max_sampled_pss_mib | 仅部分配对较高 | -0.72% [-0.87%, +1.01%] (5/5) | 1/5 | 否 |
| direct | no-live | short-small | max_sampled_rss_mib | 仅部分配对较高 | -0.69% [-0.86%, +0.99%] (5/5) | 1/5 | 否 |
| direct | no-live | short-small | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.77% [-1.08%, +0.89%] (5/5) | 1/5 | 否 |
| direct | no-live | short-large | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -16.83% [-47.10%, +4.05%] (5/5) | 1/5 | 否 |
| direct | no-live | short-large | lifecycle_ms | 仅部分配对较高 | -18.08% [-48.97%, +4.42%] (5/5) | 2/5 | 否 |
| direct | no-live | short-large | lifecycle_self_cpu_ms | 仅部分配对较高 | -1.99% [-21.50%, +5.45%] (5/5) | 1/5 | 否 |
| direct | no-live | short-large | mean_turn_ms | 仅部分配对较高 | -46.89% [-76.43%, +29.86%] (5/5) | 2/5 | 否 |
| direct | no-live | short-large | measured_turn_cpu_ms | 仅部分配对较高 | -9.52% [-47.06%, +32.73%] (5/5) | 2/5 | 否 |
| direct | no-live | short-large | median_measured_turn_ms | 仅部分配对较高 | -17.34% [-55.53%, +39.46%] (5/5) | 2/5 | 否 |
| direct | no-live | short-large | through_cleanup_self_cpu_ms | 仅部分配对较高 | -1.99% [-21.50%, +5.45%] (5/5) | 1/5 | 否 |
| direct | no-live | short-large | through_product_exit_wall_ms | 仅部分配对较高 | -17.27% [-47.93%, +4.12%] (5/5) | 1/5 | 否 |
| direct | no-live | short-large | turn_cpu_ms_per_turn | 仅部分配对较高 | -9.52% [-47.06%, +32.73%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | complete_harness_lifecycle_wall_ms | 中位较高 | +0.30% [-1.63%, +3.92%] (5/5) | 4/5 | 否 |
| direct | no-live | stream | lifecycle_ms | 中位较高 | +0.32% [-1.95%, +2.97%] (5/5) | 3/5 | 否 |
| direct | no-live | stream | lifecycle_self_cpu_ms | 仅部分配对较高 | +0.00% [-6.90%, +15.38%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | max_sampled_anon_pss_mib | 仅部分配对较高 | -1.20% [-1.87%, +0.60%] (5/5) | 1/5 | 否 |
| direct | no-live | stream | max_sampled_pss_mib | 仅部分配对较高 | -0.99% [-1.58%, +0.42%] (5/5) | 1/5 | 否 |
| direct | no-live | stream | max_sampled_rss_mib | 仅部分配对较高 | -0.98% [-1.57%, +0.39%] (5/5) | 1/5 | 否 |
| direct | no-live | stream | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.81% [-1.41%, +0.69%] (5/5) | 1/5 | 否 |
| direct | no-live | stream | mean_turn_ms | 仅部分配对较高 | -0.05% [-2.05%, +2.75%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | measured_turn_cpu_ms | 仅部分配对较高 | +0.00% [-16.67%, +20.00%] (5/5) | 1/5 | 否 |
| direct | no-live | stream | median_measured_turn_ms | 仅部分配对较高 | -0.05% [-2.05%, +2.75%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | through_cleanup_self_cpu_ms | 仅部分配对较高 | +0.00% [-6.90%, +15.38%] (5/5) | 2/5 | 否 |
| direct | no-live | stream | through_product_exit_wall_ms | 中位较高 | +0.45% [-2.06%, +3.19%] (5/5) | 4/5 | 否 |
| direct | no-live | stream | turn_cpu_ms_per_turn | 仅部分配对较高 | +0.00% [-16.67%, +20.00%] (5/5) | 1/5 | 否 |
| direct | no-live | large | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -4.19% [-4.49%, +2.49%] (5/5) | 2/5 | 否 |
| direct | no-live | large | lifecycle_ms | 仅部分配对较高 | -4.26% [-4.72%, +0.16%] (5/5) | 1/5 | 否 |
| direct | no-live | large | lifecycle_self_cpu_ms | 仅部分配对较高 | +0.00% [-6.67%, +6.67%] (5/5) | 1/5 | 否 |
| direct | no-live | large | max_sampled_anon_pss_mib | 仅部分配对较高 | -0.19% [-0.90%, +1.81%] (5/5) | 2/5 | 否 |
| direct | no-live | large | max_sampled_pss_mib | 仅部分配对较高 | -0.15% [-0.63%, +0.68%] (5/5) | 2/5 | 否 |
| direct | no-live | large | max_sampled_rss_mib | 仅部分配对较高 | -0.22% [-0.61%, +0.67%] (5/5) | 2/5 | 否 |
| direct | no-live | large | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.50% [-1.10%, +0.34%] (5/5) | 2/5 | 否 |
| direct | no-live | large | mean_turn_ms | 仅部分配对较高 | -3.45% [-19.21%, +2.30%] (5/5) | 1/5 | 否 |
| direct | no-live | large | median_measured_turn_ms | 仅部分配对较高 | -3.45% [-19.21%, +2.30%] (5/5) | 1/5 | 否 |
| direct | no-live | large | through_cleanup_self_cpu_ms | 仅部分配对较高 | +0.00% [-6.67%, +6.67%] (5/5) | 1/5 | 否 |
| direct | no-live | large | through_product_exit_wall_ms | 仅部分配对较高 | -3.57% [-4.55%, +1.31%] (5/5) | 1/5 | 否 |
| direct | no-live | tool | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -1.54% [-5.24%, +1.04%] (5/5) | 1/5 | 否 |
| direct | no-live | tool | lifecycle_ms | 仅部分配对较高 | -2.28% [-10.35%, +0.67%] (5/5) | 2/5 | 否 |
| direct | no-live | tool | max_sampled_anon_pss_mib | 仅部分配对较高 | -1.13% [-1.90%, +0.25%] (5/5) | 1/5 | 否 |
| direct | no-live | tool | max_sampled_pss_mib | 仅部分配对较高 | -0.84% [-1.05%, +0.16%] (5/5) | 1/5 | 否 |
| direct | no-live | tool | max_sampled_rss_mib | 仅部分配对较高 | -0.77% [-1.03%, +0.15%] (5/5) | 1/5 | 否 |
| direct | no-live | cancel | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -0.47% [-12.80%, +12.73%] (5/5) | 2/5 | 否 |
| direct | no-live | cancel | lifecycle_ms | 仅部分配对较高 | -1.91% [-8.16%, +9.48%] (5/5) | 2/5 | 否 |
| direct | no-live | cancel | max_sampled_anon_pss_mib | 仅部分配对较高 | -0.63% [-1.23%, +0.04%] (5/5) | 1/5 | 否 |
| direct | no-live | cancel | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.27% [-1.09%, +0.15%] (5/5) | 1/5 | 否 |
| direct | no-live | cancel | mean_turn_ms | 中位较高 | +12.14% [-14.14%, +22.85%] (5/5) | 4/5 | 否 |
| direct | no-live | cancel | measured_turn_cpu_ms | 中位较高 | +33.33% [-25.00%, +50.00%] (5/5) | 4/5 | 否 |
| direct | no-live | cancel | median_measured_turn_ms | 中位较高 | +12.14% [-14.14%, +22.85%] (5/5) | 4/5 | 否 |
| direct | no-live | cancel | through_product_exit_wall_ms | 中位较高 | +0.01% [-9.67%, +9.91%] (5/5) | 3/5 | 否 |
| direct | no-live | cancel | turn_cpu_ms_per_turn | 中位较高 | +33.33% [-25.00%, +50.00%] (5/5) | 4/5 | 否 |
| direct | live | short-small | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -8.35% [-13.73%, +19.71%] (5/5) | 2/5 | 否 |
| direct | live | short-small | lifecycle_ms | 仅部分配对较高 | -9.89% [-14.38%, +21.59%] (5/5) | 2/5 | 否 |
| direct | live | short-small | lifecycle_self_cpu_ms | 仅部分配对较高 | -8.25% [-14.71%, +29.93%] (5/5) | 2/5 | 否 |
| direct | live | short-small | mean_turn_ms | 仅部分配对较高 | -17.14% [-25.42%, +42.15%] (5/5) | 2/5 | 否 |
| direct | live | short-small | measured_turn_cpu_ms | 仅部分配对较高 | -10.26% [-24.79%, +49.30%] (5/5) | 2/5 | 否 |
| direct | live | short-small | median_measured_turn_ms | 仅部分配对较高 | -21.01% [-25.41%, +41.65%] (5/5) | 2/5 | 否 |
| direct | live | short-small | through_cleanup_self_cpu_ms | 仅部分配对较高 | -8.25% [-14.71%, +29.93%] (5/5) | 2/5 | 否 |
| direct | live | short-small | through_product_exit_wall_ms | 仅部分配对较高 | -9.26% [-13.84%, +20.78%] (5/5) | 2/5 | 否 |
| direct | live | short-small | turn_cpu_ms_per_turn | 仅部分配对较高 | -10.26% [-24.79%, +49.30%] (5/5) | 2/5 | 否 |
| direct | live | short-large | lifecycle_self_cpu_ms | 仅部分配对较高 | -9.54% [-19.68%, +1.25%] (5/5) | 1/5 | 否 |
| direct | live | short-large | max_sampled_anon_pss_mib | 仅部分配对较高 | -0.24% [-0.36%, +0.70%] (5/5) | 1/5 | 否 |
| direct | live | short-large | max_sampled_pss_mib | 仅部分配对较高 | -0.12% [-0.59%, +0.59%] (5/5) | 1/5 | 否 |
| direct | live | short-large | max_sampled_rss_mib | 仅部分配对较高 | -0.12% [-0.56%, +0.59%] (5/5) | 2/5 | 否 |
| direct | live | short-large | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.31% [-0.84%, +0.61%] (5/5) | 1/5 | 否 |
| direct | live | short-large | mean_turn_ms | 仅部分配对较高 | -21.50% [-53.90%, +16.60%] (5/5) | 1/5 | 否 |
| direct | live | short-large | measured_turn_cpu_ms | 仅部分配对较高 | -16.91% [-50.77%, +22.37%] (5/5) | 2/5 | 否 |
| direct | live | short-large | median_measured_turn_ms | 仅部分配对较高 | -22.95% [-57.92%, +31.57%] (5/5) | 1/5 | 否 |
| direct | live | short-large | through_cleanup_self_cpu_ms | 仅部分配对较高 | -9.54% [-19.68%, +1.25%] (5/5) | 1/5 | 否 |
| direct | live | short-large | turn_cpu_ms_per_turn | 仅部分配对较高 | -16.91% [-50.77%, +22.37%] (5/5) | 2/5 | 否 |
| direct | live | stream | complete_harness_lifecycle_wall_ms | 中位较高 | +0.25% [-2.04%, +1.56%] (5/5) | 3/5 | 否 |
| direct | live | stream | lifecycle_ms | 中位较高 | +0.17% [-1.88%, +0.49%] (5/5) | 3/5 | 否 |
| direct | live | stream | lifecycle_self_cpu_ms | 仅部分配对较高 | -3.23% [-16.67%, +7.69%] (5/5) | 2/5 | 否 |
| direct | live | stream | max_sampled_anon_pss_mib | 仅部分配对较高 | -0.42% [-2.20%, +0.46%] (5/5) | 1/5 | 否 |
| direct | live | stream | max_sampled_pss_mib | 仅部分配对较高 | -0.51% [-1.99%, +0.01%] (5/5) | 1/5 | 否 |
| direct | live | stream | max_sampled_rss_mib | 仅部分配对较高 | -0.55% [-2.03%, +0.00%] (5/5) | 1/5 | 否 |
| direct | live | stream | mean_turn_ms | 仅部分配对较高 | -0.16% [-1.58%, +1.87%] (5/5) | 2/5 | 否 |
| direct | live | stream | measured_turn_cpu_ms | 仅部分配对较高 | +0.00% [-12.50%, +13.33%] (5/5) | 2/5 | 否 |
| direct | live | stream | median_measured_turn_ms | 仅部分配对较高 | -0.16% [-1.58%, +1.87%] (5/5) | 2/5 | 否 |
| direct | live | stream | through_cleanup_self_cpu_ms | 仅部分配对较高 | -3.23% [-16.67%, +7.69%] (5/5) | 2/5 | 否 |
| direct | live | stream | through_product_exit_wall_ms | 中位较高 | +0.13% [-1.87%, +0.54%] (5/5) | 3/5 | 否 |
| direct | live | stream | turn_cpu_ms_per_turn | 仅部分配对较高 | +0.00% [-12.50%, +13.33%] (5/5) | 2/5 | 否 |
| direct | live | large | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -1.71% [-2.36%, +6.00%] (5/5) | 2/5 | 否 |
| direct | live | large | lifecycle_ms | 中位较高 | +0.01% [-3.74%, +4.03%] (5/5) | 3/5 | 否 |
| direct | live | large | lifecycle_self_cpu_ms | 中位较高 | +5.56% [-6.25%, +5.88%] (5/5) | 3/5 | 否 |
| direct | live | large | mean_turn_ms | 中位较高 | +4.13% [-8.80%, +36.43%] (5/5) | 3/5 | 否 |
| direct | live | large | measured_turn_cpu_ms | 仅部分配对较高 | +0.00% [+0.00%, +66.67%] (5/5) | 2/5 | 否 |
| direct | live | large | median_measured_turn_ms | 中位较高 | +4.13% [-8.80%, +36.43%] (5/5) | 3/5 | 否 |
| direct | live | large | through_cleanup_self_cpu_ms | 中位较高 | +5.56% [-6.25%, +5.88%] (5/5) | 3/5 | 否 |
| direct | live | large | through_product_exit_wall_ms | 仅部分配对较高 | -0.96% [-3.63%, +4.57%] (5/5) | 2/5 | 否 |
| direct | live | large | turn_cpu_ms_per_turn | 仅部分配对较高 | +0.00% [+0.00%, +66.67%] (5/5) | 2/5 | 否 |
| direct | live | tool | complete_harness_lifecycle_wall_ms | 中位较高 | +1.88% [-8.47%, +16.99%] (5/5) | 3/5 | 否 |
| direct | live | tool | lifecycle_ms | 中位较高 | +2.85% [-7.50%, +18.35%] (5/5) | 3/5 | 否 |
| direct | live | tool | lifecycle_self_cpu_ms | 中位较高 | +5.56% [-14.29%, +22.22%] (5/5) | 3/5 | 否 |
| direct | live | tool | max_sampled_anon_pss_mib | 仅部分配对较高 | -1.03% [-2.25%, +0.74%] (5/5) | 1/5 | 否 |
| direct | live | tool | max_sampled_pss_mib | 仅部分配对较高 | -0.48% [-1.53%, +0.33%] (5/5) | 1/5 | 否 |
| direct | live | tool | max_sampled_rss_mib | 仅部分配对较高 | -0.45% [-1.48%, +0.35%] (5/5) | 1/5 | 否 |
| direct | live | tool | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.32% [-1.21%, +0.43%] (5/5) | 1/5 | 否 |
| direct | live | tool | mean_turn_ms | 中位较高 | +14.71% [-20.76%, +47.57%] (5/5) | 3/5 | 否 |
| direct | live | tool | measured_turn_cpu_ms | 中位较高 | +16.67% [-37.50%, +80.00%] (5/5) | 3/5 | 否 |
| direct | live | tool | median_measured_turn_ms | 中位较高 | +14.71% [-20.76%, +47.57%] (5/5) | 3/5 | 否 |
| direct | live | tool | through_cleanup_self_cpu_ms | 中位较高 | +5.56% [-14.29%, +22.22%] (5/5) | 3/5 | 否 |
| direct | live | tool | through_product_exit_wall_ms | 中位较高 | +1.53% [-8.87%, +17.01%] (5/5) | 3/5 | 否 |
| direct | live | tool | turn_cpu_ms_per_turn | 中位较高 | +16.67% [-37.50%, +80.00%] (5/5) | 3/5 | 否 |
| direct | live | cancel | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -2.16% [-11.81%, +5.28%] (5/5) | 2/5 | 否 |
| direct | live | cancel | lifecycle_ms | 中位较高 | +0.47% [-11.94%, +4.34%] (5/5) | 3/5 | 否 |
| direct | live | cancel | lifecycle_self_cpu_ms | 仅部分配对较高 | -5.56% [-11.76%, +13.33%] (5/5) | 1/5 | 否 |
| direct | live | cancel | max_sampled_anon_pss_mib | 中位较高 | +0.21% [-0.83%, +0.48%] (5/5) | 3/5 | 否 |
| direct | live | cancel | max_sampled_pss_mib | 仅部分配对较高 | -0.26% [-1.00%, +0.35%] (5/5) | 2/5 | 否 |
| direct | live | cancel | max_sampled_rss_mib | 仅部分配对较高 | -0.30% [-0.91%, +0.33%] (5/5) | 2/5 | 否 |
| direct | live | cancel | max_sampled_turn_rss_mib | 仅部分配对较高 | -0.52% [-1.06%, +0.54%] (5/5) | 2/5 | 否 |
| direct | live | cancel | mean_turn_ms | 仅部分配对较高 | -7.35% [-22.43%, +19.38%] (5/5) | 1/5 | 否 |
| direct | live | cancel | measured_turn_cpu_ms | 仅部分配对较高 | -16.67% [-20.00%, +33.33%] (5/5) | 1/5 | 否 |
| direct | live | cancel | median_measured_turn_ms | 仅部分配对较高 | -7.35% [-22.43%, +19.38%] (5/5) | 1/5 | 否 |
| direct | live | cancel | through_cleanup_self_cpu_ms | 仅部分配对较高 | -5.56% [-11.76%, +13.33%] (5/5) | 1/5 | 否 |
| direct | live | cancel | through_product_exit_wall_ms | 中位较高 | +0.05% [-12.22%, +5.05%] (5/5) | 3/5 | 否 |
| direct | live | cancel | turn_cpu_ms_per_turn | 仅部分配对较高 | -16.67% [-20.00%, +33.33%] (5/5) | 1/5 | 否 |
| fresh Pi | no-live | short-small | closed_allocated_bytes | 中位较高 | +4442.86% [+4440.48%, +4445.24%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | closed_logical_bytes | 中位较高 | +4433.80% [+4431.40%, +4436.19%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | complete_harness_lifecycle_wall_ms | 中位较高 | +106.23% [+62.73%, +118.07%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | lifecycle_ms | 中位较高 | +220.40% [+139.00%, +234.91%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | lifecycle_self_cpu_ms | 中位较高 | +168.63% [+89.83%, +182.00%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | max_sampled_anon_pss_mib | 中位较高 | +11.92% [+10.64%, +18.72%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | max_sampled_pss_mib | 中位较高 | +45.35% [+43.17%, +52.67%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | max_sampled_rss_mib | 中位较高 | +6.22% [+4.91%, +9.82%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | mean_turn_ms | 中位较高 | +385.42% [+150.70%, +442.49%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | measured_turn_cpu_ms | 中位较高 | +177.27% [+32.26%, +204.35%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | measured_turn_write_bytes | 中位较高 | +388.94% [+388.83%, +389.05%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | median_measured_turn_ms | 中位较高 | +469.84% [+209.30%, +527.19%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | through_cleanup_process_write_bytes | 中位较高 | +379.80% [+379.71%, +379.89%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | through_cleanup_process_write_mib | 中位较高 | +379.80% [+379.71%, +379.89%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | through_cleanup_self_cpu_ms | 中位较高 | +45.26% [+9.80%, +51.61%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | through_product_exit_wall_ms | 中位较高 | +106.90% [+60.77%, +116.07%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | turn_cpu_ms_per_turn | 中位较高 | +177.27% [+32.26%, +204.35%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-small | turn_write_bytes_per_turn | 中位较高 | +388.94% [+388.83%, +389.05%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | closed_allocated_bytes | 中位较高 | +4558.33% [+4556.67%, +4563.33%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | closed_logical_bytes | 中位较高 | +4552.00% [+4550.32%, +4555.35%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | complete_harness_lifecycle_wall_ms | 中位较高 | +283.06% [+235.56%, +356.59%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | lifecycle_ms | 中位较高 | +420.33% [+343.45%, +513.37%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | lifecycle_self_cpu_ms | 中位较高 | +307.77% [+278.79%, +351.61%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | max_sampled_anon_pss_mib | 中位较高 | +27.90% [+26.19%, +30.23%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | max_sampled_pss_mib | 中位较高 | +50.47% [+48.04%, +52.72%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | max_sampled_rss_mib | 中位较高 | +19.23% [+17.71%, +20.55%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | mean_turn_ms | 中位较高 | +408.98% [+211.62%, +1088.95%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | measured_turn_cpu_ms | 中位较高 | +240.00% [+104.55%, +263.16%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | measured_turn_write_bytes | 中位较高 | +459.10% [+458.59%, +459.33%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | median_measured_turn_ms | 中位较高 | +495.14% [+226.59%, +520.25%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | through_cleanup_process_write_bytes | 中位较高 | +428.86% [+428.78%, +429.03%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | through_cleanup_process_write_mib | 中位较高 | +428.86% [+428.78%, +429.03%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | through_cleanup_self_cpu_ms | 中位较高 | +187.67% [+164.08%, +213.14%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | through_product_exit_wall_ms | 中位较高 | +294.91% [+243.87%, +370.83%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | turn_cpu_ms_per_turn | 中位较高 | +240.00% [+104.55%, +263.16%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | short-large | turn_write_bytes_per_turn | 中位较高 | +459.10% [+458.59%, +459.33%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | closed_allocated_bytes | 中位较高 | +1578.12% [+1575.00%, +1578.12%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | closed_logical_bytes | 中位较高 | +1565.53% [+1562.39%, +1565.53%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | complete_harness_lifecycle_wall_ms | 仅部分配对较高 | -13.23% [-14.53%, +1.95%] (5/5) | 1/5 | 否 |
| fresh Pi | no-live | stream | lifecycle_ms | 仅部分配对较高 | -0.32% [-2.06%, +14.54%] (5/5) | 2/5 | 否 |
| fresh Pi | no-live | stream | max_sampled_pss_mib | 中位较高 | +21.85% [+20.71%, +22.53%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | measured_turn_write_bytes | 中位较高 | +182.54% [+181.75%, +191.80%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | through_cleanup_process_write_bytes | 中位较高 | +301.82% [+300.91%, +309.26%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | through_cleanup_process_write_mib | 中位较高 | +301.82% [+300.91%, +309.26%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | stream | through_product_exit_wall_ms | 仅部分配对较高 | -13.01% [-14.53%, +1.92%] (5/5) | 1/5 | 否 |
| fresh Pi | no-live | stream | turn_write_bytes_per_turn | 中位较高 | +182.54% [+181.75%, +191.80%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | closed_allocated_bytes | 中位较高 | +1026.32% [+1026.32%, +1026.32%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | closed_logical_bytes | 中位较高 | +1016.68% [+1016.68%, +1016.68%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | lifecycle_ms | 中位较高 | +20.76% [+10.07%, +28.99%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | lifecycle_self_cpu_ms | 仅部分配对较高 | +0.00% [-6.25%, +6.67%] (5/5) | 1/5 | 否 |
| fresh Pi | no-live | large | max_sampled_anon_pss_mib | 中位较高 | +7.37% [+6.15%, +7.99%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | max_sampled_pss_mib | 中位较高 | +51.44% [+49.48%, +52.00%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | max_sampled_rss_mib | 中位较高 | +2.44% [+1.23%, +2.67%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | max_sampled_turn_rss_mib | 中位较高 | +2.35% [+1.15%, +2.57%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | measured_turn_write_bytes | 中位较高 | +243.06% [+243.06%, +243.06%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | through_cleanup_process_write_bytes | 中位较高 | +355.23% [+355.23%, +355.23%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | through_cleanup_process_write_mib | 中位较高 | +355.23% [+355.23%, +355.23%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | large | turn_write_bytes_per_turn | 中位较高 | +243.06% [+243.06%, +243.06%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | closed_allocated_bytes | 中位较高 | +1887.50% [+1887.50%, +1887.50%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | closed_logical_bytes | 中位较高 | +1876.72% [+1876.72%, +1876.72%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | lifecycle_ms | 中位较高 | +11.37% [+7.04%, +20.26%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | max_sampled_pss_mib | 中位较高 | +34.32% [+31.04%, +38.50%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | max_sampled_turn_rss_mib | 中位较高 | +3.33% [+1.95%, +4.13%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | measured_turn_write_bytes | 中位较高 | +211.64% [+211.64%, +211.64%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | through_cleanup_process_write_bytes | 中位较高 | +318.75% [+318.75%, +318.75%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | through_cleanup_process_write_mib | 中位较高 | +318.75% [+318.75%, +318.75%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | tool | turn_write_bytes_per_turn | 中位较高 | +211.64% [+211.64%, +211.64%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | closed_allocated_bytes | 中位较高 | +1293.33% [+1293.33%, +1293.33%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | closed_logical_bytes | 中位较高 | +1280.93% [+1280.93%, +1280.93%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | lifecycle_ms | 中位较高 | +12.49% [+8.52%, +22.19%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | max_sampled_pss_mib | 中位较高 | +36.11% [+30.29%, +40.03%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | max_sampled_turn_rss_mib | 中位较高 | +1.48% [+0.69%, +2.64%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | measured_turn_write_bytes | 中位较高 | +253.73% [+253.73%, +253.73%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | through_cleanup_process_write_bytes | 中位较高 | +377.99% [+377.99%, +377.99%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | through_cleanup_process_write_mib | 中位较高 | +377.99% [+377.99%, +377.99%] (5/5) | 5/5 | 是 |
| fresh Pi | no-live | cancel | turn_write_bytes_per_turn | 中位较高 | +253.73% [+253.73%, +253.73%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | closed_allocated_bytes | 中位较高 | +4445.24% [+4442.86%, +4445.24%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | closed_logical_bytes | 中位较高 | +4436.19% [+4433.80%, +4436.19%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | complete_harness_lifecycle_wall_ms | 中位较高 | +112.11% [+94.36%, +121.40%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | lifecycle_ms | 中位较高 | +224.80% [+191.89%, +242.29%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | lifecycle_self_cpu_ms | 中位较高 | +218.52% [+177.19%, +233.33%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | max_sampled_anon_pss_mib | 中位较高 | +21.33% [+15.49%, +28.44%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | max_sampled_pss_mib | 中位较高 | +54.34% [+47.87%, +62.86%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | max_sampled_rss_mib | 中位较高 | +12.18% [+8.59%, +16.47%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | max_sampled_turn_rss_mib | 中位较高 | +1.12% [-1.34%, +7.67%] (5/5) | 3/5 | 否 |
| fresh Pi | live | short-small | mean_turn_ms | 中位较高 | +388.43% [+293.22%, +504.15%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | measured_turn_cpu_ms | 中位较高 | +273.91% [+181.48%, +329.17%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | measured_turn_write_bytes | 中位较高 | +388.94% [+388.83%, +389.05%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | median_measured_turn_ms | 中位较高 | +478.77% [+285.62%, +537.57%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | through_cleanup_process_write_bytes | 中位较高 | +379.80% [+379.71%, +379.89%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | through_cleanup_process_write_mib | 中位较高 | +379.80% [+379.71%, +379.89%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | through_cleanup_self_cpu_ms | 中位较高 | +79.17% [+56.44%, +87.50%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | through_product_exit_wall_ms | 中位较高 | +113.10% [+94.32%, +122.77%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | turn_cpu_ms_per_turn | 中位较高 | +273.91% [+181.48%, +329.17%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-small | turn_write_bytes_per_turn | 中位较高 | +388.94% [+388.83%, +389.05%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | closed_allocated_bytes | 中位较高 | +4555.00% [+4551.67%, +4556.67%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | closed_logical_bytes | 中位较高 | +4546.98% [+4545.30%, +4548.66%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | complete_harness_lifecycle_wall_ms | 中位较高 | +271.45% [+243.72%, +297.20%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | lifecycle_ms | 中位较高 | +382.58% [+354.77%, +426.36%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | lifecycle_self_cpu_ms | 中位较高 | +373.64% [+327.45%, +409.35%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | max_sampled_anon_pss_mib | 中位较高 | +31.56% [+28.39%, +32.76%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | max_sampled_pss_mib | 中位较高 | +53.38% [+50.36%, +54.60%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | max_sampled_rss_mib | 中位较高 | +21.80% [+19.90%, +22.59%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | mean_turn_ms | 中位较高 | +433.20% [+404.23%, +507.02%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | measured_turn_cpu_ms | 中位较高 | +352.17% [+322.73%, +428.57%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | measured_turn_write_bytes | 中位较高 | +459.27% [+458.70%, +460.01%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | median_measured_turn_ms | 中位较高 | +532.06% [+489.90%, +569.41%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | through_cleanup_process_write_bytes | 中位较高 | +428.97% [+428.71%, +429.22%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | through_cleanup_process_write_mib | 中位较高 | +428.97% [+428.71%, +429.22%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | through_cleanup_self_cpu_ms | 中位较高 | +240.52% [+200.69%, +260.93%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | through_product_exit_wall_ms | 中位较高 | +281.04% [+251.76%, +309.22%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | turn_cpu_ms_per_turn | 中位较高 | +352.17% [+322.73%, +428.57%] (5/5) | 5/5 | 是 |
| fresh Pi | live | short-large | turn_write_bytes_per_turn | 中位较高 | +459.27% [+458.70%, +460.01%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | closed_allocated_bytes | 中位较高 | +1575.00% [+1479.41%, +1578.12%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | closed_logical_bytes | 中位较高 | +1562.39% [+1467.56%, +1565.53%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | lifecycle_ms | 仅部分配对较高 | -2.92% [-4.90%, +0.97%] (5/5) | 1/5 | 否 |
| fresh Pi | live | stream | max_sampled_pss_mib | 中位较高 | +22.84% [+22.25%, +23.76%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | measured_turn_write_bytes | 中位较高 | +181.75% [+171.76%, +182.54%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | through_cleanup_process_write_bytes | 中位较高 | +300.91% [+289.43%, +301.82%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | through_cleanup_process_write_mib | 中位较高 | +300.91% [+289.43%, +301.82%] (5/5) | 5/5 | 是 |
| fresh Pi | live | stream | turn_write_bytes_per_turn | 中位较高 | +181.75% [+171.76%, +182.54%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | closed_allocated_bytes | 中位较高 | +1026.32% [+1026.32%, +1026.32%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | closed_logical_bytes | 中位较高 | +1016.68% [+1016.68%, +1016.68%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | lifecycle_ms | 中位较高 | +16.73% [+13.32%, +22.67%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | lifecycle_self_cpu_ms | 仅部分配对较高 | -6.25% [-6.25%, +6.67%] (5/5) | 2/5 | 否 |
| fresh Pi | live | large | max_sampled_anon_pss_mib | 中位较高 | +8.15% [+4.75%, +9.63%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | max_sampled_pss_mib | 中位较高 | +51.97% [+47.69%, +53.65%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | max_sampled_rss_mib | 中位较高 | +2.70% [+0.31%, +3.54%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | max_sampled_turn_rss_mib | 中位较高 | +3.05% [+0.50%, +3.44%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | measured_turn_write_bytes | 中位较高 | +243.06% [+243.06%, +243.06%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | through_cleanup_process_write_bytes | 中位较高 | +355.23% [+355.23%, +355.23%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | through_cleanup_process_write_mib | 中位较高 | +355.23% [+355.23%, +355.23%] (5/5) | 5/5 | 是 |
| fresh Pi | live | large | turn_write_bytes_per_turn | 中位较高 | +243.06% [+243.06%, +243.06%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | closed_allocated_bytes | 中位较高 | +1887.50% [+1887.50%, +1887.50%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | closed_logical_bytes | 中位较高 | +1876.72% [+1876.72%, +1876.72%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | lifecycle_ms | 中位较高 | +18.33% [+10.24%, +31.46%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | lifecycle_self_cpu_ms | 仅部分配对较高 | +0.00% [-15.79%, +5.26%] (5/5) | 2/5 | 否 |
| fresh Pi | live | tool | max_sampled_pss_mib | 中位较高 | +32.95% [+30.16%, +34.20%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | max_sampled_turn_rss_mib | 中位较高 | +2.76% [+1.81%, +3.79%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | measured_turn_write_bytes | 中位较高 | +211.64% [+211.64%, +211.64%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | through_cleanup_process_write_bytes | 中位较高 | +318.75% [+318.75%, +318.75%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | through_cleanup_process_write_mib | 中位较高 | +318.75% [+318.75%, +318.75%] (5/5) | 5/5 | 是 |
| fresh Pi | live | tool | turn_write_bytes_per_turn | 中位较高 | +211.64% [+211.64%, +211.64%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | closed_allocated_bytes | 中位较高 | +1293.33% [+1293.33%, +1293.33%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | closed_logical_bytes | 中位较高 | +1280.93% [+1280.93%, +1280.93%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | lifecycle_ms | 中位较高 | +17.87% [+10.94%, +33.05%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | lifecycle_self_cpu_ms | 仅部分配对较高 | -5.88% [-16.67%, +6.25%] (5/5) | 1/5 | 否 |
| fresh Pi | live | cancel | max_sampled_pss_mib | 中位较高 | +35.05% [+34.01%, +36.65%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | max_sampled_turn_rss_mib | 中位较高 | +1.21% [-0.57%, +1.51%] (5/5) | 4/5 | 否 |
| fresh Pi | live | cancel | measured_turn_write_bytes | 中位较高 | +253.73% [+253.73%, +253.73%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | through_cleanup_process_write_bytes | 中位较高 | +377.99% [+377.99%, +377.99%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | through_cleanup_process_write_mib | 中位较高 | +377.99% [+377.99%, +377.99%] (5/5) | 5/5 | 是 |
| fresh Pi | live | cancel | turn_write_bytes_per_turn | 中位较高 | +253.73% [+253.73%, +253.73%] (5/5) | 5/5 | 是 |

### 不可用配对单列

缺失计数与零分母保持不可用；不会用零或相邻cell补齐。

direct与fresh Pi所有审计配对比值均可用。

未显示为独立时延指标的DeepSeek flush仍是未测量，而非零耗时。先前所有轮次、当前探索筛查、SQL/WAL采集及CPU profiler数据均不进入本文件正式样本的中位数或比值。
