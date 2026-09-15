//! 学习数据同步：把 [`SyncService`] 挂到 Host 的秒级节拍上——到点先落盘再请求增量同步，
//! 报告到了且没在组句就把合并后的学习表热替换进引擎。菜单「立即同步」走全量；
//! 停用输入法（切走输入源）时在落盘之后顺带请求一轮，进程由 launchd 常驻，后台线程总能跑完。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use manbo_learning::FrequencyLearner;
use manbo_sync::{SyncConfig, SyncScope, SyncService};

use super::*;

/// 失败后的最大退避倍数（间隔 × 8）。
const MAX_BACKOFF: u32 = 8;

/// Host 持有的同步进行态。
pub(super) struct Runner {
    /// 后台同步线程。
    service: SyncService,

    /// 用户数据目录（学习表都在这里）。
    user_dir: PathBuf,

    /// 自动同步的间隔。
    interval: Duration,

    /// 下次自动同步的时刻。
    next_due: Instant,

    /// 失败退避倍数：连续失败逐轮翻倍，成功归一。
    backoff: u32,
}

impl Runner {
    fn new(service: SyncService, user_dir: PathBuf, interval_minutes: u64) -> Self {
        let interval = Duration::from_secs(interval_minutes.max(1) * 60);
        Self {
            service,
            user_dir,
            interval,
            // 进程启动时已跑过一轮全量，这里先等一个间隔
            next_due: Instant::now() + interval,
            backoff: 1,
        }
    }
}

impl Host {
    /// 按 `[sync]` 接同步；关着或配置缺项就退回不同步。启动与热加载共用。
    pub(super) fn attach_sync(&mut self, config: &SyncConfig) {
        self.sync = None;
        if !config.enabled {
            tracing::info!("学习数据同步未开启（[sync] enabled = false）");
            return;
        }
        let Some(user_dir) = paths::user_data_dir() else {
            tracing::warn!("学习数据同步未启用：拿不到用户数据目录");
            return;
        };
        match SyncService::start(config, user_dir.clone()) {
            Ok(service) => {
                self.sync = Some(Runner::new(service, user_dir, config.interval_minutes));
            }
            Err(error) => tracing::warn!(%error, "学习数据同步未启用（配置缺项？）"),
        }
    }

    /// 菜单「立即同步」：落盘后请求一轮全量。
    pub fn sync_now(&mut self) {
        if self.sync.is_none() {
            tracing::info!("同步未开启，忽略「立即同步」");
            return;
        }
        self.engine.flush_learning();
        if let Some(runner) = self.sync.as_mut() {
            runner.service.request(SyncScope::Full);
            runner.next_due = Instant::now() + runner.interval * runner.backoff;
        }
    }

    /// 停用输入法时调：落盘已经做了，这里只请求一轮增量，把这一程的输入推上去。
    /// 进程不退出，后台线程总能跑完；报告到了 `tick_sync` 再处理。
    pub fn sync_after_deactivate(&mut self) {
        if let Some(runner) = self.sync.as_ref() {
            runner.service.request(SyncScope::Learning);
        }
    }

    /// 秒级节拍上的同步事务：收报告、到点发增量。组句 / 翻译中整个跳过——
    /// 请求晚一拍没关系，报告的处理（热替换学习器）不能被打断。
    pub(super) fn tick_sync(&mut self) {
        if self.sync.is_none()
            || self.translation.is_some()
            || !self.engine.composition().is_empty()
        {
            return;
        }
        // 1) 上一轮的报告：干净就热替换学习器，失败就翻倍退避
        let report = self.sync.as_ref().and_then(|runner| runner.service.poll());
        if let Some(report) = report {
            if report.is_clean() {
                if let Some(runner) = self.sync.as_mut() {
                    runner.backoff = 1;
                }
                let user_dir = self.sync.as_ref().map(|runner| runner.user_dir.clone());
                if let Some(user_dir) = user_dir {
                    self.reload_learner(&user_dir);
                }
            } else {
                if let Some(runner) = self.sync.as_mut() {
                    runner.backoff = (runner.backoff * 2).min(MAX_BACKOFF);
                }
                tracing::warn!(
                    errors = report.errors,
                    first = report.messages.first().map(String::as_str).unwrap_or(""),
                    "同步有失败，放慢节奏"
                );
            }
        }
        // 2) 到点：先落盘（磁盘=内存），再交给后台线程去合并与上传
        let due = self
            .sync
            .as_ref()
            .is_some_and(|runner| Instant::now() >= runner.next_due);
        if due {
            self.engine.flush_learning();
            if let Some(runner) = self.sync.as_mut() {
                runner.service.request(SyncScope::Learning);
                runner.next_due = Instant::now() + runner.interval * runner.backoff;
            }
            tracing::info!("发起学习数据同步");
        }
    }

    /// 从磁盘重建学习器换进引擎（磁盘上刚是合并后的内容；核心约定：`set_learner` 不 flush 旧的）。
    fn reload_learner(&mut self, user_dir: &PathBuf) {
        match FrequencyLearner::from_path(user_dir.join("user.tsv")) {
            Ok(learner) => {
                self.engine.set_learner(Box::new(learner));
                tracing::info!("学习数据已按同步结果热替换");
            }
            Err(error) => {
                tracing::error!(%error, "同步后的学习数据读不回来，本轮沿用内存数据");
            }
        }
    }
}
