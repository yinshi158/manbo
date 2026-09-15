//! 同步后端抽象：一个「存学习数据文件的地方」，取 / 存都带乐观并发版本号。
//!
//! 版本号语义（WebDAV 的 ETag；文件夹后端用 mtime+大小拼的串）：`get` 带上次见过的版本号，
//! 没变就返回 [`GetOutcome::Unchanged`] 省一次下载；`put` 带「我以为的版本号」，对不上说明
//! 另一台设备刚传过，返回 [`SyncError::Conflict`] 由 cycle 重取重并。

mod folder;
mod webdav;

use crate::config::{BackendKind, SyncConfig};
use crate::error::SyncError;

/// 远端的一份文件内容与版本号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBlob {
    /// 文件字节（TSV / TOML 都是 UTF-8 文本，按行容错解析）。
    pub data: Vec<u8>,

    /// 版本号；下次 `get` 带它判断 304 / `put` 带 it 做乐观并发。
    pub tag: String,

    /// 远端 mtime（毫秒），给整文件合并的「双方都改了」分支比较新旧；
    /// WebDAV 不解析 HTTP 日期就是 `None`（该分支退化为平手取本地）。
    pub mtime_ms: Option<u128>,
}

/// 取一个远端文件的三种结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GetOutcome {
    /// 远端没有这个文件（首次同步）。
    Missing,

    /// 版本号没变：远端内容与本地基准一致，直接拿基准当远端内容，省一次下载。
    Unchanged,

    /// 拿到了新内容。
    Fresh(RemoteBlob),
}

/// 同步后端。实现必须线程安全地被单个 worker 线程独占使用（`Send` 即可）。
pub trait SyncBackend: Send {
    /// 建好根目录 / 集合；已存在不算错。
    fn ensure_root(&mut self) -> Result<(), SyncError>;

    /// 取 `name`。`if_none_match` 是上次见过的版本号。
    fn get(&mut self, name: &str, if_none_match: Option<&str>) -> Result<GetOutcome, SyncError>;

    /// 存 `name`。`expect` 是调用方以为的当前版本号（首次上传传 `None`，要求远端也没有）；
    /// 对不上返回 [`SyncError::Conflict`]，成功返回新版本号。
    fn put(&mut self, name: &str, data: &[u8], expect: Option<&str>) -> Result<String, SyncError>;
}

/// 按配置建后端；配置缺项在这里报错，壳据此降级为不同步。
pub fn build_backend(config: &SyncConfig) -> Result<Box<dyn SyncBackend>, SyncError> {
    config.validate()?;
    match config.backend {
        BackendKind::Folder => Ok(Box::new(folder::FolderBackend::new(
            config.folder.clone().unwrap_or_default(),
        ))),
        BackendKind::Webdav => Ok(Box::new(webdav::WebdavBackend::new(config)?)),
    }
}
