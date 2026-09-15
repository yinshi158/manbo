//! 同步的错误类型。文案用英文（`thiserror` 约定），日志与设置页展示由壳用中文描述。

use std::path::PathBuf;

use thiserror::Error;

/// 同步过程中的失败。单文件失败不中断整轮（见 [`crate::cycle`]），只有整个后端起不来才向壳报错。
#[derive(Debug, Error)]
pub enum SyncError {
    /// `[sync] enabled = true` 但 WebDAV 地址没填。
    #[error("sync is enabled but the WebDAV url is not configured")]
    MissingUrl,

    /// `[sync] backend = "folder"` 但同步目录没填。
    #[error("sync is enabled but the folder is not configured")]
    MissingFolder,

    /// 远端文件在合并期间被另一台设备改写，重试次数用完。
    #[error("remote file {0} kept changing during the sync")]
    Conflict(String),

    /// 启动 / 退出的限时同步没有在预算内完成（后台线程会继续写完，下轮收敛）。
    #[error("sync did not finish within {0} ms")]
    Timeout(u64),

    /// 本地文件读写失败（权限、坏盘）。
    #[error("I/O error on {path}: {source}")]
    Io {
        /// 出错的文件。
        path: PathBuf,

        /// 底层 io 错误。
        #[source]
        source: std::io::Error,
    },

    /// 网络请求失败（DNS、断网、TLS、5xx）。
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// 远端返回了预期之外的状态码。
    #[error("unexpected HTTP status {0}")]
    HttpStatus(u16),

    /// 后台线程建异步运行时失败。
    #[error("async runtime failed: {0}")]
    Runtime(#[source] std::io::Error),
}
