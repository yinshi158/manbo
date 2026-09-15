//! 同步的验证入口：拿一个目录当「云」（folder 后端）跑一轮全量同步，打印报告后退出。
//!
//! 典型用法（模拟两台设备交替输入）：
//!
//! ```text
//! manbo-cli --sync ./cloud --data-dir ./device-a
//! manbo-cli --sync ./cloud --data-dir ./device-b
//! manbo-cli --sync ./cloud --data-dir ./device-a   # A 拿到 B 的
//! ```

use std::path::Path;

use manbo_sync::{run_once, BackendKind, SyncConfig, SyncScope};

/// 跑一轮全量同步并产出人类可读的报告文本。
pub fn run(cloud: &Path, data_dir: Option<&Path>) -> Result<String, manbo_sync::SyncError> {
    let data_dir =
        data_dir.unwrap_or_else(|| Path::new("manbo-data")).to_owned();
    let config = SyncConfig {
        enabled: true,
        backend: BackendKind::Folder,
        folder: Some(cloud.to_owned()),
        startup_timeout_ms: 30_000,
        ..SyncConfig::default()
    };
    let report = run_once(&config, &data_dir, SyncScope::Full, 30_000)?;
    Ok(format!(
        "同步完成：{} 个文件有变化，{} 个一致，{} 个失败{}\n设备目录：{}\n云端目录：{}\n",
        report.synced,
        report.skipped,
        report.errors,
        report
            .messages
            .iter()
            .map(|message| format!("\n  - {message}"))
            .collect::<String>(),
        data_dir.display(),
        cloud.display(),
    ))
}
