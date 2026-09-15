//! 后台同步线程：收命令、跑一轮、回报告。壳在自己的节拍上 [`SyncService::request`] /
//! [`SyncService::poll`]，都不阻塞输入；线程模式照 [`manbo_predict`] 的 worker。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use crate::backend::{build_backend, SyncBackend};
use crate::config::SyncConfig;
use crate::cycle::run_cycle;
use crate::error::SyncError;
use crate::registry::SyncScope;
use crate::state::{SyncReport, SyncState};

/// worker 的命令：目前只有「跑一轮」；关闭靠壳 drop 掉发送端，`recv` 出错自然退出。
enum Command {
    /// 跑一轮指定范围的同步。
    Sync(SyncScope),
}

/// 常驻的同步服务。缺配置（地址 / 目录没填）在 `start` 就报错，壳降级为不同步并记日志。
pub struct SyncService {
    /// 往 worker 发命令。
    commands: Sender<Command>,

    /// 从 worker 收报告。
    reports: Receiver<SyncReport>,
}

impl SyncService {
    /// 起后台线程。调用方先确认 `config.enabled`。
    pub fn start(config: &SyncConfig, data_dir: PathBuf) -> Result<Self, SyncError> {
        let backend = build_backend(config)?;
        let (commands, command_rx) = mpsc::channel::<Command>();
        let (report_tx, reports) = mpsc::channel::<SyncReport>();
        std::thread::Builder::new()
            .name("manbo-sync".to_owned())
            .spawn(move || worker(command_rx, report_tx, backend, data_dir))
            .map_err(SyncError::Runtime)?;
        tracing::info!(
            backend = config.backend.key(),
            interval_minutes = config.interval_minutes,
            "学习数据同步已启用"
        );
        Ok(Self { commands, reports })
    }

    /// 请求一轮同步；不阻塞。连续多条被 worker 合成一条（有全量就全量）。
    pub fn request(&self, scope: SyncScope) {
        if self.commands.send(Command::Sync(scope)).is_err() {
            tracing::warn!("同步线程已退出，请求被丢弃");
        }
    }

    /// 取上一轮的报告；没有就 `None`（壳在下一拍再取）。
    pub fn poll(&self) -> Option<SyncReport> {
        self.reports.try_recv().ok()
    }
}

/// worker 主循环：一条命令一轮同步，报告发回壳。状态（版本号 / 上次结果）由 [`run_cycle`] 收尾。
fn worker(
    commands: Receiver<Command>,
    reports: Sender<SyncReport>,
    mut backend: Box<dyn SyncBackend>,
    data_dir: PathBuf,
) {
    while let Ok(command) = commands.recv() {
        let Some(scope) = coalesce(&commands, command) else {
            return;
        };
        let mut state = SyncState::load(&data_dir);
        let report = run_cycle(backend.as_mut(), &data_dir, scope, &mut state);
        if reports.send(report).is_err() {
            return; // 壳已经不在了
        }
    }
}

/// 连续排队的命令合成一条：只要有全量就跑全量。
fn coalesce(commands: &Receiver<Command>, first: Command) -> Option<SyncScope> {
    let mut scope = match first {
        Command::Sync(scope) => scope,
    };
    loop {
        match commands.try_recv() {
            Ok(Command::Sync(next)) => match (scope, next) {
                (SyncScope::Full, _) | (_, SyncScope::Full) => scope = SyncScope::Full,
                _ => scope = next,
            },
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return Some(scope),
        }
    }
}

/// 一次性同步（启动 / 退出用）：独立线程跑一轮，限时等结果。
///
/// 超时就先返回 [`SyncError::Timeout`] 让壳继续走（输入优先于学习），线程继续把这一轮写完
/// ——全部是原子写，基准在下一轮对齐。与 [`SyncService`] 别同时跑：启动在线程起来之前、
/// 退出在它停了之后，本来就不重叠。
pub fn run_once(
    config: &SyncConfig,
    data_dir: &Path,
    scope: SyncScope,
    timeout_ms: u64,
) -> Result<SyncReport, SyncError> {
    let backend = build_backend(config)?;
    let data_dir = data_dir.to_owned();
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("manbo-sync-once".to_owned())
        .spawn(move || {
            let mut state = SyncState::load(&data_dir);
            let mut backend = backend;
            let report = run_cycle(backend.as_mut(), &data_dir, scope, &mut state);
            let _ = done_tx.send(report);
        })
        .map_err(SyncError::Runtime)?;
    match done_rx.recv_timeout(Duration::from_millis(timeout_ms)) {
        Ok(report) => Ok(report),
        Err(_) => {
            tracing::warn!(timeout_ms, "同步未在限时内完成，先继续；线程会写完，下一轮收敛");
            Err(SyncError::Timeout(timeout_ms))
        }
    }
}
