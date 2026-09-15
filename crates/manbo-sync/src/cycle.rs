//! 一轮同步的编排：对登记表里的每个文件「取远端 → 三方合并 → 落本地 → 推远端 → 记基准」。
//!
//! 失败语义：单文件失败记进报告继续下一个（输入法不能被同步拖垮）；整轮结束才写状态。
//! 落盘校验：写基准前重读本地文件，发现被并发改写（比如撞上了壳的 60 秒落盘节拍）就放弃
//! 本轮的基准更新——远端已经推上去了，本地下一轮重并，数据不会丢。

use std::path::Path;

use manbo_core::storage::{read_text_lossy, write_atomic_str};

use crate::backend::{GetOutcome, SyncBackend};
use crate::error::SyncError;
use crate::merge::{merge, merge_whole_file, WholeFileOutcome};
use crate::registry::{MergePolicy, SyncFile, SyncScope};
use crate::state::{base_path, SyncReport, SyncState};

/// 上传撞车（412）最多重取重并几次。
const CONFLICT_RETRIES: usize = 3;

/// 跑一轮同步，返回汇报。状态（版本号 / 上次结果）在这里收尾写盘。
pub fn run_cycle(
    backend: &mut dyn SyncBackend,
    data_dir: &Path,
    scope: SyncScope,
    state: &mut SyncState,
) -> SyncReport {
    let mut report = SyncReport::default();
    if let Err(error) = backend.ensure_root() {
        tracing::warn!(%error, "同步：建远端根目录失败，本轮放弃");
        report.errors = 1;
        report.messages.push(format!("建远端根目录失败：{error}"));
        state.record_outcome(&report);
        state.save(data_dir);
        return report;
    }
    for file in crate::registry::resolve(data_dir, scope) {
        match sync_one(backend, data_dir, &file, state) {
            Ok(true) => report.synced += 1,
            Ok(false) => report.skipped += 1,
            Err(error) => {
                report.errors += 1;
                tracing::warn!(file = %file.name, %error, "同步：文件处理失败");
                report.messages.push(format!("{}：{error}", file.name));
            }
        }
    }
    state.record_outcome(&report);
    state.save(data_dir);
    report
}

