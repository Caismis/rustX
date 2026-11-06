# 原生节点读取合并与内容引用按需查找

本轮继续检查当前热路径，直接合并两处确定的重复操作。**原生输入叶节点验证从 2 次查询减少为 1 次，分支节点从 4 次减少为 3 次；内容引用解析从固定两次查找、两次构造 locator 键，改为一次构造、按需查找。** 这是局部操作计数的变化，不是整请求提速百分比。本轮没有新的延迟、CPU、写入或追平 Pi 结论。

起始提交 `84422d2bf11550fb1ba31bf1023903b4737a4360`；上次正式测量的生产实现仍是 `274d5d47e6e791cd9093aa40b64965def6d178fc`，不能把它的结果当作本轮新源码的性能。

## 已完成的合并

1. [原生输入节点](../../src/durable/sqlite/manifest.rs)原先先读取 digest/count/height，再读取同一行的 item/left/right digest 和节点形状。现在一次 SELECT 返回这两个有界部分，共用同一头部解码和边界检查。分支仍分别读取两个子节点的当前头部，并检查子节点 digest、数量、高度、平衡和分支摘要。没有读取 item_json 正文，也没有用构造缓存或旧 cut 替代数据库当前证据。现有准备语句回归在缓存容量 0、16、64 下断言实际工作计数；当前数据改写、后续授权拒绝和回滚回归继续通过。
2. [内容引用解析](../../src/runtime_client/projection.rs)原先无论 path 是否为 arguments，都先查询 raw representation，再查询原 representation；相同 representation 也查两次。现在只构造一个 locator 键：普通 path 只查原 representation；arguments 先查 raw，只有缺失且原 representation 不同才查第二次。保留原有优先级、缺失行为和每次重新解析当前 authority 的语义。新增一项回归覆盖 24 个 path/representation/可用来源组合；已有当前 authority 改变、合并缓存复用和旧 cut 不变的回归继续通过。

没有改变 schema 88、公开 DTO/线协议、提交或发布边界、WAL/NORMAL、独立派生域或恢复流程，没有新增依赖或兼容路径。

## 写入侧的复核

以下只重算[上一轮归档](evidence/content-runtime-20261102/README.md)的数据，没有重跑产品。每格为 32 个热轮次；帧归属由端点页面映射得到，不能解释为每次写入的严格因果归属，更不能解释为能够全部删除的成本。

| 历史轮数 | WAL 总帧 | 事件索引帧，含 event_id 唯一索引 | 占比 | Source 节点逻辑列字节 | 原生输入节点逻辑列字节 |
|---|---:|---:|---:|---:|---:|
| 8 | 5,665 | 1,687 | 29.779% | 75,523 | 43,559 |
| 64 | 6,389 | 1,973 | 30.881% | 94,678 | 50,891 |

事件索引累计是值得关注的写入类别，但现有索引支持不同 scope、历史范围、有界查找、唯一身份和独立 native/Trace provenance。本次没有找到一棵能直接删除且保留这些要求的索引，也没有证明一个更少写页的替代布局；不能由 30% 的归属占比推导出能减少 30% 的写入。

Source 节点和原生输入节点沿用相同拓扑，但前者保存宽度、字段定位、模板等派生证据，后者认证实际输入选择。Source 派生失败时，原生输入仍须可恢复；读取时也必须检验当前原生节点。因此两套行数相似不代表可以直接删除其中一套。单纯合并表或删掉当前字节校验没有满足现有功能约束的依据。本轮选择消除同一节点的重复 SELECT。

[FoldCache::remember](../../src/durable/sqlite/append_facts.rs)仍对编码缓冲区执行 to_vec。它是确定存在的复制，但尚没有可靠分配/CPU 归因证明其为主要成本。共享 owner 需要额外分配和引用计数；本轮没有引入这类未验证收益的所有权改造，也不把这项成本称为已经消除。

## 验证与边界

测试构建及 15 项检查命令通过：**2,359 项不同测试 passed、0 failed、1 项既有 ignored**。包括 SQLite 640、runtime client 616、72 项真实进程死亡、151 项 durable integration、62 项 process integration 和另外 8 项 artifact lineage。Fmt 和 release/all-target/all-feature Clippy（`-D warnings`）通过。

正确性构建使用 release 依赖、根包 O0、all features，不是生产性能二进制。964 个 src/tests 输入在构建及检查前后相同；两个 Cargo 输入与起始 HEAD 一致。没有运行完整仓库 CI/browser E2E，也没有合并 main。

[检查证据](evidence/read-fusion-20261102/README.md)保存源码补丁、输入哈希、构建和检查命令/输出、通过汇总，以及旧 WAL 归属的复算结果。归档生成后逐成员回读验证长度和 SHA256。

**Pi、DeepSeek 均未重跑；本轮也未重跑 Rust 整请求性能矩阵。** 因此只能确认上述重复操作已减少，不能据此更新与 Pi 的倍数、宣称整请求更快或写入更小。此前正式结果及全部不利数据不变。
