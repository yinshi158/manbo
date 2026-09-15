//! 学习数据同步：把数据目录里的学习成果在多台设备之间做三方合并。
//!
//! 职责边界：Core（[`manbo_core`]）永远不联网；本 crate 是唯一碰同步网络的地方，
//! 由各平台壳驱动——启动 / 退出跑全量合并，运行中借壳的落盘节拍跑学习表增量合并。
//! 后台线程与通道的用法照 [`manbo_predict`] 的既定模式：`submit` / `poll` 都不阻塞。
//!
//! 合并语义：每张表按「键 → 计数列」做三方合并 `merged = remote + (local − base)`，
//! `base` 是上次合并达成一致的内容（存 `<数据目录>/sync/base/`），所以同一份快照重复合并
//! 不会重复计数，删除与计数衰减会作为负增量传播到其他设备。

mod backend;
mod config;
mod cycle;
mod error;
mod merge;
mod registry;
mod service;
mod state;

pub use backend::{build_backend, GetOutcome, RemoteBlob, SyncBackend};
pub use config::{BackendKind, SyncConfig};
pub use error::SyncError;
pub use registry::{MergePolicy, SyncFile, SyncScope};
pub use service::{run_once, SyncService};
pub use state::{SyncReport, SyncState};
