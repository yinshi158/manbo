//! 合并的分发入口与共用的按行解析约定。
//!
//! 所有行表共用学习 crate 的加载约定：空行、首尾空白与 `#` 注释行跳过，格式不对的行静默跳过
//! （`debug` 记一条；同步每 15 分钟跑一轮，`warn` 会把一个永久坏行刷成日志噪音）。

mod counters;
mod glossary;
mod vocab;
mod whole_file;

pub(crate) use whole_file::merge_whole_file;
pub use whole_file::WholeFileOutcome;

use crate::registry::MergePolicy;

/// `user-words.tsv` 的固定词频（与 learning crate 的 `USER_WORD_FREQUENCY` 一致，两边别改岔）。
const UNION_MAX_FREQUENCY: u64 = 100;

/// 三方合并一张行表，返回合并后的文件内容（不带表头注释；学习器 / 统计下次落盘会按自己的格式补）。
///
/// 整文件的 [`MergePolicy::WholeFile`] 不走这里，由 [`merge_whole_file`] 处理；
/// 误传进来时原样返回本地并记警告（编程错误，测试应当拦住，运行中不 panic）。
pub fn merge(
    policy: MergePolicy,
    base: Option<&str>,
    local: Option<&str>,
    remote: Option<&str>,
) -> String {
    if policy == MergePolicy::WholeFile {
        tracing::warn!("整文件合并应走 merge_whole_file，这里原样返回本地");
        return local.unwrap_or("").to_owned();
    }
    let base = base.unwrap_or("");
    let local = local.unwrap_or("");
    let remote = remote.unwrap_or("");
    match policy {
        MergePolicy::Counters => counters::merge(counters::KeyColumns::AllButLast, None, base, local, remote),
        MergePolicy::DailyStats => counters::merge(counters::KeyColumns::First, None, base, local, remote),
        MergePolicy::UnionMax => counters::merge(
            counters::KeyColumns::AllButLast,
            Some(UNION_MAX_FREQUENCY),
            base,
            local,
            remote,
        ),
        MergePolicy::Vocabulary => vocab::merge(base, local, remote),
        MergePolicy::Glossary => glossary::merge(base, local, remote),
        MergePolicy::WholeFile => unreachable!("上面已拦"),
    }
}

/// 数据行：去首尾空白、空行与 `#` 注释。
pub(super) fn data_lines(source: &str) -> impl Iterator<Item = &str> {
    source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}
