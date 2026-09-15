//! 同步状态：每文件的远端版本号、设备标识、上次结果。
//!
//! 存 `<数据目录>/sync/state.json`，设置页直接读它显示「上次同步」；`sync/base/` 放每张表
//! 上次合并达成一致的基准快照，是三方合并的 `base` 一侧。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use manbo_core::storage::{read_text_lossy, write_atomic_str};
use serde::{Deserialize, Serialize};

/// 同步状态目录名（数据目录下）。
pub const SYNC_DIR: &str = "sync";

/// 基准快照目录名（`sync/` 下）。
pub const BASE_DIR: &str = "base";

/// 一轮同步的汇报：数字给日志，`messages` 给设置页 / 排查。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    /// 有实际变化（上传或改写了本地）的文件数。
    pub synced: usize,

    /// 三方一致不用动的文件数。
    pub skipped: usize,

    /// 失败的文件数（单文件失败不中断整轮）。
    pub errors: usize,

    /// 逐条中文短描述（失败的文件与其原因）。
    pub messages: Vec<String>,
}

impl SyncReport {
    /// 整轮没有任何失败。
    pub fn is_clean(&self) -> bool {
        self.errors == 0
    }
}

/// 同步状态（`sync/state.json`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncState {
    /// 本机标识：仅信息展示与冲突副本命名，不是密码学用途。
    #[serde(default)]
    pub device_id: String,

    /// 文件名 → 远端版本号（ETag / 文件夹的 mtime-大小 串）。
    #[serde(default)]
    pub etags: BTreeMap<String, String>,

    /// 上次整轮成功的时刻（本地时间 RFC3339）；设置页显示用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success: Option<String>,

    /// 上次失败的简述；设置页显示用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// `<数据目录>/sync/state.json`。
pub fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SYNC_DIR).join("state.json")
}

/// `<数据目录>/sync/base/<name>`。
pub fn base_path(data_dir: &Path, name: &str) -> PathBuf {
    data_dir.join(SYNC_DIR).join(BASE_DIR).join(name)
}

impl SyncState {
    /// 读状态；没有或坏掉就从零开始（版本号丢了最多是多下载一轮，不会错合并）。
    pub fn load(data_dir: &Path) -> Self {
        let path = state_path(data_dir);
        match read_text_lossy(&path) {
            Ok(Some(text)) => serde_json::from_str(&text).unwrap_or_else(|error| {
                tracing::warn!(path = %path.display(), %error, "同步状态解析失败，从零开始");
                Self::fresh()
            }),
            Ok(None) => Self::fresh(),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "同步状态读不了，从零开始");
                Self::fresh()
            }
        }
    }

    /// 新状态：生成一次设备标识后由 `save` 持久化。
    fn fresh() -> Self {
        Self {
            device_id: device_id(),
            ..Self::default()
        }
    }

    /// 原子写回；失败只记警告（下轮会再写）。
    pub fn save(&self, data_dir: &Path) {
        let path = state_path(data_dir);
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                tracing::warn!(path = %parent.display(), %error, "建同步状态目录失败");
                return;
            }
        }
        let text = match serde_json::to_string_pretty(self) {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!(%error, "同步状态序列化失败");
                return;
            }
        };
        if let Err(error) = write_atomic_str(&path, &text) {
            tracing::warn!(path = %path.display(), %error, "同步状态写不进去");
        }
    }

    /// 记一轮的结果：全干净记成功时刻，有失败记第一条简述。
    pub fn record_outcome(&mut self, report: &SyncReport) {
        if report.is_clean() {
            self.last_success = Some(now_text());
            self.last_error = None;
        } else {
            self.last_error = report.messages.first().cloned();
        }
    }
}

/// 设备标识：时间纳秒 ⊕ 进程号的 hex。进程号会复用、时钟会回拨，但它只用于展示与命名，够用。
fn device_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{:x}", nanos ^ (u128::from(std::process::id()) << 64 | nanos.rotate_left(33)))
}

/// 本地时刻的 RFC3339 文本。
fn now_text() -> String {
    jiff::Zoned::now().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_state_generates_a_persistent_device_id() {
        let dir = std::env::temp_dir().join(format!("manbo-sync-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = SyncState::load(&dir);
        assert!(!state.device_id.is_empty());
        state.save(&dir);
        let reloaded = SyncState::load(&dir);
        assert_eq!(reloaded.device_id, state.device_id, "设备标识要持久化");
        assert_eq!(reloaded.etags, state.etags);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outcome_updates_success_and_error_fields() {
        let mut state = SyncState::fresh();
        let mut report = SyncReport::default();
        report.synced = 3;
        state.record_outcome(&report);
        assert!(state.last_success.is_some());
        assert!(state.last_error.is_none());
        report.errors = 1;
        report.messages.push("user.tsv：HTTP request failed: 断网".to_owned());
        state.record_outcome(&report);
        assert_eq!(
            state.last_error.as_deref(),
            Some("user.tsv：HTTP request failed: 断网")
        );
    }
}
