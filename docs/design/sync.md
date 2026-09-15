# 学习数据同步

多台设备之间的学习数据合并同步：换设备不用重新调教，日常轮换用的两台机器互相补齐。
核心在 `crates/manbo-sync`，各平台壳只做接线；Core 不联网的边界不变。

## 决定与理由

**三方合并，不是整文件覆盖。** 学习数据六张表全是「键 → 计数」，可以逐键合并：

```
merged[键] = remote[键] + (local[键] − base[键])
```

`base` 是上次合并达成一致的内容，每台设备在 `<数据目录>/sync/base/` 各存一份。
由此得到三条免费性质：

- **幂等**：合并结果写回基准，同一份快照并多少遍都不变；
- **删除与衰减传播**：删候选清零、计数减半，都是相对基准的负增量，远端会跟着减；
- **两台独立设备首同步求和**是正确语义（各自真选了那么多次）。会重复计数的只有
  「只拷 TSV 文件到新设备再开同步」这一种迁移方式；整目录拷贝（连 `sync/` 一起）不会。
  用户文档写明推荐整目录迁移。

**运行中的同步只碰学习六表 + config.toml**（`SyncScope::Learning`）。引擎在内存里也持有
统计类文件（usage / vocab / glossary），运行中合并它们会被下次落盘盖掉；这些价值低，
只在启动（引擎还没装数据）与退出（最终 flush 之后）合并，重启自然收敛，
不为它们加更多 Engine API。

**`Engine::set_learner` 不先 flush 旧的。** 这是整个运行时同步最容易写错的地方：磁盘上
已经是合并后的新内容，旧内存再落一次盘就把合并结果盖掉了。约定壳在发起合并**之前**
flush（此时磁盘=内存），合并完成后再 `set_learner` 换上新数据。

**合并中途撞上 60 秒落盘节拍**：cycle 写完本地文件后重读一遍做校验，发现被改写就放弃
本轮的基准更新（远端已经推上去了，本地下一轮重并）。残余的竞态窗口在「cycle 完成 →
壳轮询到报告并热替换」之间约一秒，恰好又赶上 flush 节拍的代价是一次性的少量计数偏移，
输入优先于学习，接受。

**合并只能由持有 Engine 的进程做。** Windows 设置进程若直接改学习文件，会跟引擎内存打架；
所以设置页的「立即同步」在 Windows 写 `sync/request` 触发文件，macOS 菜单直接调进程内方法。

**网络实现放独立 crate**（与云联想同一条架构线）：`manbo-sync`，Core 永远不联网。
线程与通道模式照 `manbo-predict`：独立线程 + mpsc、current_thread runtime、
失败 warn 降级、请求不阻塞输入。

## 后端

`SyncBackend` trait：`get(name, if_none_match)` / `put(name, data, expect)`，乐观并发
（`expect` 对不上返回 Conflict，cycle 重取重并，≤3 次）。两个实现：

- **WebDAV**：坚果云 / Nextcloud / NAS。只用 GET / PUT / MKCOL 三个动词，不做 PROPFIND
  （不引 XML）；版本号是 ETag，服务器不回 ETag 就用「字节数 + Last-Modified」兜底。
  缺省地址给了坚果云示例，应用密码走配置或 `MANBO_SYNC_PASSWORD`。
  建目录是**逐段 MKCOL**（`/dav/manbo` 先建 `/dav` 再建 `/manbo`），**403 与 405 都当「这个集合已经有了」**：
  坚果云对自家固定根路径 `/dav` 的 MKCOL 恒回 403（2026-09-15 实测；同一凭据下 `/dav/manbo` 的 MKCOL 201、
  PUT 201、GET 200、DELETE 文件 204 都正常），只放行 405 会让**每一轮同步都在第一段放弃**——远端一个文件都不会有，
  本机 `sync/base/` 也永远空着，症状只是「同步文件夹是空的」。凭据不对是 401、真没权限会在随后的 GET / PUT 上报出来，
  都不会被这一步吞掉。
- **folder**：一个本地目录当「云」，交给 iCloud Drive / Dropbox / Syncthing / 坚果云同步盘
  传输。版本号 = mtime 毫秒 + 字节数。也是集成测试与 CLI（`manbo-cli --sync <云目录>`）
  的后端——CI 里不碰网络就能测完整合并流程。

**不做**：账号体系与自建服务器；端到端加密（同步的是用户造词与计数，不含输入日志原文，
但 WebDAV 服务商可见 TSV 明文——用户文档写明，整个功能 opt-in）。

## 同步清单

| 文件 | 策略 | 时机 |
|---|---|---|
| `user.tsv` / `user-ngram.tsv` / `user-choices.tsv` / `user-english.tsv` / `user-typos.tsv` | 计数 | 启动 / 运行 / 退出 |
| `user-words.tsv` | 计数 + 词频 100 封顶（两台各自新增同一个词不翻倍） | 同上 |
| `config.toml` | 整文件三方：单方改取那方；双方都改取 mtime 新的，输方留 `config.conflict-*.toml` | 同上；落盘后壳的热加载自动生效 |
| `usage.tsv` | 按天多列计数，键 = 日期列 | 启动 / 退出 |
| `user-vocab.tsv` | 三列计数求和，首见取早、末见取晚 | 启动 / 退出 |
| `user-glossary-<lang>.tsv` | 键值并集，本地手改优先，删除传播 | 启动 / 退出 |

永不上传：`input-log.jsonl`（上屏原文 + 应用名）、`.env`（密钥）、`dicts/`、`model/`、`logs/`。

## 各端接线

- **Windows**：`main.rs` 装配引擎前 `run_once(Full)`（限时，失败不挡启动）；`Router::tick`
  里 `tick_sync`——到点先 `flush_learning` 再请求 Learning，报告到了且空闲就
  `set_learner` 重建；`serve_pipe` 退出后最终 flush + `run_once(Full)`；`[sync]` 热加载
  重接服务；设置页「同步」写配置 + 触发文件 + 读 `sync/state.json` 显示状态。tsf DLL 不动。
- **macOS**：`host::init` 里同款启动同步；`Host::tick`（秒级定时器）同款节拍；
  `deactivateServer` 落盘后请求一轮 Learning（进程常驻，后台线程总能跑完，无 terminate 钩子
  也能把数据推上去）；菜单「立即同步」；`[sync]` 热加载重接。偏好设置窗口的同步分区待加，
  配置先走 config.toml。
- **CLI**：`manbo-cli --sync <云目录> --data-dir <设备目录>`，离线验证合并与手动同步工具。

失败退避：连续失败自动同步间隔 ×2，封顶 ×8，成功或手动同步归一。
状态（设备 id、每文件版本号、上次成功 / 失败）在 `sync/state.json`，设置页直接读。
