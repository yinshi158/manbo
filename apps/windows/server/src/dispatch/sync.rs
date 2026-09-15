//! 学习数据同步的接线：把 [`SyncService`] 挂到 Router 的空闲节拍上——到点先落盘再请求增量同步，
//! 报告到了且空闲就把合并后的学习表热替换进引擎；设置页的「立即同步」走 `sync/request` 触发文件
//! （合并只能由持有 Engine 的 Server 进程做，设置进程直接改文件会跟引擎内存打架）。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use manbo_learning::FrequencyLearner;
use manbo_sync::{SyncConfig, SyncScope, SyncService};

use super::Router;

/// 触发文件：`<数据目录>/sync/request`，设置页写一个空文件表示「立即同步」。
const REQUEST_FILE: (&str, &str) = ("sync", "request");

/// 失败后的最大退避倍数（间隔 × 8）。
const MAX_BACKOFF: u32 = 8;

/// Router 持有的同步进行态。
pub(super) struct RouterSync {
    /// 后台同步线程。
    service: SyncService,

    /// 用户数据目录（学习表与触发文件都在这里）。
    user_dir: PathBuf,

    /// 自动同步的间隔。
    interval: Duration,

    /// 下次自动同步的时刻。
    next_due: Instant,

    /// 失败退避倍数：连续失败逐轮翻倍，成功归一。
    backoff: u32,
}

impl RouterSync {
    fn new(service: SyncService, user_dir: PathBuf, interval_minutes: u64) -> Self {
        let interval = Duration::from_secs(interval_minutes.max(1) * 60);
        Self {
            service,
            user_dir,
            interval,
            // Server 启动前已跑过一轮全量，这里先等一个间隔
            next_due: Instant::now() + interval,
            backoff: 1,
        }
    }
}

impl Router {
    /// 按 `[sync]` 接同步；关着或配置缺项就退回不同步。启动与热加载共用。
    pub fn attach_sync(&mut self, config: &SyncConfig, user_dir: Option<&Path>) {
        self.sync = None;
        if !config.enabled {
            tracing::info!("学习数据同步未开启（[sync] enabled = false）");
            return;
        }
        let Some(user_dir) = user_dir else {
            tracing::warn!("学习数据同步未启用：拿不到用户数据目录");
            return;
        };
        match SyncService::start(config, user_dir.to_owned()) {
            Ok(service) => {
                self.sync = Some(RouterSync::new(
                    service,
                    user_dir.to_owned(),
                    config.interval_minutes,
                ));
            }
            Err(error) => tracing::warn!(%error, "学习数据同步未启用（配置缺项？）"),
        }
    }

    /// 空闲节拍上叫：收报告、看触发文件、到点发起同步。组句 / 翻译中整个跳过——
    /// 请求晚一拍没关系，报告的处理（热替换学习器）不能被打断。
    pub(super) fn tick_sync(&mut self) {
        if self.sync.is_none() {
            return;
        }
        if self.composed.is_some() || self.translation.is_some() {
            return;
        }
        // 1) 上一轮的报告：干净就热替换学习器（下一轮的增量以此为基），失败就翻倍退避
        let report = self.sync.as_ref().and_then(|sync| sync.service.poll());
        if let Some(report) = report {
            if report.is_clean() {
                if let Some(sync) = self.sync.as_mut() {
                    sync.backoff = 1;
                }
                let user_dir = self.sync.as_ref().map(|sync| sync.user_dir.clone());
                if let Some(user_dir) = user_dir {
                    self.reload_learner(&user_dir);
                }
            } else {
                if let Some(sync) = self.sync.as_mut() {
                    sync.backoff = (sync.backoff * 2).min(MAX_BACKOFF);
                }
                tracing::warn!(
                    errors = report.errors,
                    first = report.messages.first().map(String::as_str).unwrap_or(""),
                    "同步有失败，放慢节奏"
                );
            }
        }
        // 2) 设置页的「立即同步」触发文件
        let mut manual = false;
        if let Some(sync) = self.sync.as_ref() {
            let request_file = sync.user_dir.join(REQUEST_FILE.0).join(REQUEST_FILE.1);
            if request_file.exists() {
                manual = std::fs::remove_file(&request_file).is_ok();
            }
        }
        // 3) 到点（或手动）：先落盘（磁盘=内存），再交给后台线程去合并与上传
        let due = self
            .sync
            .as_ref()
            .is_some_and(|sync| Instant::now() >= sync.next_due);
        if manual || due {
            let scope = if manual {
                SyncScope::Full
            } else {
                SyncScope::Learning
            };
            self.flush_learning();
            if let Some(sync) = self.sync.as_mut() {
                sync.service.request(scope);
                sync.next_due = Instant::now() + sync.interval * sync.backoff;
            }
            tracing::info!(?scope, manual, "发起学习数据同步");
        }
    }

    /// 从磁盘重建学习器换进引擎（磁盘上刚是合并后的内容；核心约定：`set_learner` 不 flush 旧的）。
    fn reload_learner(&mut self, user_dir: &Path) {
        match FrequencyLearner::from_path(user_dir.join("user.tsv")) {
            Ok(learner) => {
                self.engine.set_learner(Box::new(learner));
                tracing::info!("学习数据已按同步结果热替换");
            }
            Err(error) => {
                // 读失败保持内存里的学习数据：文件真坏了的话，下轮同步前还有落盘节拍兜底
                tracing::error!(%error, "同步后的学习数据读不回来，本轮沿用内存数据");
            }
        }
    }
}