/// 合并并落盘一个文件；`Ok(true)` 有实际动作（写本地或推远端），`Ok(false)` 三方一致没动。
fn sync_one(
    backend: &mut dyn SyncBackend,
    data_dir: &Path,
    file: &SyncFile,
    state: &mut SyncState,
) -> Result<bool, SyncError> {
    let local_path = data_dir.join(&file.name);
    let local = read_local(&local_path)?;
    let base = read_base(data_dir, &file.name);

    let mut outcome = backend.get(&file.name, state.etags.get(&file.name).map(String::as_str))?;
    // 版本号说没变、本地却连基准都没有（状态文件被清过）：退回全量取一次
    if outcome == GetOutcome::Unchanged && base.is_none() {
        outcome = backend.get(&file.name, None)?;
    }
    let (mut remote, mut expect, remote_mtime_ms) = match outcome {
        GetOutcome::Missing => (None, None, None),
        // 版本号没变 ⇒ 远端内容就是基准，省一次下载
        GetOutcome::Unchanged => (base.clone(), state.etags.get(&file.name).cloned(), None),
        GetOutcome::Fresh(blob) => (
            Some(String::from_utf8_lossy(&blob.data).into_owned()),
            Some(blob.tag),
            blob.mtime_ms,
        ),
    };

    if file.policy == MergePolicy::WholeFile {
        return sync_whole_file(
            backend,
            data_dir,
            file,
            state,
            base.as_deref(),
            local.as_deref(),
            remote.as_deref(),
            remote_mtime_ms,
            expect,
        );
    }

    // 行表：合并 → （有本地变化就落盘）→（与远端不同就上传，撞车重并）→ 校验后记基准
    let mut attempts = 0;
    loop {
        let merged = merge(file.policy, base.as_deref(), local.as_deref(), remote.as_deref());
        let changed_locally = differs_from(&merged, local.as_deref());
        let changed_remotely = differs_from(&merged, remote.as_deref());
        if !changed_locally && !changed_remotely {
            ensure_base(data_dir, file, &merged, base.is_none());
            return Ok(false);
        }
        let mut tag = expect.clone();
        if changed_remotely {
            match backend.put(&file.name, merged.as_bytes(), expect.as_deref()) {
                Ok(new_tag) => tag = Some(new_tag),
                Err(SyncError::Conflict(_)) if attempts < CONFLICT_RETRIES => {
                    attempts += 1;
                    (remote, expect, _) = refetch(backend, &file.name)?;
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        if changed_locally {
            write_atomic_str(&local_path, &merged).map_err(|source| SyncError::Io {
                path: local_path.clone(),
                source,
            })?;
        }
        verify_and_store_base(data_dir, file, state, &local_path, &merged, tag)?;
        return Ok(true);
    }
}

/// config.toml 这类整文件：按 [`merge_whole_file`] 的分支落盘 / 上传，输方写冲突副本。
fn sync_whole_file(
    backend: &mut dyn SyncBackend,
    data_dir: &Path,
    file: &SyncFile,
    state: &mut SyncState,
    base: Option<&str>,
    local: Option<&str>,
    remote: Option<&str>,
    remote_mtime_ms: Option<u128>,
    expect: Option<String>,
) -> Result<bool, SyncError> {
    let local_path = data_dir.join(&file.name);
    let local_mtime_ms = std::fs::metadata(&local_path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis());
    let outcome = merge_whole_file(base, local, remote, local_mtime_ms, remote_mtime_ms);
    let merged = match outcome {
        WholeFileOutcome::Unchanged => {
            ensure_base(data_dir, file, local.unwrap_or(""), base.is_none());
            return Ok(false);
        }
        WholeFileOutcome::TakeLocal => local.unwrap_or("").to_owned(),
        WholeFileOutcome::TakeRemote => remote.unwrap_or("").to_owned(),
        WholeFileOutcome::Split {
            winner,
            winner_is_local,
            loser,
        } => {
            let copy = data_dir.join(conflict_copy_name(&file.name));
            tracing::info!(file = %file.name, copy = %copy.display(), "同步：配置两边都改过，输的一方留冲突副本");
            write_atomic_str(&copy, &loser).map_err(|source| SyncError::Io {
                path: copy.clone(),
                source,
            })?;
            // 赢家是本地就只推远端；是远端就等下面统一写本地
            let _ = winner_is_local;
            winner
        }
    };
    let changed_locally = differs_from(&merged, local);
    if changed_locally {
        write_atomic_str(&local_path, &merged).map_err(|source| SyncError::Io {
            path: local_path.clone(),
            source,
        })?;
    }
    let mut tag = expect;
    if differs_from(&merged, remote) {
        // 整文件没法逐键重并：上传撞车就放弃本轮，下轮拿到对方的新内容再合
        match backend.put(&file.name, merged.as_bytes(), tag.as_deref()) {
            Ok(new_tag) => tag = Some(new_tag),
            Err(SyncError::Conflict(_)) => {
                tracing::info!(file = %file.name, "同步：整文件上传撞车，本轮放弃，下轮重并");
                return Ok(true);
            }
            Err(error) => return Err(error),
        }
    }
    if !verify_and_store_base(data_dir, file, state, &local_path, &merged, tag)? {
        return Ok(true);
    }
    Ok(true)
}

/// 落盘校验并写基准：重读本地与合并结果一致才更新基准与版本号；
/// 被并发改写（落盘节拍撞上）就跳过基准，下一轮重并。返回是否记了基准。
fn verify_and_store_base(
    data_dir: &Path,
    file: &SyncFile,
    state: &mut SyncState,
    local_path: &Path,
    merged: &str,
    tag: Option<String>,
) -> Result<bool, SyncError> {
    let after = read_local(local_path)?;
    if after.as_deref() != Some(merged) {
        tracing::warn!(
            file = %file.name,
            "同步：本地文件在合并期间又被改写（可能是落盘节拍），基准下轮再对齐"
        );
        return Ok(false);
    }
    let base = base_path(data_dir, &file.name);
    if let Some(parent) = base.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SyncError::Io {
            path: parent.to_owned(),
            source,
        })?;
    }
    write_atomic_str(&base, merged).map_err(|source| SyncError::Io {
        path: base.clone(),
        source,
    })?;
    if let Some(tag) = tag {
        state.etags.insert(file.name.clone(), tag);
    }
    Ok(true)
}

/// 三方一致但还没有基准（首同步两边都空）：补一份空基准，之后就有版本号可用了。
fn ensure_base(data_dir: &Path, file: &SyncFile, content: &str, base_missing: bool) {
    if base_missing {
        let base = base_path(data_dir, &file.name);
        if let Some(parent) = base.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = write_atomic_str(&base, content);
    }
}

/// 撞车后重取远端；拿不到内容（刚被删了）就按没有远端重并。
fn refetch(
    backend: &mut dyn SyncBackend,
    name: &str,
) -> Result<(Option<String>, Option<String>, Option<u128>), SyncError> {
    match backend.get(name, None)? {
        GetOutcome::Fresh(blob) => {
            let text = String::from_utf8_lossy(&blob.data).into_owned();
            Ok((Some(text), Some(blob.tag), blob.mtime_ms))
        }
        GetOutcome::Missing => Ok((None, None, None)),
        GetOutcome::Unchanged => Ok((None, None, None)),
    }
}

/// 合并结果是否与一侧不同；「合并为空而该侧本就不存在」不算不同（不造空文件）。
fn differs_from(merged: &str, side: Option<&str>) -> bool {
    merged != side.unwrap_or("") && !(merged.is_empty() && side.is_none())
}

fn read_local(path: &Path) -> Result<Option<String>, SyncError> {
    read_text_lossy(path).map_err(|source| SyncError::Io {
        path: path.to_owned(),
        source,
    })
}

/// 基准读不了（权限等）当没有基准：最多是多并一轮，不该拖垮同步。
fn read_base(data_dir: &Path, name: &str) -> Option<String> {
    match read_text_lossy(&base_path(data_dir, name)) {
        Ok(base) => base,
        Err(error) => {
            tracing::warn!(file = name, %error, "同步：基准快照读不了，按没有基准处理");
            None
        }
    }
}

/// 冲突副本名：`config.conflict-20260914-153045-1234.toml`。
fn conflict_copy_name(name: &str) -> String {
    let stem = name.strip_suffix(".toml").unwrap_or(name);
    let stamp = jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string();
    format!("{stem}.conflict-{stamp}-{}.toml", std::process::id())
}
