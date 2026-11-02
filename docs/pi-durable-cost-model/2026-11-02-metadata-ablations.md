# Targeted long-history diagnostics

Separate from the final six-case matrix. Control is the previous ordinary O3 Rust binary 8b19873c…. Five single-knob counterfactual builds and an older-Rust replication control each have six balanced-order pairs in each observer mode, CPU 2,8. Negative changes mean the counterfactual was faster than the current control; this does not identify a universally harmful component. No Pi/DeepSeek executes.

| Condition | Mode | Median wall % [min,max] | Window CPU % | Full CPU % | Full writes % | Wall lower/equal/higher |
|---|---|---:|---:|---:|---:|---:|
| deep-attempt-detach | no-live-subscriber | 1.498 [1.051,7.157] | 1.724 | 1.303 | -0.018 | 0/0/6 |
| deep-attempt-detach | live-subscriber | -0.671 [-7.293,2.744] | -2.598 | 0.608 | 0.018 | 4/0/2 |
| eager-attempt | no-live-subscriber | 1.801 [-9.033,8.423] | 1.667 | 0.326 | 0.011 | 2/0/4 |
| eager-attempt | live-subscriber | 0.030 [-8.924,2.774] | -2.564 | -1.506 | 0.029 | 3/0/3 |
| explicit-sequence-index | no-live-subscriber | -2.965 [-41.853,2.732] | -3.280 | -1.443 | 0.622 | 4/0/2 |
| explicit-sequence-index | live-subscriber | 0.731 [-11.286,6.974] | 0.000 | 0.157 | 0.618 | 3/0/3 |
| full-attempt-budget | no-live-subscriber | 0.963 [-29.668,13.303] | 1.667 | 0.000 | -0.027 | 2/0/4 |
| full-attempt-budget | live-subscriber | 0.466 [-4.427,7.112] | -2.632 | -0.303 | -0.032 | 3/0/3 |
| uncached-fold | no-live-subscriber | 1.277 [-6.720,2.405] | 0.000 | -0.648 | 0.016 | 1/0/5 |
| uncached-fold | live-subscriber | -2.513 [-6.369,4.184] | -2.598 | -0.454 | 0.000 | 4/0/2 |
| pre-round-rust-control | no-live-subscriber | 0.707 [-5.976,5.010] | 1.613 | 0.487 | 0.591 | 3/0/3 |
| pre-round-rust-control | live-subscriber | 0.448 [-5.241,11.081] | -1.316 | -0.142 | 0.640 | 3/0/3 |

Every pair and other counter remains in ablation-audit.json. The older code is +0.707% / +0.448% in this replication batch, with three pairs each direction in both modes; the prior approximately 5% regression is not stably reproduced here. This does not explain away or remove the earlier adverse batch. Different batches are not pooled.
