//! 整文件三方合并：`config.toml` 这类没有键值结构的文件。
//!
//! 文件级三分支：只有本机改了取本机、只有远端改了取远端；双方都改了取 mtime 新的一方
//! （mtime 缺失当 0，平手取本地），输的一方原样落成冲突副本，绝不静默丢配置。
//! 注意跨设备时钟可能有偏差，mtime 只用在「双方都改了」这个罕见分支里兜底。

/// 整文件合并的结果，落盘动作由 [`crate::cycle`] 执行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WholeFileOutcome {
    /// 三方一致，什么都不用动。
    Unchanged,

    /// 取本地内容（远端还停留在基准，本地要推上去）。
    TakeLocal,

    /// 取远端内容（本地要重写成远端的样子）。
    TakeRemote,

    /// 双方都改了：取 mtime 新的一方，输方内容由 cycle 落成冲突副本。
    Split {
        /// 赢家的完整内容。
        winner: String,

        /// 赢家是不是本地（决定 cycle 是只上传还是要重写本地）。
        winner_is_local: bool,

        /// 输家的完整内容（写进冲突副本）。
        loser: String,
    },
}

/// `None` 按空文件处理（首次同步任何一侧都可能没有文件）。
fn fn_text(side: Option<&str>) -> &str {
    side.unwrap_or("")
}

pub(crate) fn merge_whole_file(
    base: Option<&str>,
    local: Option<&str>,
    remote: Option<&str>,
    local_mtime_ms: Option<u128>,
    remote_mtime_ms: Option<u128>,
) -> WholeFileOutcome {
    let text = fn_text;
    let base = text(base);
    let local = text(local);
    let remote = text(remote);
    let local_changed = local != base;
    let remote_changed = remote != base;
    match (local_changed, remote_changed) {
        (false, false) => WholeFileOutcome::Unchanged,
        (true, false) => WholeFileOutcome::TakeLocal,
        (false, true) => WholeFileOutcome::TakeRemote,
        (true, true) => {
            let local_time = local_mtime_ms.unwrap_or(0);
            let remote_time = remote_mtime_ms.unwrap_or(0);
            if local_time >= remote_time {
                WholeFileOutcome::Split {
                    winner: local.to_owned(),
                    winner_is_local: true,
                    loser: remote.to_owned(),
                }
            } else {
                WholeFileOutcome::Split {
                    winner: remote.to_owned(),
                    winner_is_local: false,
                    loser: local.to_owned(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "[general]\npage_size = 9\n";
    const LOCAL: &str = "[general]\npage_size = 5\n";
    const REMOTE: &str = "[general]\ntheme = \"dark\"\n";

    #[test]
    fn unchanged_when_neither_side_moved() {
        assert_eq!(
            merge_whole_file(Some(BASE), Some(BASE), Some(BASE), None, None),
            WholeFileOutcome::Unchanged
        );
        // 首同步三方全空也一样
        assert_eq!(merge_whole_file(None, None, None, None, None), WholeFileOutcome::Unchanged);
    }

    #[test]
    fn one_sided_changes_win_cleanly() {
        assert_eq!(
            merge_whole_file(Some(BASE), Some(LOCAL), Some(BASE), None, None),
            WholeFileOutcome::TakeLocal
        );
        assert_eq!(
            merge_whole_file(Some(BASE), Some(BASE), Some(REMOTE), None, None),
            WholeFileOutcome::TakeRemote
        );
    }

    #[test]
    fn missing_side_counts_as_unchanged_empty() {
        // 本机没有文件、基准也没有（全新设备）：远端有就取远端
        assert_eq!(
            merge_whole_file(None, None, Some(REMOTE), None, None),
            WholeFileOutcome::TakeRemote
        );
    }

    #[test]
    fn both_changed_picks_the_newer_mtime_and_keeps_the_loser() {
        let outcome = merge_whole_file(
            Some(BASE),
            Some(LOCAL),
            Some(REMOTE),
            Some(1_000),
            Some(2_000),
        );
        assert_eq!(
            outcome,
            WholeFileOutcome::Split {
                winner: REMOTE.to_owned(),
                winner_is_local: false,
                loser: LOCAL.to_owned(),
            }
        );
        // 平手取本地，输方（远端内容）留副本
        let outcome = merge_whole_file(Some(BASE), Some(LOCAL), Some(REMOTE), Some(2_000), Some(2_000));
        assert_eq!(
            outcome,
            WholeFileOutcome::Split {
                winner: LOCAL.to_owned(),
                winner_is_local: true,
                loser: REMOTE.to_owned(),
            }
        );
    }
}
